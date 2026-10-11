// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, bounded};
use rayon::prelude::*;
use tantivy::{Index, IndexReader, IndexWriter, Term};

use crate::index::{
    indexed_file_stamps, indexed_revision, now_ms, open_or_create_index, open_reader, open_writer,
    refresh_reader, set_status, write_document,
};
use crate::model::{ReconciliationTimings, SearchDocument, Work, WorkerState};
use crate::passages;
use crate::project::{FileStamp, ItemId, file_stamp, index_stamp, item_path, read_document};
use crate::tasks;

const COMMIT_BATCH_SIZE: usize = 512;
const COMMIT_INTERVAL: Duration = Duration::from_millis(250);

struct Writers {
    main: IndexWriter,
    passage: IndexWriter,
    task: IndexWriter,
}

impl Writers {
    fn acquire<'a>(
        slot: &'a mut Option<Self>,
        main: &Index,
        passage: &Index,
        task: &Index,
    ) -> Result<&'a mut Self, String> {
        if slot.is_none() {
            *slot = Some(Self {
                main: open_writer(main).map_err(|error| error.to_string())?,
                passage: open_writer(passage).map_err(|error| error.to_string())?,
                task: open_writer(task).map_err(|error| error.to_string())?,
            });
        }
        Ok(slot.as_mut().expect("Writers were acquired"))
    }

    fn commit(
        &mut self,
        main: &IndexReader,
        passage: &IndexReader,
        task: &IndexReader,
    ) -> Result<(), String> {
        self.passage.commit().map_err(|error| error.to_string())?;
        passage.reload().map_err(|error| error.to_string())?;
        self.task.commit().map_err(|error| error.to_string())?;
        task.reload().map_err(|error| error.to_string())?;
        self.main.commit().map_err(|error| error.to_string())?;
        main.reload().map_err(|error| error.to_string())
    }

    fn finish(
        self,
        main: &IndexReader,
        passage: &IndexReader,
        task: &IndexReader,
    ) -> Result<(), String> {
        self.passage
            .wait_merging_threads()
            .map_err(|error| error.to_string())?;
        self.task
            .wait_merging_threads()
            .map_err(|error| error.to_string())?;
        self.main
            .wait_merging_threads()
            .map_err(|error| error.to_string())?;
        passage.reload().map_err(|error| error.to_string())?;
        task.reload().map_err(|error| error.to_string())?;
        main.reload().map_err(|error| error.to_string())
    }
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The worker owns its receiver and lifetime until shutdown."
)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn service_worker(
    root: PathBuf,
    index_path: PathBuf,
    receiver: Receiver<Work>,
    worker: WorkerState,
) {
    let WorkerState {
        last_read,
        status,
        dirty,
        index: query_index,
        reader: query_reader,
        passage_index: query_passage_index,
        passage_reader: query_passage_reader,
        task_index: query_task_index,
        task_reader: query_task_reader,
        aggregation_cache,
    } = worker;
    let result = (|| -> std::result::Result<(), String> {
        let (index, cold) = open_or_create_index(&index_path)?;
        let mut reader = open_reader(&index).map_err(|error| error.to_string())?;
        *query_index.write().map_err(|error| error.to_string())? = Some(index.clone());
        *query_reader.write().map_err(|error| error.to_string())? = Some(reader.clone());
        let (passage_index, passage_cold) =
            passages::open_or_create(&passages::index_path(&index_path))?;
        let mut passage_reader = open_reader(&passage_index).map_err(|error| error.to_string())?;
        *query_passage_index
            .write()
            .map_err(|error| error.to_string())? = Some(passage_index.clone());
        *query_passage_reader
            .write()
            .map_err(|error| error.to_string())? = Some(passage_reader.clone());
        let (task_index, task_cold) = tasks::open_or_create(&tasks::index_path(&index_path))?;
        let mut task_reader = open_reader(&task_index).map_err(|error| error.to_string())?;
        *query_task_index
            .write()
            .map_err(|error| error.to_string())? = Some(task_index.clone());
        *query_task_reader
            .write()
            .map_err(|error| error.to_string())? = Some(task_reader.clone());
        let mut timings = ReconciliationTimings::default();
        let schema = index.schema();
        let mut writers = None;
        let passage_schema = passage_index.schema();
        let passage_item_id = passage_schema
            .get_field("item_id")
            .map_err(|error| error.to_string())?;
        let task_schema = task_index.schema();
        let task_note_id = task_schema
            .get_field("note_id")
            .map_err(|error| error.to_string())?;
        set_status(&status, |current| {
            current.cold = cold;
            "reconciling".clone_into(&mut current.state);
            "Discovering project files".clone_into(&mut current.phase);
            current.processed = 0;
            current.total = 0;
            current.checked = 0;
            current.reconciliation_timings = Some(timings.clone());
        });
        let started_walk = Instant::now();
        let candidates = crate::engine::project::item::list_project_item_ids(&root)?;
        timings.walk_ms = started_walk.elapsed().as_secs_f64() * 1000.0;
        let checked = candidates.len();
        let started_stamps = Instant::now();
        set_status(&status, |current| {
            "Checking file stamps".clone_into(&mut current.phase);
            current.processed = 0;
            current.total = checked;
            current.checked = 0;
            current.reconciliation_timings = Some(timings.clone());
        });
        let stamped = AtomicUsize::new(0);
        let last_progress_ms = AtomicU64::new(0);
        let current_items = crate::parallel::files(|| {
            candidates
                .into_par_iter()
                .map(|id| {
                    let stamp = FileStamp::read(&item_path(&root, &id))?;
                    let id = ItemId::parse(&id).ok_or_else(|| format!("Invalid item ID: {id}"))?;
                    let processed = stamped.fetch_add(1, Ordering::Relaxed) + 1;
                    let elapsed_ms =
                        u64::try_from(started_stamps.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let previous_progress_ms = last_progress_ms.load(Ordering::Relaxed);
                    let due = elapsed_ms.saturating_sub(previous_progress_ms) >= 50;
                    if processed == checked
                        || processed.is_multiple_of(256)
                        || (due
                            && last_progress_ms
                                .compare_exchange(
                                    previous_progress_ms,
                                    elapsed_ms,
                                    Ordering::Relaxed,
                                    Ordering::Relaxed,
                                )
                                .is_ok())
                    {
                        set_status(&status, |current| {
                            current.processed = processed;
                            current.checked = processed;
                        });
                    }
                    Ok(stamp.map(|stamp| (id, stamp)))
                })
                .collect::<std::result::Result<Vec<_>, String>>()
        })?
        .into_iter()
        .flatten()
        .collect::<HashMap<_, _>>();
        let checked = current_items.len();
        timings.file_stamp_ms = started_stamps.elapsed().as_secs_f64() * 1000.0;
        let started_indexed = Instant::now();
        let mut indexed = if cold {
            HashMap::new()
        } else {
            let status = &status;
            set_status(status, |current| {
                "Loading cached index stamps".clone_into(&mut current.phase);
                current.processed = 0;
                current.total = usize::try_from(reader.searcher().num_docs()).unwrap_or(usize::MAX);
                current.checked = checked;
            });
            indexed_file_stamps(&reader, "id", false, |processed, total| {
                set_status(status, |current| {
                    current.processed = processed;
                    current.total = total;
                    current.checked = checked;
                });
            })?
        };
        let passage_stamps = if passage_cold {
            HashMap::new()
        } else {
            indexed_file_stamps(&passage_reader, "item_id", true, |_, _| {})?
        };
        let task_stamps = if task_cold {
            HashMap::new()
        } else {
            indexed_file_stamps(&task_reader, "note_id", true, |_, _| {})?
        };
        timings.indexed_stamp_ms = started_indexed.elapsed().as_secs_f64() * 1000.0;
        let started_diff = Instant::now();
        let changed_items = current_items
            .into_iter()
            .filter(|(id, stamp)| {
                indexed.remove(id) != Some(Some(*stamp))
                    || passage_stamps.get(id) != Some(&Some(*stamp))
                    || task_stamps.get(id) != Some(&Some(*stamp))
            })
            .collect::<Vec<_>>();
        let removed_ids = indexed.into_keys().collect::<Vec<_>>();
        let expected = changed_items.len() + removed_ids.len();
        // Startup snapshots must not remain alive throughout the receive loop.
        drop(passage_stamps);
        drop(task_stamps);
        timings.diff_ms = started_diff.elapsed().as_secs_f64() * 1000.0;
        set_status(&status, |current| {
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
        });
        let id_field = schema.get_field("id").map_err(|error| error.to_string())?;
        if expected > 0 {
            let writers = Writers::acquire(&mut writers, &index, &passage_index, &task_index)?;
            let (sender, projection_receiver) =
                bounded::<std::result::Result<SearchDocument, String>>(4);
            let cancelled = AtomicBool::new(false);
            thread::scope(|scope| {
                let items = &changed_items;
                let root = &root;
                let cancelled = &cancelled;
                scope.spawn(move || {
                    let projected = crate::parallel::files(|| {
                        items
                            .par_iter()
                            .for_each_with(sender.clone(), |sender, (id, stamp)| {
                                if cancelled.load(Ordering::Relaxed) {
                                    return;
                                }
                                let _ = sender.send(read_document(
                                    &item_path(root, id.as_str()),
                                    stamp.indexed(),
                                ));
                            });
                        Ok(())
                    });
                    if let Err(error) = projected {
                        let _ = sender.send(Err(error));
                    }
                });
                let result = (|| {
                    for processed in 0..changed_items.len() {
                        let document = projection_receiver.recv().map_err(|error| {
                            format!("Item projection worker stopped: {error}")
                        })??;
                        passages::write_document(&mut writers.passage, &passage_schema, &document)
                            .map_err(|error| error.to_string())?;
                        tasks::write_document(&mut writers.task, &task_schema, &document)
                            .map_err(|error| error.to_string())?;
                        write_document(&mut writers.main, &schema, document)
                            .map_err(|error| error.to_string())?;
                        set_status(&status, |current| {
                            current.processed = processed + 1;
                            current.pending = expected.saturating_sub(processed + 1);
                        });
                    }
                    Ok::<(), String>(())
                })();
                cancelled.store(true, Ordering::Relaxed);
                // Unblock producers before the scope joins them on failure.
                drop(projection_receiver);
                result
            })?;
            for id in removed_ids {
                let id = id.as_str();
                writers
                    .main
                    .delete_term(Term::from_field_text(id_field, id));
                writers
                    .passage
                    .delete_term(Term::from_field_text(passage_item_id, id));
                writers
                    .task
                    .delete_term(Term::from_field_text(task_note_id, id));
            }
            writers.commit(&reader, &passage_reader, &task_reader)?;
        }
        drop(changed_items);
        // Readers remain ready. Writers and their worker pools only exist while
        // committing changes; a warm startup needs no writers at all.
        if let Some(writers) = writers.take() {
            set_status(&status, |current| {
                "Finishing index compaction".clone_into(&mut current.phase);
            });
            writers.finish(&reader, &passage_reader, &task_reader)?;
        }
        aggregation_cache
            .lock()
            .map_err(|error| error.to_string())?
            .clear();
        set_status(&status, |current| {
            current.documents = usize::try_from(reader.searcher().num_docs()).unwrap_or(usize::MAX);
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
        });
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
        let mut pending = 0usize;
        let mut committed_revisions = Vec::<(String, String)>::new();
        let mut last_commit = Instant::now();
        let mut last_evicted = Instant::now();
        loop {
            let work = receiver.recv_timeout(Duration::from_millis(50)).ok();
            match work {
                Some(Work::Shutdown) => break,
                Some(Work::Upsert {
                    id,
                    expected_revision,
                }) => {
                    let path = item_path(&root, &id);
                    if let Ok(document) = file_stamp(&path)
                        .map(|stamp| index_stamp(&stamp))
                        .and_then(|stamp| read_document(&path, stamp))
                    {
                        let revision = document.revision.clone();
                        // File watchers can report metadata changes caused by a read.
                        // The opaque canonical revision is the authoritative signal;
                        // avoid rewriting the index and clearing result caches when it
                        // has not changed.
                        if indexed_revision(&reader, &schema, &id)
                            .map_err(|error| error.to_string())?
                            .as_deref()
                            == Some(revision.as_str())
                        {
                            if let Ok(mut values) = dirty.lock()
                                && values.get(&id).is_some_and(|pending| {
                                    pending == &revision
                                        || (expected_revision.is_none() && pending.is_empty())
                                })
                            {
                                values.remove(&id);
                            }
                            // A redundant scan event may be the final queued item
                            // after a commit. Clearing its dirty marker must also
                            // publish idle status, without committing again.
                            let dirty_count = dirty.lock().map_or(0, |values| values.len());
                            set_status(&status, |current| {
                                current.pending = pending.max(dirty_count);
                                current.state = if current.pending == 0 {
                                    "ready".to_owned()
                                } else {
                                    "reconciling".to_owned()
                                };
                            });
                            continue;
                        }
                        aggregation_cache
                            .lock()
                            .map_err(|error| error.to_string())?
                            .clear();
                        if expected_revision.is_none() {
                            dirty
                                .lock()
                                .map_err(|error| error.to_string())?
                                .insert(id.clone(), revision.clone());
                        }
                        let writers =
                            Writers::acquire(&mut writers, &index, &passage_index, &task_index)?;
                        passages::write_document(&mut writers.passage, &passage_schema, &document)
                            .map_err(|error| error.to_string())?;
                        tasks::write_document(&mut writers.task, &task_schema, &document)
                            .map_err(|error| error.to_string())?;
                        write_document(&mut writers.main, &schema, document)
                            .map_err(|error| error.to_string())?;
                        committed_revisions.push((id, revision));
                    } else {
                        if expected_revision.is_none() {
                            dirty
                                .lock()
                                .map_err(|error| error.to_string())?
                                .insert(id.clone(), String::new());
                        }
                        let writers =
                            Writers::acquire(&mut writers, &index, &passage_index, &task_index)?;
                        writers
                            .main
                            .delete_term(Term::from_field_text(id_field, &id));
                        writers
                            .passage
                            .delete_term(Term::from_field_text(passage_item_id, &id));
                        writers
                            .task
                            .delete_term(Term::from_field_text(task_note_id, &id));
                        committed_revisions.push((id, String::new()));
                    }
                    pending += 1;
                }
                None => {}
            }
            if pending >= COMMIT_BATCH_SIZE
                || (pending > 0 && last_commit.elapsed() >= COMMIT_INTERVAL)
            {
                writers
                    .as_mut()
                    .expect("Pending changes own writers")
                    .commit(&reader, &passage_reader, &task_reader)?;
                writers
                    .take()
                    .expect("Pending changes own writers")
                    .finish(&reader, &passage_reader, &task_reader)?;
                aggregation_cache
                    .lock()
                    .map_err(|error| error.to_string())?
                    .clear();
                pending = 0;
                last_commit = Instant::now();
                if let Ok(mut values) = dirty.lock() {
                    for (id, revision) in std::mem::take(&mut committed_revisions) {
                        if values.get(&id) == Some(&revision) {
                            values.remove(&id);
                        }
                    }
                }
                let dirty_count = dirty.lock().map_or(0, |values| values.len());
                set_status(&status, |current| {
                    current.pending = dirty_count;
                    current.documents =
                        usize::try_from(reader.searcher().num_docs()).unwrap_or(usize::MAX);
                    current.last_committed_at = Some(now_ms());
                    current.state = if dirty_count == 0 {
                        "ready".to_owned()
                    } else {
                        "reconciling".to_owned()
                    };
                });
            }
            let last_used = *last_read.lock().map_err(|error| error.to_string())?;
            if pending == 0
                && writers.is_none()
                && last_used > last_evicted
                && last_used.elapsed() >= Duration::from_secs(5)
            {
                // In-flight queries retain their snapshots. New readers drop
                // decoded column and block caches without changing index data.
                let fresh = (|| -> tantivy::Result<_> {
                    Ok((
                        refresh_reader(&index, &index_path)?,
                        refresh_reader(&passage_index, &passages::index_path(&index_path))?,
                        refresh_reader(&task_index, &tasks::index_path(&index_path))?,
                    ))
                })();
                match fresh {
                    Ok((main, passage, task)) => {
                        *query_reader.write().map_err(|error| error.to_string())? =
                            Some(main.clone());
                        *query_passage_reader
                            .write()
                            .map_err(|error| error.to_string())? = Some(passage.clone());
                        *query_task_reader
                            .write()
                            .map_err(|error| error.to_string())? = Some(task.clone());
                        reader = main;
                        passage_reader = passage;
                        task_reader = task;
                        aggregation_cache
                            .lock()
                            .map_err(|error| error.to_string())?
                            .clear();
                        crate::memory::release_unused();
                        tracing::debug!(operation = "index_idle_cache_release");
                    }
                    Err(error) => {
                        tracing::warn!(operation = "index_idle_cache_release", error = %error);
                    }
                }
                last_evicted = last_used;
            }
        }
        if pending > 0 {
            writers
                .as_mut()
                .expect("Pending changes own writers")
                .commit(&reader, &passage_reader, &task_reader)?;
            aggregation_cache
                .lock()
                .map_err(|error| error.to_string())?
                .clear();
            if let Ok(mut values) = dirty.lock() {
                for (id, revision) in std::mem::take(&mut committed_revisions) {
                    if values.get(&id) == Some(&revision) {
                        values.remove(&id);
                    }
                }
            }
        }
        if let Some(writers) = writers.take() {
            writers.finish(&reader, &passage_reader, &task_reader)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(
            error_class = "index_reconciliation_failed",
            "index reconciliation failed"
        );
        set_status(&status, |current| {
            "error".clone_into(&mut current.state);
            "Indexing failed".clone_into(&mut current.phase);
            current.error = Some(error);
        });
    }
}
