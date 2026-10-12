// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded native subscriptions, batching and canonical-file reconciliation.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::error::{ApplicationError, Result};

const QUIET: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_secs(2);
const RESCAN_INTERVAL: Duration = Duration::from_mins(5);
const ROOTS: [&str; 4] = ["items", "properties", "facets", "analytics/events"];
const SUBSCRIPTION_FAILURE: &str =
    "Filesystem subscription unavailable; external changes use the periodic rescan.";

#[derive(Clone)]
struct Ingress {
    root: PathBuf,
    sender: Sender<Option<String>>,
    full: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

impl Ingress {
    fn enqueue(&self, filename: Option<String>) {
        if !self.stopping.load(Ordering::Acquire) && self.sender.try_send(filename).is_err() {
            self.full.store(true, Ordering::Release);
        }
    }

    fn receive(&self, event: notify::Result<Event>) {
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        let Ok(event) = event else {
            self.subscription_failed();
            return;
        };
        // Reconciliation reads must never trigger another reconciliation.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event.need_rescan() || event.paths.is_empty() {
            self.enqueue(None);
            return;
        }
        for path in event.paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if let Some(filename) = relative.to_str() {
                if !matches!(classify(Some(filename)), Change::Ignore) {
                    self.enqueue(Some(filename.to_owned()));
                }
            } else {
                self.enqueue(None);
            }
        }
    }

    fn subscription_failed(&self) {
        if let Ok(mut failure) = self.failure.lock() {
            if failure.as_deref() != Some(SUBSCRIPTION_FAILURE) {
                tracing::warn!(
                    operation = "watcher.subscription",
                    error_class = "filesystem_subscription_unavailable"
                );
            }
            *failure = Some(SUBSCRIPTION_FAILURE.to_owned());
        }
        self.full.store(true, Ordering::Release);
    }
}

struct Subscriptions {
    ingress: Ingress,
    watcher: Option<RecommendedWatcher>,
}

impl Subscriptions {
    fn refresh(&mut self) {
        // Reconnect four roots at each periodic scan, including replaced roots
        // whose old native subscriptions may no longer report any events.
        self.watcher.take();
        let ingress = self.ingress.clone();
        let Ok(mut watcher) = notify::recommended_watcher(move |event| ingress.receive(event))
        else {
            self.ingress.subscription_failed();
            return;
        };
        let mut healthy = true;
        for prefix in ROOTS {
            if watcher
                .watch(&self.ingress.root.join(prefix), RecursiveMode::NonRecursive)
                .is_err()
            {
                healthy = false;
            }
        }
        self.watcher = Some(watcher);
        if healthy {
            if let Ok(mut failure) = self.ingress.failure.lock()
                && failure.as_deref() == Some(SUBSCRIPTION_FAILURE)
            {
                *failure = None;
            }
        } else {
            self.ingress.subscription_failed();
        }
    }
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
    #[cfg(test)]
    ingress: Ingress,
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
        for prefix in ROOTS {
            std::fs::create_dir_all(root.join(prefix))?;
        }
        let ingress = Ingress {
            root: std::fs::canonicalize(&root)?,
            sender,
            full: full.clone(),
            stopping: stopping.clone(),
            failure: failure.clone(),
        };
        #[cfg(test)]
        let test_ingress = ingress.clone();
        let mut subscriptions = Subscriptions {
            ingress,
            watcher: None,
        };
        subscriptions.refresh();
        let events = EventWorker {
            root,
            index,
            full,
            stopping: stopping.clone(),
            failure: failure.clone(),
            subscriptions,
        };
        let worker = std::thread::Builder::new()
            .name("bluink-reconcile-events".to_owned())
            .spawn(move || events.run(receiver, interval))?;
        Ok(Self {
            #[cfg(test)]
            ingress: test_ingress,
            stopping,
            failure,
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    pub(super) fn enqueue_for_test(&self, filename: Option<String>) {
        self.ingress.enqueue(filename);
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
    subscriptions: Subscriptions,
}

impl EventWorker {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The worker owns its receiver and lifetime until shutdown."
    )]
    fn run(mut self, receiver: Receiver<Option<String>>, interval: Duration) {
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
            if stopping {
                // Stop native callbacks before draining the accepted queue.
                self.subscriptions.watcher.take();
                pending.full |= self.full.swap(false, Ordering::AcqRel);
            }
            if !stopping && Instant::now() >= next_release {
                crate::memory::release_unused();
                next_release = Instant::now() + Duration::from_secs(5);
            }
            if !stopping && Instant::now() >= next_rescan {
                self.subscriptions.refresh();
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
    use notify::event::{AccessKind, ModifyKind, RenameMode};
    const ID: &str = "00000000-0000-7000-8000-000000000099";

    fn ingress(root: &Path, capacity: usize) -> (Ingress, Receiver<Option<String>>) {
        let (sender, receiver) = bounded(capacity);
        (
            Ingress {
                root: root.to_owned(),
                sender,
                full: Arc::new(AtomicBool::new(false)),
                stopping: Arc::new(AtomicBool::new(false)),
                failure: Arc::new(Mutex::new(None)),
            },
            receiver,
        )
    }

    #[test]
    fn native_events_normalize_both_rename_paths_and_ignore_reads_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let (ingress, received) = ingress(directory.path(), 16);
        ingress.receive(Ok(Event::new(EventKind::Modify(ModifyKind::Name(
            RenameMode::Both,
        )))
        .add_path(directory.path().join("properties/old.string.property"))
        .add_path(directory.path().join("properties/new.string.property"))));
        let names: Vec<_> = received.try_iter().collect();
        assert_eq!(names.len(), 2);
        assert!(
            names
                .iter()
                .all(|name| matches!(classify(name.as_deref()), Change::Schema))
        );
        for path in [
            directory.path().join(".cache/file"),
            directory.path().join("../properties/outside"),
            directory
                .path()
                .join(format!("items/00/{ID}/assets/image.png")),
        ] {
            ingress.receive(Ok(Event::new(EventKind::Any).add_path(path)));
        }
        ingress.receive(Ok(Event::new(EventKind::Access(AccessKind::Any))
            .add_path(directory.path().join("properties/field"))));
        assert!(received.is_empty());
        ingress.receive(Ok(Event::new(EventKind::Any)));
        assert_eq!(received.try_recv().unwrap(), None);
        ingress.receive(Err(notify::Error::generic("subscription failure")));
        assert!(ingress.full.load(Ordering::Acquire));
        assert_eq!(
            ingress.failure.lock().unwrap().as_deref(),
            Some(SUBSCRIPTION_FAILURE)
        );
        ingress.stopping.store(true, Ordering::Release);
        ingress.receive(Ok(Event::new(EventKind::Any)));
        assert!(received.is_empty());
    }

    #[test]
    fn native_subscriptions_deliver_all_four_roots_and_stop_without_node() {
        let directory = tempfile::tempdir().unwrap();
        for prefix in ROOTS {
            std::fs::create_dir_all(directory.path().join(prefix).join("nested/deep")).unwrap();
        }
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let (ingress, received) = ingress(&root, 1024);
        let mut subscriptions = Subscriptions {
            ingress: ingress.clone(),
            watcher: None,
        };
        subscriptions.refresh();
        assert!(ingress.failure.lock().unwrap().is_none());
        let mut observed = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut probe = 0;
        while observed.len() != ROOTS.len() {
            for prefix in ROOTS {
                std::fs::write(root.join(prefix).join(format!("probe-{probe}")), "external")
                    .unwrap();
            }
            probe += 1;
            std::thread::sleep(Duration::from_millis(50));
            for filename in received.try_iter().flatten() {
                for prefix in ROOTS {
                    if filename
                        .replace('\\', "/")
                        .starts_with(&format!("{prefix}/"))
                    {
                        observed.insert(prefix);
                    }
                }
            }
            assert!(
                Instant::now() < deadline,
                "Missing native events: {observed:?}"
            );
        }
        ingress.stopping.store(true, Ordering::Release);
        subscriptions.watcher.take();
        received.try_iter().for_each(drop);
        std::fs::write(root.join("properties/after-stop"), "ignored").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(received.is_empty());
    }

    #[test]
    fn unavailable_roots_recover_on_refresh_and_replaced_roots_are_reconnected() {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let (ingress, _) = ingress(&root, 1024);
        let mut subscriptions = Subscriptions {
            ingress: ingress.clone(),
            watcher: None,
        };
        subscriptions.refresh();
        assert_eq!(
            ingress.failure.lock().unwrap().as_deref(),
            Some(SUBSCRIPTION_FAILURE)
        );
        for prefix in ROOTS {
            std::fs::create_dir_all(root.join(prefix)).unwrap();
        }
        subscriptions.refresh();
        assert!(ingress.failure.lock().unwrap().is_none());
        std::fs::remove_dir(root.join("facets")).unwrap();
        subscriptions.refresh();
        assert!(ingress.failure.lock().unwrap().is_some());
        std::fs::create_dir(root.join("facets")).unwrap();
        subscriptions.refresh();
        assert!(ingress.failure.lock().unwrap().is_none());
    }
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
        let ingress = Ingress {
            root: PathBuf::from("/project"),
            sender,
            full: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(Mutex::new(None)),
        };
        ingress.enqueue(Some("first".to_owned()));
        ingress.enqueue(Some("second".to_owned()));
        assert!(ingress.full.load(Ordering::Acquire));
    }
}
