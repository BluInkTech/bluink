// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tantivy::{Index, IndexReader};

use super::source::SourceId;
use super::state::{CachedResult, ServiceStatus, Work};
use crate::query::schema::QuerySchema;
use crossbeam_channel::{Receiver, Sender, bounded};

pub(crate) const PUBLICATION_TIMEOUT: Duration = Duration::from_secs(15);

/// Keep the schema owner and its reader together when publishing or acquiring.
#[derive(Clone)]
pub(crate) struct IndexSnapshot {
    pub(crate) index: Index,
    pub(crate) reader: IndexReader,
}

pub(crate) struct MainSnapshot {
    pub(crate) index: Index,
    pub(crate) reader: IndexReader,
    pub(crate) dirty: Arc<HashMap<String, String>>,
    pub(crate) query_fields: QuerySchema,
}

struct IndexState {
    reconciled: bool,
    snapshots: Option<IndexSnapshot>,
    query_fields: QuerySchema,
    last_read: Instant,
    status: ServiceStatus,
    dirty: Arc<HashMap<String, String>>,
    cache: HashMap<String, CachedResult>,
    cache_generation: u64,
    accepting_work: bool,
}

impl IndexState {
    fn check_readable(&self) -> Result<(), String> {
        if let Some(error) = &self.status.error {
            return Err(format!("Search indexing failed: {error}"));
        }
        if !self.accepting_work {
            return Err("Search index service is shutting down".to_owned());
        }
        Ok(())
    }

    fn main_snapshot(&self) -> Option<MainSnapshot> {
        if !self.reconciled {
            return None;
        }
        self.snapshots.as_ref().map(|snapshots| MainSnapshot {
            index: snapshots.index.clone(),
            reader: snapshots.reader.clone(),
            dirty: Arc::clone(&self.dirty),
            query_fields: self.query_fields.clone(),
        })
    }

    fn invalidate_cache(&mut self) -> Result<(), String> {
        self.cache_generation = self
            .cache_generation
            .checked_add(1)
            .ok_or("Index cache generation exhausted")?;
        self.cache.clear();
        Ok(())
    }
}

/// Captured with query inputs. Results may only enter this generation.
pub(crate) struct QueryCache {
    coordinator: Arc<IndexCoordinator>,
    generation: u64,
}

impl QueryCache {
    pub(crate) fn get(&self, key: &str) -> Result<Option<CachedResult>, String> {
        let state = self
            .coordinator
            .state
            .lock()
            .map_err(|error| error.to_string())?;
        if state.cache_generation != self.generation || !state.dirty.is_empty() {
            return Ok(None);
        }
        Ok(state.cache.get(key).cloned())
    }

    pub(crate) fn insert(&self, key: String, value: CachedResult) -> Result<(), String> {
        let mut state = self
            .coordinator
            .state
            .lock()
            .map_err(|error| error.to_string())?;
        if state.cache_generation != self.generation || !state.dirty.is_empty() {
            return Ok(());
        }
        // Keep large facet/calendar responses bounded without an eviction framework.
        if state.cache.len() >= 16
            && let Some(evicted) = state.cache.keys().next().cloned()
        {
            state.cache.remove(&evicted);
        }
        state.cache.insert(key, value);
        drop(state);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn values(&self) -> Result<Vec<CachedResult>, String> {
        let state = self
            .coordinator
            .state
            .lock()
            .map_err(|error| error.to_string())?;
        Ok(state.cache.values().cloned().collect())
    }
}

/// Publication never exposes an incompletely opened set of projections.
/// Callers retain owned handles; they cannot hold the coordinator's lock.
pub(crate) struct IndexCoordinator {
    state: Mutex<IndexState>,
    changed: Condvar,
}

impl crate::engine::project::usage::WriteObserver for IndexCoordinator {
    fn pending(&self, id: &str, revision: &str) -> Result<(), String> {
        self.mark_dirty(id.to_owned(), revision.to_owned())
    }

    fn item_candidates(&self, kind: &str) -> Result<Vec<String>, String> {
        let snapshot = self
            .usage_snapshot()?
            .ok_or("Wait for initial indexing before checking item relationships.")?;
        crate::catalogue_usage::item_candidates(&snapshot, kind).map_err(|error| error.to_string())
    }
}

impl IndexCoordinator {
    pub(crate) fn new(query_fields: QuerySchema, cold: bool) -> Self {
        Self {
            changed: Condvar::new(),
            state: Mutex::new(IndexState {
                reconciled: false,
                snapshots: None,
                query_fields,
                last_read: Instant::now(),
                dirty: Arc::default(),
                cache: HashMap::new(),
                cache_generation: 0,
                accepting_work: true,
                status: ServiceStatus {
                    state: if cold {
                        "building".to_owned()
                    } else {
                        "reconciling".to_owned()
                    },
                    phase: "Opening search indexes".to_owned(),
                    processed: 0,
                    total: 0,
                    checked: 0,
                    changed: 0,
                    pending: 0,
                    documents: 0,
                    last_committed_at: None,
                    error: None,
                    cold,
                    schema_fingerprint: crate::indexing::index::SCHEMA_FINGERPRINT.to_owned(),
                    reconciliation_timings: None,
                },
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn cache(self: &Arc<Self>) -> Result<QueryCache, String> {
        let generation = self
            .state
            .lock()
            .map_err(|error| error.to_string())?
            .cache_generation;
        Ok(QueryCache {
            coordinator: Arc::clone(self),
            generation,
        })
    }

    pub(crate) fn invalidate_cache(&self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|error| error.to_string())?
            .invalidate_cache()
    }

    pub(crate) fn status(&self) -> Result<ServiceStatus, String> {
        let state = self.state.lock().map_err(|error| error.to_string())?;
        let mut status = state.status.clone();
        let accepted = state.dirty.len();
        drop(state);
        status.pending = status.pending.max(accepted);
        if accepted > 0 && status.state == "ready" {
            "reconciling".clone_into(&mut status.state);
            "Indexing changed files".clone_into(&mut status.phase);
        }
        Ok(status)
    }

    #[cfg(test)]
    pub(crate) fn dirty_revisions(&self) -> Result<HashMap<String, String>, String> {
        let state = self.state.lock().map_err(|error| error.to_string())?;
        Ok(state.dirty.as_ref().clone())
    }

    pub(crate) fn mark_dirty(&self, id: String, revision: String) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.invalidate_cache()?;
        Arc::make_mut(&mut state.dirty).insert(id, revision);
        drop(state);
        Ok(())
    }

    pub(crate) fn pending_revision(&self, id: &str) -> Result<Option<String>, String> {
        let state = self.state.lock().map_err(|error| error.to_string())?;
        Ok(state.dirty.get(id).cloned())
    }

    /// An external read must not replace a canonical revision published during it.
    pub(crate) fn observe_revision(
        &self,
        id: &str,
        before_read: Option<&str>,
        observed: &str,
    ) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if state.dirty.get(id).map(String::as_str) == before_read {
            state.invalidate_cache()?;
            Arc::make_mut(&mut state.dirty).insert(id.to_owned(), observed.to_owned());
        }
        drop(state);
        Ok(())
    }

    pub(crate) fn submit(
        &self,
        sender: &Sender<Work>,
        id: String,
        revision: Option<String>,
    ) -> Result<(), String> {
        let identity = SourceId::try_from(id.as_str())?;
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if !state.accepting_work {
            return Err("Search index service is shutting down".to_owned());
        }
        state.invalidate_cache()?;
        // Canonical publication already records its revision under the relationship
        // lock. Delayed notifications (including watchers) cannot overwrite it.
        Arc::make_mut(&mut state.dirty)
            .entry(id)
            .or_insert_with(|| revision.clone().unwrap_or_default());
        // This channel is unbounded: send cannot wait for the worker. Holding
        // admission through send orders accepted writes before Shutdown.
        let sent = sender.send(Work::Upsert {
            id: identity,
            expected_revision: revision,
        });
        drop(state);
        sent.map_err(|error| error.to_string())
    }

    /// Admit one complete logical mutation; no other publication can split it.
    pub(crate) fn submit_mutation(
        &self,
        sender: &Sender<Work>,
        changes: &[(String, String)],
    ) -> Result<Receiver<Result<(), String>>, String> {
        let sources = changes
            .iter()
            .map(|(id, revision)| Ok((SourceId::try_from(id.as_str())?, revision.clone())))
            .collect::<Result<Vec<_>, String>>()?;
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.check_readable()?;
        state.invalidate_cache()?;
        for (id, revision) in changes {
            Arc::make_mut(&mut state.dirty)
                .entry(id.clone())
                .or_insert_with(|| revision.clone());
        }
        let (completion, received) = bounded(1);
        let sent = sender.send(Work::Mutation {
            sources,
            completion,
        });
        drop(state);
        sent.map_err(|error| error.to_string())?;
        Ok(received)
    }

    pub(crate) fn stop(&self, sender: &Sender<Work>) -> Result<(), String> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(error) => {
                // Cleanup must still wake the worker when state is poisoned.
                let _ = sender.send(Work::Shutdown);
                return Err(error.to_string());
            }
        };
        state.accepting_work = false;
        self.changed.notify_all();
        // A worker that failed startup may already have disconnected.
        let _ = sender.send(Work::Shutdown);
        drop(state);
        Ok(())
    }

    /// Startup readiness belongs to the wrapper, not polling feature callers.
    pub(crate) fn wait_until_reconciled(&self) -> Result<(), String> {
        let deadline = Instant::now() + PUBLICATION_TIMEOUT;
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        loop {
            state.check_readable()?;
            if state.reconciled {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(
                    "Search is still reconciling; retry the query after startup completes."
                        .to_owned(),
                );
            }
            (state, _) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|error| error.to_string())?;
        }
    }

    /// A completed batch cannot acknowledge a newer write accepted meanwhile.
    pub(crate) fn acknowledge(
        &self,
        revisions: impl IntoIterator<Item = (String, String)>,
    ) -> Result<usize, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.invalidate_cache()?;
        for (id, revision) in revisions {
            if state.dirty.get(&id) == Some(&revision) {
                Arc::make_mut(&mut state.dirty).remove(&id);
            }
        }
        Ok(state.dirty.len())
    }

    /// A watcher event with no revision may already match the published index.
    pub(crate) fn acknowledge_current(
        &self,
        id: &str,
        revision: &str,
        external: bool,
    ) -> Result<usize, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.invalidate_cache()?;
        if state
            .dirty
            .get(id)
            .is_some_and(|pending| pending == revision || (external && pending.is_empty()))
        {
            Arc::make_mut(&mut state.dirty).remove(id);
        }
        Ok(state.dirty.len())
    }

    pub(crate) fn update_status(
        &self,
        update: impl FnOnce(&mut ServiceStatus),
    ) -> Result<(), String> {
        {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            update(&mut state.status);
            drop(state);
            self.changed.notify_all();
        }
        Ok(())
    }

    pub(crate) fn replace_query_fields(&self, fields: QuerySchema) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.invalidate_cache()?;
        state.query_fields = fields;
        drop(state);
        Ok(())
    }

    pub(crate) fn last_read(&self) -> Result<Instant, String> {
        let state = self.state.lock().map_err(|error| error.to_string())?;
        Ok(state.last_read)
    }

    #[cfg(test)]
    pub(crate) fn snapshots(&self) -> Result<Option<IndexSnapshot>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.last_read = Instant::now();
        Ok(state.snapshots.clone())
    }

    pub(crate) fn main_snapshot(&self) -> Result<Option<MainSnapshot>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.check_readable()?;
        state.last_read = Instant::now();
        Ok(state.main_snapshot())
    }

    /// Deletion needs a complete baseline; pending canonical writes are overlaid.
    pub(crate) fn usage_snapshot(&self) -> Result<Option<MainSnapshot>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if !state.accepting_work || !state.reconciled || state.status.error.is_some() {
            return Ok(None);
        }
        state.last_read = Instant::now();
        Ok(state.main_snapshot())
    }

    pub(crate) fn complete_reconciliation(&self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|error| error.to_string())?
            .reconciled = true;
        self.changed.notify_all();
        Ok(())
    }

    /// The cache token and all inputs belong to the same coordinator capture.
    pub(crate) fn cached_snapshot(
        self: &Arc<Self>,
    ) -> Result<Option<(MainSnapshot, QueryCache)>, String> {
        self.capture_cached(false)
    }

    /// Planning requires settled inputs; readiness and capture are indivisible.
    pub(crate) fn ready_cached_snapshot(
        self: &Arc<Self>,
    ) -> Result<Option<(MainSnapshot, QueryCache)>, String> {
        self.capture_cached(true)
    }

    fn capture_cached(
        self: &Arc<Self>,
        require_ready: bool,
    ) -> Result<Option<(MainSnapshot, QueryCache)>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.check_readable()?;
        if require_ready
            && (!state.accepting_work
                || state.status.state != "ready"
                || state.status.pending != 0
                || !state.dirty.is_empty())
        {
            return Ok(None);
        }
        state.last_read = Instant::now();
        Ok(state.main_snapshot().map(|snapshot| {
            let cache = QueryCache {
                coordinator: Arc::clone(self),
                generation: state.cache_generation,
            };
            (snapshot, cache)
        }))
    }

    pub(crate) fn query_snapshot(&self) -> Result<Option<MainSnapshot>, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.check_readable()?;
        state.last_read = Instant::now();
        Ok(state.main_snapshot())
    }

    pub(crate) fn publish(&self, snapshots: IndexSnapshot) -> Result<(), String> {
        let previous = {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            state.invalidate_cache()?;
            state.snapshots.replace(snapshots)
        };
        // Releasing readers and their segment leases can do filesystem work.
        drop(previous);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexing::index::{open_reader, open_writer};
    use tantivy::schema::{STORED, Schema};

    #[test]
    fn startup_waiters_wake_on_reconciliation_failure_and_shutdown() {
        for outcome in ["ready", "failed", "stopped"] {
            let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), false));
            let index = Index::create_in_ram(Schema::builder().build());
            coordinator.publish(bundle(&index)).unwrap();
            // Opening old readers does not expose an unreconciled baseline.
            assert!(coordinator.main_snapshot().unwrap().is_none());
            let waiting = Arc::clone(&coordinator);
            let (done, received) = bounded(1);
            let worker = std::thread::spawn(move || {
                done.send(waiting.wait_until_reconciled()).unwrap();
            });
            assert!(received.try_recv().is_err());
            match outcome {
                "ready" => coordinator.complete_reconciliation().unwrap(),
                "failed" => coordinator
                    .update_status(|status| {
                        status.error = Some("Disk unavailable".to_owned());
                    })
                    .unwrap(),
                _ => {
                    let (sender, _receiver) = crossbeam_channel::unbounded();
                    coordinator.stop(&sender).unwrap();
                }
            }
            let result = received.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(result.is_ok(), outcome == "ready");
            worker.join().unwrap();
            if outcome != "ready" {
                assert!(coordinator.main_snapshot().is_err());
                assert!(coordinator.query_snapshot().is_err());
                assert!(coordinator.cached_snapshot().is_err());
            }
        }
    }

    fn bundle(index: &Index) -> IndexSnapshot {
        IndexSnapshot {
            index: index.clone(),
            reader: open_reader(index).unwrap(),
        }
    }

    #[test]
    fn replacement_preserves_acquired_readers_and_publishes_every_projection() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        assert!(coordinator.snapshots().unwrap().is_none());
        let mut schema = Schema::builder();
        let field = schema.add_u64_field("value", STORED);
        let index = Index::create_in_ram(schema.build());
        coordinator.publish(bundle(&index)).unwrap();
        let acquired = coordinator.snapshots().unwrap().unwrap();
        coordinator.complete_reconciliation().unwrap();
        coordinator
            .mark_dirty("note".into(), "revision".into())
            .unwrap();
        let pending_query = coordinator.query_snapshot().unwrap().unwrap();
        let pending_read = &pending_query;
        coordinator
            .replace_query_fields(QuerySchema::from_properties(std::iter::once((
                "custom_date".to_owned(),
                crate::engine::project::types::MetadataFieldValueType::Timestamp,
            ))))
            .unwrap();

        let mut writer = open_writer(&index).unwrap();
        writer.add_document(tantivy::doc!(field => 1_u64)).unwrap();
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        coordinator.publish(bundle(&index)).unwrap();
        coordinator
            .acknowledge([("note".into(), "revision".into())])
            .unwrap();
        assert_eq!(pending_read.reader.searcher().num_docs(), 0);
        assert_eq!(pending_read.dirty["note"], "revision");
        let published_query = coordinator.query_snapshot().unwrap().unwrap();
        let published_read = &published_query;
        assert!(!pending_read.query_fields.contains("custom_date"));
        assert!(published_read.query_fields.contains("custom_date"));
        assert_eq!(published_read.reader.searcher().num_docs(), 1);
        assert!(published_read.dirty.is_empty());

        let current = coordinator.snapshots().unwrap().unwrap();
        assert_eq!(acquired.reader.searcher().num_docs(), 0);
        assert_eq!(current.reader.searcher().num_docs(), 1);
    }

    #[test]
    fn malformed_submission_leaves_queue_dirty_state_and_cache_untouched() {
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), false));
        let (sender, receiver) = crossbeam_channel::unbounded();
        let cache = coordinator.cache().unwrap();
        cache
            .insert("saved".into(), CachedResult::Facets(Vec::new()))
            .unwrap();
        for id in ["", "../outside", "not-a-uuid"] {
            assert!(coordinator.submit(&sender, id.to_owned(), None).is_err());
        }
        assert!(receiver.is_empty());
        assert!(coordinator.dirty_revisions().unwrap().is_empty());
        assert!(cache.get("saved").unwrap().is_some());
    }

    #[test]
    fn shutdown_orders_accepted_submissions_before_its_message() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        let (sender, receiver) = crossbeam_channel::unbounded();
        coordinator
            .submit(
                &sender,
                "00000000-0000-7000-8000-000000000010".into(),
                Some("revision".into()),
            )
            .unwrap();
        let accepted = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|number| {
                    let coordinator = &coordinator;
                    let sender = &sender;
                    scope.spawn(move || {
                        coordinator
                            .submit(
                                sender,
                                format!("00000000-0000-7000-8000-{number:012}"),
                                None,
                            )
                            .is_ok()
                    })
                })
                .collect();
            coordinator.stop(&sender).unwrap();
            // Join every producer before inspecting the complete channel.
            // A producer racing shutdown may either be admitted or rejected.
            handles
                .into_iter()
                .map(|handle| usize::from(handle.join().unwrap()))
                .sum::<usize>()
        });
        assert!(
            coordinator
                .submit(&sender, "00000000-0000-7000-8000-000000000011".into(), None)
                .is_err()
        );
        let messages: Vec<_> = receiver.try_iter().collect();
        assert_eq!(messages.len(), accepted + 2);
        assert!(
            matches!(messages.first(), Some(Work::Upsert { id, .. }) if id.as_str() == "00000000-0000-7000-8000-000000000010")
        );
        assert!(matches!(messages.last(), Some(Work::Shutdown)));
        assert!(
            messages[..messages.len() - 1]
                .iter()
                .all(|work| matches!(work, Work::Upsert { .. }))
        );
        assert!(
            !coordinator
                .dirty_revisions()
                .unwrap()
                .contains_key("00000000-0000-7000-8000-000000000011")
        );
    }

    #[test]
    fn planning_capture_requires_ready_clean_admitted_state() {
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), false));
        assert!(coordinator.ready_cached_snapshot().unwrap().is_none());
        let index = Index::create_in_ram(Schema::builder().build());
        coordinator.publish(bundle(&index)).unwrap();
        coordinator.complete_reconciliation().unwrap();
        for phase in ["starting", "reconciling", "error"] {
            coordinator
                .update_status(|status| phase.clone_into(&mut status.state))
                .unwrap();
            assert!(coordinator.ready_cached_snapshot().unwrap().is_none());
        }
        coordinator
            .update_status(|status| {
                "ready".clone_into(&mut status.state);
                status.pending = 1;
            })
            .unwrap();
        assert!(coordinator.ready_cached_snapshot().unwrap().is_none());
        coordinator
            .update_status(|status| status.pending = 0)
            .unwrap();
        coordinator
            .mark_dirty("page".into(), "revision".into())
            .unwrap();
        assert!(coordinator.ready_cached_snapshot().unwrap().is_none());
        coordinator
            .acknowledge([("page".into(), "revision".into())])
            .unwrap();
        let (snapshot, _) = coordinator.ready_cached_snapshot().unwrap().unwrap();
        assert!(snapshot.dirty.is_empty());
        let (sender, _receiver) = crossbeam_channel::unbounded();
        coordinator.stop(&sender).unwrap();
        assert!(coordinator.ready_cached_snapshot().is_err());
    }

    #[test]
    fn cached_capture_keeps_its_reader_and_rejects_results_after_publication() {
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), false));
        assert!(coordinator.cached_snapshot().unwrap().is_none());
        let mut schema = Schema::builder();
        let title = schema.add_text_field("title", STORED);
        let index = Index::create_in_ram(schema.build());
        coordinator.publish(bundle(&index)).unwrap();
        coordinator.complete_reconciliation().unwrap();
        let (before, old_cache) = coordinator.cached_snapshot().unwrap().unwrap();
        old_cache
            .insert("facets".into(), CachedResult::Facets(Vec::new()))
            .unwrap();
        assert!(old_cache.get("facets").unwrap().is_some());

        let mut writer = open_writer(&index).unwrap();
        writer
            .add_document(tantivy::doc!(title => "New generation"))
            .unwrap();
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        coordinator.publish(bundle(&index)).unwrap();

        let (after, current_cache) = coordinator.cached_snapshot().unwrap().unwrap();
        assert_eq!(before.reader.searcher().num_docs(), 0);
        assert_eq!(after.reader.searcher().num_docs(), 1);
        assert!(old_cache.get("facets").unwrap().is_none());
        old_cache
            .insert("stale".into(), CachedResult::Facets(Vec::new()))
            .unwrap();
        assert!(current_cache.get("stale").unwrap().is_none());
        current_cache
            .insert("current".into(), CachedResult::Facets(Vec::new()))
            .unwrap();
        assert!(current_cache.get("current").unwrap().is_some());
    }

    #[test]
    fn cache_rejects_results_computed_before_invalidation_or_during_dirty_work() {
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), false));
        let result = || CachedResult::Facets(Vec::new());
        let before_write = coordinator.cache().unwrap();
        before_write.insert("facets".into(), result()).unwrap();
        assert!(before_write.get("facets").unwrap().is_some());

        coordinator
            .mark_dirty("note".into(), "revision".into())
            .unwrap();
        assert!(before_write.get("facets").unwrap().is_none());
        let during_write = coordinator.cache().unwrap();
        during_write.insert("during".into(), result()).unwrap();
        assert!(during_write.values().unwrap().is_empty());
        coordinator
            .acknowledge([("note".into(), "revision".into())])
            .unwrap();
        before_write.insert("old".into(), result()).unwrap();
        during_write.insert("old-dirty".into(), result()).unwrap();
        assert!(coordinator.cache().unwrap().values().unwrap().is_empty());

        let before_schema = coordinator.cache().unwrap();
        coordinator
            .replace_query_fields(QuerySchema::default())
            .unwrap();
        before_schema.insert("old-schema".into(), result()).unwrap();
        assert!(coordinator.cache().unwrap().values().unwrap().is_empty());

        let fresh = coordinator.cache().unwrap();
        fresh.insert("fresh".into(), result()).unwrap();
        assert!(fresh.get("fresh").unwrap().is_some());
        coordinator.invalidate_cache().unwrap();
        fresh.insert("late".into(), result()).unwrap();
        assert!(coordinator.cache().unwrap().values().unwrap().is_empty());
    }

    #[test]
    fn acknowledgements_preserve_newer_writes_and_distinguish_unknown_events() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        coordinator
            .mark_dirty("note".into(), "newer".into())
            .unwrap();
        assert_eq!(
            coordinator
                .acknowledge([("note".into(), "older".into())])
                .unwrap(),
            1
        );
        assert_eq!(
            coordinator
                .acknowledge_current("note", "older", true)
                .unwrap(),
            1
        );
        assert_eq!(coordinator.dirty_revisions().unwrap()["note"], "newer");
        assert_eq!(
            coordinator
                .acknowledge([("note".into(), "newer".into())])
                .unwrap(),
            0
        );

        coordinator
            .mark_dirty("external".into(), String::new())
            .unwrap();
        assert_eq!(
            coordinator
                .acknowledge_current("external", "stored", false)
                .unwrap(),
            1
        );
        assert_eq!(
            coordinator
                .acknowledge_current("external", "stored", true)
                .unwrap(),
            0
        );
    }

    #[test]
    fn delayed_notifications_and_external_reads_preserve_canonical_revisions() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        let id = "00000000-0000-7000-8000-000000000011";
        let (sender, _receiver) = crossbeam_channel::unbounded();
        coordinator.mark_dirty(id.into(), "old".into()).unwrap();
        let before_read = coordinator.pending_revision(id).unwrap();
        coordinator.mark_dirty(id.into(), "new".into()).unwrap();
        coordinator
            .submit(&sender, id.into(), Some("old".into()))
            .unwrap();
        coordinator.submit(&sender, id.into(), None).unwrap();
        coordinator
            .observe_revision(id, before_read.as_deref(), "old")
            .unwrap();
        coordinator.acknowledge_current(id, "old", true).unwrap();
        assert_eq!(
            coordinator.pending_revision(id).unwrap().as_deref(),
            Some("new")
        );
        coordinator.observe_revision(id, Some("new"), "").unwrap();
        coordinator
            .acknowledge([(id.into(), String::new())])
            .unwrap();
        assert!(coordinator.pending_revision(id).unwrap().is_none());
    }

    #[test]
    fn usage_capture_rejects_incomplete_failed_and_stopped_indexes() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        let index = Index::create_in_ram(Schema::builder().build());
        coordinator.publish(bundle(&index)).unwrap();
        assert!(coordinator.usage_snapshot().unwrap().is_none());
        coordinator
            .update_status(|status| {
                status.reconciliation_timings =
                    Some(crate::indexing::state::ReconciliationTimings::default());
            })
            .unwrap();
        assert!(coordinator.usage_snapshot().unwrap().is_none());
        coordinator.complete_reconciliation().unwrap();
        coordinator.mark_dirty("note".into(), "new".into()).unwrap();
        assert_eq!(
            coordinator.usage_snapshot().unwrap().unwrap().dirty["note"],
            "new"
        );
        coordinator
            .update_status(|status| status.error = Some("failed".into()))
            .unwrap();
        assert!(coordinator.usage_snapshot().unwrap().is_none());
        coordinator
            .update_status(|status| status.error = None)
            .unwrap();
        let (sender, _receiver) = crossbeam_channel::unbounded();
        coordinator.stop(&sender).unwrap();
        assert!(coordinator.usage_snapshot().unwrap().is_none());
    }

    #[test]
    fn poisoned_state_is_reported_instead_of_returning_a_snapshot() {
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        let _ = std::panic::catch_unwind(|| {
            let _guard = coordinator.state.lock().unwrap();
            panic!("Simulate an interrupted publication");
        });
        assert!(coordinator.snapshots().is_err());
        assert!(coordinator.status().is_err());
        let (sender, receiver) = crossbeam_channel::unbounded();
        assert!(coordinator.stop(&sender).is_err());
        assert!(matches!(receiver.try_recv().unwrap(), Work::Shutdown));
        assert!(coordinator.update_status(|_| unreachable!()).is_err());
        assert!(coordinator.dirty_revisions().is_err());
        assert!(
            coordinator
                .mark_dirty("note".into(), "revision".into())
                .is_err()
        );
        assert!(
            coordinator
                .acknowledge([("note".into(), "revision".into())])
                .is_err()
        );
        let index = Index::create_in_ram(Schema::builder().build());
        assert!(coordinator.publish(bundle(&index)).is_err());
    }
}
