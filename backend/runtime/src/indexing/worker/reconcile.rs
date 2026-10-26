// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::projections::Projections;
use super::{acquire_writer, publish_reader};
use crate::indexing::coordinator::{IndexCoordinator, IndexSnapshot};
use crate::indexing::index::{indexed_file_stamps, now_ms, open_or_create_index, open_reader};
use crate::indexing::source::{SourceDocument, SourceId};
use crate::indexing::state::ReconciliationTimings;
use crate::projection::FileStamp;
use crossbeam_channel::bounded;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;
use tantivy::{Index, IndexReader, IndexWriter};

pub(super) struct ReadyIndex {
    pub(super) index: Index,
    pub(super) reader: IndexReader,
}

pub(super) fn reconcile(
    root: &Path,
    index_path: &Path,
    coordinator: &IndexCoordinator,
) -> Result<ReadyIndex, String> {
    let _mutation = crate::engine::project::transaction::lock(
        root,
        crate::engine::project::io::LockMode::Shared,
    )
    .map_err(|error| error.to_string())?;
    let (index, cold) = open_or_create_index(index_path)?;
    let mut reader = open_reader(&index).map_err(|error| error.to_string())?;
    coordinator.publish(IndexSnapshot {
        index: index.clone(),
        reader: reader.clone(),
    })?;
    let mut timings = ReconciliationTimings::default();
    let projections = Projections::new(&index)?;
    let mut writer = None;
    coordinator.update_status(|current| {
        current.cold = cold;
        "reconciling".clone_into(&mut current.state);
        "Discovering project files".clone_into(&mut current.phase);
        current.processed = 0;
        current.total = 0;
        current.checked = 0;
        current.reconciliation_timings = Some(timings.clone());
    })?;
    let current_items = scan_current_items(root, coordinator, &mut timings)?;
    let checked = current_items.len();
    let plan = compare_stamps(current_items, &reader, cold, coordinator, &mut timings)?;
    let expected = plan.changed.len() + plan.removed.len();
    coordinator.update_status(|current| {
        current.phase = if cold {
            "Indexing project files".to_owned()
        } else {
            "Indexing changed files".to_owned()
        };
        current.processed = 0;
        current.total = expected;
        current.checked = checked;
        current.changed = expected;
        current.pending = expected;
        current.reconciliation_timings = Some(timings.clone());
    })?;
    if expected > 0 {
        let writer = acquire_writer(&mut writer, &index)?;
        apply_changes(root, &plan, &projections, writer, coordinator)?;
        writer.commit().map_err(|error| error.to_string())?;
    }
    drop(plan);
    // Readers remain ready. Writers and their worker pools only exist while
    // committing changes; a warm startup needs no writer at all.
    if let Some(writer) = writer.take() {
        coordinator.update_status(|current| {
            "Finishing index compaction".clone_into(&mut current.phase);
        })?;
        writer
            .wait_merging_threads()
            .map_err(|error| error.to_string())?;
        reader = publish_reader(coordinator, &index)?;
    }
    report_complete(coordinator, &reader, checked, expected, cold, &timings)?;
    crate::engine::project::transaction::acknowledge(root)?;
    Ok(ReadyIndex { index, reader })
}

fn scan_current_items(
    root: &Path,
    coordinator: &IndexCoordinator,
    timings: &mut ReconciliationTimings,
) -> Result<HashMap<SourceId, FileStamp>, String> {
    let started_walk = Instant::now();
    let candidates = crate::indexing::source::list(root)?;
    timings.walk_ms = started_walk.elapsed().as_secs_f64() * 1000.0;
    let checked = candidates.len();
    let started_stamps = Instant::now();
    coordinator.update_status(|current| {
        "Checking file stamps".clone_into(&mut current.phase);
        current.processed = 0;
        current.total = checked;
        current.checked = 0;
        current.reconciliation_timings = Some(timings.clone());
    })?;
    // Probe each batch with bounded workers, then publish progress once from the
    // coordinating thread. Unchanged file contents are never read or projected.
    let mut current_items = HashMap::new();
    let mut last_progress = Instant::now();
    let stamps = crate::indexing::source::file_stamps(root, &candidates, |processed, total| {
        if processed == total || last_progress.elapsed().as_millis() >= 50 {
            coordinator.update_status(|current| {
                current.processed = processed;
                current.checked = processed;
            })?;
            last_progress = Instant::now();
        }
        Ok(())
    })?;
    for (id, stamp) in candidates.into_iter().zip(stamps) {
        if let Some(stamp) = stamp {
            current_items.insert(id, stamp);
        }
    }
    timings.file_stamp_ms = started_stamps.elapsed().as_secs_f64() * 1000.0;
    Ok(current_items)
}

struct ReconciliationPlan {
    changed: Vec<(SourceId, FileStamp)>,
    removed: Vec<SourceId>,
}

fn compare_stamps(
    current_items: HashMap<SourceId, FileStamp>,
    reader: &IndexReader,
    cold: bool,
    coordinator: &IndexCoordinator,
    timings: &mut ReconciliationTimings,
) -> Result<ReconciliationPlan, String> {
    let checked = current_items.len();
    let started_indexed = Instant::now();
    let indexed = if cold {
        HashMap::new()
    } else {
        coordinator.update_status(|current| {
            "Loading cached index stamps".clone_into(&mut current.phase);
            current.processed = 0;
            current.total = usize::try_from(reader.searcher().num_docs()).unwrap_or(usize::MAX);
            current.checked = checked;
        })?;
        indexed_file_stamps(reader, |processed, total| {
            coordinator.update_status(|current| {
                current.processed = processed;
                current.total = total;
                current.checked = checked;
            })
        })?
    };
    timings.indexed_stamp_ms = started_indexed.elapsed().as_secs_f64() * 1000.0;
    let started_diff = Instant::now();
    let removed_ids = indexed
        .keys()
        .filter(|id| !current_items.contains_key(*id))
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let changed_items = current_items
        .into_iter()
        .filter(|(id, stamp)| indexed.get(id) != Some(&Some(*stamp)))
        .collect::<Vec<_>>();
    timings.diff_ms = started_diff.elapsed().as_secs_f64() * 1000.0;
    Ok(ReconciliationPlan {
        changed: changed_items,
        removed: removed_ids,
    })
}

fn apply_changes(
    root: &Path,
    plan: &ReconciliationPlan,
    projections: &Projections,
    writer: &mut IndexWriter,
    coordinator: &IndexCoordinator,
) -> Result<(), String> {
    let expected = plan.changed.len() + plan.removed.len();
    let (sender, projection_receiver) = bounded::<std::result::Result<SourceDocument, String>>(4);
    // Advisory cancellation only. The channel and scoped join synchronize results
    // and completion, so the flag does not need to order other memory accesses.
    let cancelled = AtomicBool::new(false);
    thread::scope(|scope| {
        let items = &plan.changed;
        let cancelled = &cancelled;
        scope.spawn(move || {
            let projected = crate::parallel::files(|| {
                items
                    .par_iter()
                    .for_each_with(sender.clone(), |sender, (id, stamp)| {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let _ = sender.send(id.read(root, *stamp));
                    });
                Ok(())
            });
            if let Err(error) = projected {
                let _ = sender.send(Err(error));
            }
        });
        let result = (|| {
            for processed in 0..plan.changed.len() {
                let document = projection_receiver
                    .recv()
                    .map_err(|error| format!("Item projection worker stopped: {error}"))??;
                projections.write(writer, document)?;
                if (processed + 1).is_multiple_of(super::COMMIT_BATCH_SIZE) {
                    writer.commit().map_err(|error| error.to_string())?;
                }
                coordinator.update_status(|current| {
                    current.processed = processed + 1;
                    current.pending = expected.saturating_sub(processed + 1);
                })?;
            }
            Ok::<(), String>(())
        })();
        cancelled.store(true, Ordering::Relaxed);
        // Unblock producers before the scope joins them on failure.
        drop(projection_receiver);
        result
    })?;
    for id in &plan.removed {
        projections.delete(writer, id.as_str());
    }
    Ok(())
}

fn report_complete(
    coordinator: &IndexCoordinator,
    reader: &IndexReader,
    checked: usize,
    expected: usize,
    cold: bool,
    timings: &ReconciliationTimings,
) -> Result<(), String> {
    let documents =
        crate::indexing::index::item_count(reader).map_err(|error| error.to_string())?;
    coordinator.invalidate_cache()?;
    coordinator.complete_reconciliation()?;
    coordinator.update_status(|current| {
        current.documents = documents;
        current.pending = 0;
        current.processed = expected;
        current.total = expected;
        current.phase = if expected == 0 {
            "Project cache is current".to_owned()
        } else {
            "Indexing complete".to_owned()
        };
        current.last_committed_at = Some(now_ms());
        "ready".clone_into(&mut current.state);
        current.reconciliation_timings = Some(timings.clone());
    })?;
    tracing::debug!(
        operation = "index_reconciliation",
        checked_items = checked,
        changed_items = expected,
        indexed_documents = reader.searcher().num_docs(),
        cold_build = cold,
        walk_ms = timings.walk_ms,
        file_stamp_ms = timings.file_stamp_ms,
        indexed_stamp_ms = timings.indexed_stamp_ms,
        diff_ms = timings.diff_ms,
        "index reconciliation completed"
    );
    Ok(())
}
