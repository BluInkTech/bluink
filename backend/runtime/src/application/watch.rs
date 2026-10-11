// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Node delivers OS events. Rust owns their meaning, batching and reconciliation.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

use super::error::{ApplicationError, Result};

const QUIET: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_secs(2);
const RESCAN_INTERVAL: Duration = Duration::from_mins(5);

#[derive(serde::Serialize)]
pub(super) struct SubscriptionRoot {
    path: PathBuf,
    prefix: &'static str,
}

pub(super) fn subscription_roots(root: &Path) -> Result<Vec<SubscriptionRoot>> {
    // Watch only canonical input trees. A project can contain unrelated tools,
    // caches or dependencies that must not acquire thousands of subscriptions.
    ["items", "properties", "facets", "analytics/events"]
        .into_iter()
        .map(|prefix| {
            let path = root.join(prefix);
            std::fs::create_dir_all(&path)?;
            Ok(SubscriptionRoot { path, prefix })
        })
        .collect()
}

enum Change {
    Item(String),
    Schema,
    Activity,
    Facets,
    Full,
    Ignore,
}

fn classify(filename: Option<&str>) -> Change {
    let Some(filename) = filename else {
        return Change::Full;
    };
    let normalized = filename.replace('\\', "/");
    let parts: Vec<_> = normalized.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == ".." || *part == "." || part.contains(':'))
    {
        return Change::Ignore;
    }
    match parts.as_slice() {
        ["items"] | ["items", _] => Change::Full,
        ["items", shard, id] | ["items", shard, id, "item.json"]
            if crate::engine::project::item::is_item_id(id)
                && id[..2].eq_ignore_ascii_case(shard) =>
        {
            Change::Item((*id).to_owned())
        }
        ["properties", ..] => Change::Schema,
        ["analytics", "events", ..] => Change::Activity,
        ["facets", ..] => Change::Facets,
        _ => Change::Ignore,
    }
}

pub(super) struct Coordinator {
    sender: Sender<Option<String>>,
    full: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
}

impl Coordinator {
    pub(super) fn start(root: PathBuf, index: String) -> Result<Self> {
        Self::start_with_interval(root, index, RESCAN_INTERVAL)
    }

    pub(super) fn start_with_interval(
        root: PathBuf,
        index: String,
        interval: Duration,
    ) -> Result<Self> {
        let (sender, receiver) = bounded::<Option<String>>(1024);
        let full = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let events = EventWorker {
            root,
            index,
            full: full.clone(),
            stopping: stopping.clone(),
            failure: failure.clone(),
        };
        let worker = std::thread::Builder::new()
            .name("bluink-reconcile-events".to_owned())
            .spawn(move || events.run(receiver, interval))?;
        Ok(Self {
            sender,
            full,
            stopping,
            failure,
            worker: Some(worker),
        })
    }

    pub(super) fn notify(&self, filename: Option<String>) {
        if self.sender.try_send(filename).is_err() {
            // A bounded subscription queue must not silently lose changes.
            self.full.store(true, Ordering::Release);
        }
    }

    pub(super) fn report_failure(&self) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(
                "Filesystem subscription unavailable; external changes use the periodic rescan."
                    .to_owned(),
            );
        }
        self.full.store(true, Ordering::Release);
        tracing::warn!(
            operation = "watcher.subscription",
            error_class = "filesystem_subscription_unavailable"
        );
    }

    pub(super) fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }

    pub(super) fn stop(&mut self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| {
                ApplicationError::Internal("Project event worker panicked.".to_owned())
            })?;
        }
        Ok(())
    }
}

struct EventWorker {
    root: PathBuf,
    index: String,
    full: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

impl EventWorker {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The worker owns its receiver and lifetime until shutdown."
    )]
    fn run(self, receiver: Receiver<Option<String>>, interval: Duration) {
        let mut pending = Pending::default();
        let mut first = Instant::now();
        let mut last = first;
        let mut next_rescan = Instant::now() + interval;
        let mut next_release = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(filename) = receiver.recv_timeout(Duration::from_millis(50)) {
                let change = classify(filename.as_deref());
                if !matches!(change, Change::Ignore) {
                    if pending.empty() {
                        first = Instant::now();
                    }
                    pending.add(change);
                    last = Instant::now();
                }
            }
            if self.full.swap(false, Ordering::AcqRel) {
                pending.full = true;
            }
            let stopping = self.stopping.load(Ordering::Acquire);
            if !stopping && Instant::now() >= next_release {
                crate::memory::release_unused();
                next_release = Instant::now() + Duration::from_secs(5);
            }
            if !stopping && Instant::now() >= next_rescan {
                pending.full = true;
                next_rescan = Instant::now() + interval;
            }
            if !pending.empty()
                && (stopping || last.elapsed() >= QUIET || first.elapsed() >= MAX_WAIT)
                && let Err(error) = reconcile(&self.root, &self.index, std::mem::take(&mut pending))
            {
                self.report_failure(&error);
            }
            if stopping {
                // Admission stops before shutdown. Drain accepted events before joining.
                while let Ok(filename) = receiver.try_recv() {
                    pending.add(classify(filename.as_deref()));
                }
                if !pending.empty()
                    && let Err(error) = reconcile(&self.root, &self.index, pending)
                {
                    self.report_failure(&error);
                }
                break;
            }
        }
    }

    fn report_failure(&self, error: &ApplicationError) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some("External project changes could not be reconciled.".to_owned());
        }
        tracing::error!(operation = "watcher.reconcile", error_class = "external_reconciliation_failed", error = %error);
    }
}

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each flag independently marks a dirty project projection."
)]
struct Pending {
    items: HashSet<String>,
    schema: bool,
    activity: bool,
    facets: bool,
    full: bool,
}
impl Pending {
    fn empty(&self) -> bool {
        self.items.is_empty() && !self.schema && !self.activity && !self.facets && !self.full
    }
    fn add(&mut self, change: Change) {
        match change {
            Change::Item(id) => {
                self.items.insert(id);
            }
            Change::Schema => self.schema = true,
            Change::Activity => self.activity = true,
            Change::Facets => self.facets = true,
            Change::Full => self.full = true,
            Change::Ignore => {}
        }
    }
}

fn reconcile(root: &Path, index: &str, mut pending: Pending) -> Result<()> {
    let _span = tracing::debug_span!(
        "watcher.reconcile",
        full = pending.full,
        item_count = pending.items.len()
    )
    .entered();
    if pending.full {
        // Compare metadata with committed stamps, not a second resident copy of
        // every item. Unchanged files are never parsed or queued for projection.
        let (_, reader, _) = crate::service::service_index(index)?;
        let mut indexed = crate::index::indexed_file_stamps(&reader, "id", false, |_, _| {})
            .map_err(ApplicationError::Internal)?;
        crate::engine::project::item::visit_project_item_ids(root, |id| {
            let key = crate::project::ItemId::parse(id)
                .ok_or_else(|| format!("Invalid item ID: {id}"))?;
            let previous = indexed.remove(&key);
            let path = crate::project::item_path(root, id);
            match crate::project::FileStamp::read(&path)? {
                Some(stamp) => {
                    if previous != Some(Some(stamp)) {
                        pending.items.insert(id.to_owned());
                    }
                }
                None => {
                    if previous.is_some() {
                        pending.items.insert(id.to_owned());
                    }
                }
            }
            Ok(())
        })
        .map_err(ApplicationError::Internal)?;
        pending
            .items
            .extend(indexed.into_keys().map(|id| id.as_str().to_owned()));
        pending.schema = true;
        pending.activity = true;
        pending.facets = true;
    }
    for id in pending.items {
        crate::service::enqueue_external_index_item(index.to_owned(), id)
            .map_err(|error| ApplicationError::Internal(error.to_string()))?;
    }
    if pending.schema {
        crate::service::refresh_query_fields(index.to_owned())
            .map_err(|error| ApplicationError::Internal(error.to_string()))?;
    }
    if pending.facets {
        crate::catalogue::invalidate_facets(&root.to_string_lossy())
            .map_err(|error| ApplicationError::Internal(error.to_string()))?;
    }
    if pending.activity {
        crate::engine::project::activity::reconcile_activity(root.to_string_lossy().into_owned())
            .map_err(ApplicationError::Internal)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-7000-8000-000000000099";
    #[test]
    fn events_only_target_canonical_files_and_unnamed_events_reconcile() {
        assert!(matches!(classify(None), Change::Full));
        for path in [
            format!("items/00/{ID}"),
            format!("items\\00\\{ID}\\item.json"),
        ] {
            assert!(matches!(classify(Some(&path)), Change::Item(id) if id == ID));
        }
        for path in [
            format!("items/00/{ID}/history/item.json"),
            format!("items/00/{ID}/assets/item.json"),
            format!("items/../{ID}/item.json"),
            format!("items/ff/{ID}/item.json"),
            ".cache/index/file".to_owned(),
        ] {
            assert!(matches!(classify(Some(&path)), Change::Ignore));
        }
        assert!(matches!(
            classify(Some("properties/kind.string.property")),
            Change::Schema
        ));
        assert!(matches!(
            classify(Some("analytics/events/day.json")),
            Change::Activity
        ));
        let mut pending = Pending::default();
        pending.add(Change::Item(ID.to_owned()));
        pending.add(Change::Item(ID.to_owned()));
        assert_eq!(pending.items.len(), 1);
    }

    #[test]
    fn a_full_event_queue_requests_reconciliation_instead_of_losing_changes() {
        let (sender, _receiver) = bounded(1);
        let coordinator = Coordinator {
            sender,
            full: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(Mutex::new(None)),
            worker: None,
        };
        coordinator.notify(Some("first".to_owned()));
        coordinator.notify(Some("second".to_owned()));
        assert!(coordinator.full.load(Ordering::Acquire));
    }
}
