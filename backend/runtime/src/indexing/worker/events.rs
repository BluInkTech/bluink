// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::projections::Projections;
use super::{acquire_writer, publish_reader, reconcile};
use crate::indexing::coordinator::{IndexCoordinator, IndexSnapshot};
use crate::indexing::index::{indexed_revision, now_ms, refresh_reader};
use crate::indexing::source::SourceId;
use crate::indexing::state::Work;
use crate::projection::FileStamp;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tantivy::{Index, IndexReader, IndexWriter};

pub(super) struct EventWorker {
    root: PathBuf,
    index_path: PathBuf,
    coordinator: Arc<IndexCoordinator>,
    index: Index,
    reader: IndexReader,
    projections: Projections,
    writer: Option<IndexWriter>,
    pending: usize,
    committed_revisions: Vec<(String, String)>,
    last_commit: Instant,
    last_evicted: Instant,
}

impl EventWorker {
    pub(super) fn new(
        root: PathBuf,
        index_path: PathBuf,
        coordinator: Arc<IndexCoordinator>,
        ready: reconcile::ReadyIndex,
    ) -> Result<Self, String> {
        let projections = Projections::new(&ready.index)?;
        Ok(Self {
            projections,
            root,
            index_path,
            coordinator,
            index: ready.index,
            reader: ready.reader,
            writer: None,
            pending: 0,
            committed_revisions: Vec::new(),
            last_commit: Instant::now(),
            last_evicted: Instant::now(),
        })
    }

    pub(super) fn run(&mut self, receiver: &Receiver<Work>) -> Result<(), String> {
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(Work::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Work::Mutation {
                    sources,
                    completion,
                }) => {
                    let result = self.publish_group(
                        sources
                            .into_iter()
                            .map(|(id, revision)| (id, Some(revision)))
                            .collect(),
                    );
                    let _ = completion.send(result.clone());
                    result?;
                }
                Ok(Work::Upsert {
                    id,
                    expected_revision,
                }) => {
                    self.publish_group(vec![(id, expected_revision)])?;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.release_idle()?;
        }
        if let Some(writer) = self.writer.take() {
            writer
                .wait_merging_threads()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn publish_group(&mut self, sources: Vec<(SourceId, Option<String>)>) -> Result<(), String> {
        use crate::engine::project::{io::LockMode, transaction};
        let _mutation =
            transaction::lock(&self.root, LockMode::Shared).map_err(|error| error.to_string())?;
        let mut changes = transaction::pending(&self.root)?
            .into_iter()
            .map(|(id, revision)| Ok((SourceId::try_from(id.as_str())?, Some(revision))))
            .collect::<Result<std::collections::HashMap<_, _>, String>>()?;
        changes.extend(sources);
        for (id, revision) in changes {
            self.apply(&id, revision.as_deref())?;
        }
        self.commit_pending()?;
        transaction::acknowledge(&self.root)
    }

    fn apply(
        &mut self,
        identity: &SourceId,
        expected_revision: Option<&str>,
    ) -> Result<(), String> {
        let path = identity.path(&self.root);
        let id = identity.as_str();
        let before_read = self.coordinator.pending_revision(id)?;
        if let Some(stamp) = FileStamp::read(&path)? {
            // An unreadable or invalid item is a failed projection, not a
            // deletion. Keep the published index and pending revision intact.
            let document = identity.read(&self.root, stamp)?;
            let revision = document.revision.clone();
            // File watchers can report metadata changes caused by a read.
            // The opaque canonical revision is the authoritative signal;
            // avoid rewriting the index and clearing result caches when it
            // has not changed.
            if indexed_revision(&self.reader, &self.projections.main, id)
                .map_err(|error| error.to_string())?
                .as_deref()
                == Some(revision.as_str())
            {
                let dirty_count = self.coordinator.acknowledge_current(
                    id,
                    &revision,
                    expected_revision.is_none(),
                )?;
                self.coordinator.update_status(|current| {
                    current.pending = self.pending.max(dirty_count);
                    current.state = if current.pending == 0 {
                        "ready".to_owned()
                    } else {
                        "reconciling".to_owned()
                    };
                })?;
                return Ok(());
            }
            self.coordinator.invalidate_cache()?;
            if expected_revision.is_none() {
                self.coordinator
                    .observe_revision(id, before_read.as_deref(), &revision)?;
            }
            let active_writer = acquire_writer(&mut self.writer, &self.index)?;
            self.projections.write(active_writer, document)?;
            self.committed_revisions.push((id.to_owned(), revision));
        } else {
            if expected_revision.is_none() {
                self.coordinator
                    .observe_revision(id, before_read.as_deref(), "")?;
            }
            let active_writer = acquire_writer(&mut self.writer, &self.index)?;
            self.projections.delete(active_writer, id);
            self.committed_revisions
                .push((id.to_owned(), String::new()));
        }
        self.pending += 1;
        Ok(())
    }

    fn commit_pending(&mut self) -> Result<(), String> {
        if self.pending == 0 {
            return Ok(());
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or("Pending index changes have no writer")?;
        writer.commit().map_err(|error| error.to_string())?;
        self.reader = publish_reader(&self.coordinator, &self.index)?;
        self.pending = 0;
        self.last_commit = Instant::now();
        let dirty_count = self
            .coordinator
            .acknowledge(std::mem::take(&mut self.committed_revisions))?;
        let documents =
            crate::indexing::index::item_count(&self.reader).map_err(|error| error.to_string())?;
        self.coordinator.update_status(|current| {
            current.pending = dirty_count;
            current.documents = documents;
            current.last_committed_at = Some(now_ms());
            current.state = if dirty_count == 0 {
                "ready".to_owned()
            } else {
                "reconciling".to_owned()
            };
        })
    }

    fn release_idle(&mut self) -> Result<(), String> {
        // Publication never waits for merges. Release writer pools after a quiet
        // period, or on shutdown, instead of creating and joining them per save.
        if self.pending == 0
            && self.last_commit.elapsed() >= Duration::from_secs(5)
            && let Some(writer) = self.writer.take()
        {
            writer
                .wait_merging_threads()
                .map_err(|error| error.to_string())?;
        }
        let last_used = self.coordinator.last_read()?;
        if self.pending == 0
            && self.writer.is_none()
            && last_used > self.last_evicted
            && last_used.elapsed() >= Duration::from_secs(5)
        {
            // In-flight queries retain their snapshots. New readers drop
            // decoded column and block caches without changing index data.
            match refresh_reader(&self.index, &self.index_path) {
                Ok(reader) => {
                    self.coordinator.publish(IndexSnapshot {
                        index: self.index.clone(),
                        reader: reader.clone(),
                    })?;
                    self.reader = reader;
                    crate::memory::release_unused();
                    tracing::debug!(operation = "index_idle_cache_release");
                }
                Err(error) => {
                    tracing::warn!(operation = "index_idle_cache_release", error = %error);
                }
            }
            self.last_evicted = last_used;
        }
        Ok(())
    }
}
