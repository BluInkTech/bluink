// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Instant;

use crate::application::error::Result;
use crossbeam_channel::unbounded;
use tantivy::{Index, IndexReader};

use crate::application::error::internal_error;
use crate::index::index_is_cold;
use crate::model::{
    AggregationCache, CachedResult, DirtyRevisions, Service, ServiceStatus, Work, WorkerState,
};
use crate::worker::service_worker;

static SERVICES: OnceLock<Mutex<HashMap<String, Service>>> = OnceLock::new();

fn services() -> &'static Mutex<HashMap<String, Service>> {
    SERVICES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mark_read(service: &Service) {
    if let Ok(mut last) = service.last_read.lock() {
        *last = Instant::now();
    }
}

/// Canonical writes own dirty overlays, cache invalidation and index notification.
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn mark_project_write(root: &Path, id: &str, revision: &str) -> Result<()> {
    let registered = services().lock().map_err(internal_error)?;
    let root = root.canonicalize().map_err(internal_error)?;
    for service in registered.values() {
        if Path::new(&service.project_root)
            .canonicalize()
            .ok()
            .as_ref()
            == Some(&root)
        {
            queue_item(service, id.to_owned(), Some(revision.to_owned()))?;
        }
    }
    Ok(())
}

fn queue_item(service: &Service, id: String, revision: Option<String>) -> Result<()> {
    service
        .dirty
        .lock()
        .map_err(internal_error)?
        .insert(id.clone(), revision.clone().unwrap_or_default());
    service
        .aggregation_cache
        .lock()
        .map_err(internal_error)?
        .clear();
    service
        .sender
        .send(Work::Upsert {
            id,
            expected_revision: revision,
        })
        .map_err(internal_error)
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn project_index(root: &Path) -> Result<Option<(Index, IndexReader, DirtyRevisions)>> {
    let registered = services().lock().map_err(internal_error)?;
    let root = root.canonicalize().map_err(internal_error)?;
    for service in registered.values() {
        if Path::new(&service.project_root)
            .canonicalize()
            .ok()
            .as_ref()
            != Some(&root)
        {
            continue;
        }
        let index = service.index.read().map_err(internal_error)?.clone();
        let reader = service.reader.read().map_err(internal_error)?.clone();
        if let (Some(index), Some(reader)) = (index, reader) {
            mark_read(service);
            return Ok(Some((index, reader, Arc::clone(&service.dirty))));
        }
    }
    Ok(None)
}

fn property_query_fields(project_root: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(project_root.join("properties")) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".property").map(str::to_owned))
        .filter_map(|name| name.rsplit_once('.').map(|(key, _)| key.to_owned()))
        .filter(|key| {
            !key.is_empty()
                && key.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
        .collect()
}

pub(crate) fn query_fields(index_path: &str) -> Result<HashSet<String>> {
    let registered = services().lock().map_err(internal_error)?;
    registered
        .get(index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?
        .query_fields
        .read()
        .map(|fields| fields.clone())
        .map_err(internal_error)
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn refresh_query_fields(index_path: String) -> Result<()> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(&index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    let refreshed = property_query_fields(Path::new(&service.project_root));
    *service.query_fields.write().map_err(internal_error)? = refreshed;
    service
        .aggregation_cache
        .lock()
        .map_err(internal_error)?
        .clear();
    Ok(())
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn service_index(index_path: &str) -> Result<(Index, IndexReader, DirtyRevisions)> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    mark_read(service);
    let index = service
        .index
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Tantivy index service is still starting"))?;
    let reader = service
        .reader
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Tantivy index service is still starting"))?;
    Ok((index, reader, Arc::clone(&service.dirty)))
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn service_passage_index(index_path: &str) -> Result<(Index, IndexReader)> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    mark_read(service);
    let index = service
        .passage_index
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Passage index is still starting"))?;
    let reader = service
        .passage_reader
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Passage index is still starting"))?;
    Ok((index, reader))
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn service_task_index(index_path: &str) -> Result<(Index, IndexReader)> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    mark_read(service);
    let index = service
        .task_index
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Task index is still starting"))?;
    let reader = service
        .task_reader
        .read()
        .map_err(internal_error)?
        .clone()
        .ok_or_else(|| internal_error("Task index is still starting"))?;
    Ok((index, reader))
}

pub(crate) fn aggregation_cache(index_path: &str) -> Result<AggregationCache> {
    let registered = services().lock().map_err(internal_error)?;
    registered
        .get(index_path)
        .map(|service| Arc::clone(&service.aggregation_cache))
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))
}

// These are committed-index results, never canonical state. Keep the cache
// deliberately small: catalogue and facet responses can be large, and it is
// cleared before a changed item can be visible to callers.
pub(crate) fn cache_result(
    cache: &Mutex<HashMap<String, CachedResult>>,
    key: String,
    value: CachedResult,
) {
    if let Ok(mut values) = cache.lock() {
        if values.len() >= 16
            && let Some(evicted) = values.keys().next().cloned()
        {
            values.remove(&evicted);
        }
        values.insert(key, value);
    }
}

#[tracing::instrument(level = "debug", skip_all)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn start_index_service_typed(
    project_root: String,
    index_path: String,
) -> Result<ServiceStatus> {
    let mut registered = services().lock().map_err(internal_error)?;
    if let Some(service) = registered.get(&index_path) {
        let status = service.status.lock().map_err(internal_error)?.clone();
        return Ok(status);
    }
    let cold = index_is_cold(Path::new(&index_path));
    let (sender, receiver) = unbounded();
    let status = Arc::new(Mutex::new(ServiceStatus {
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
        schema_fingerprint: "bluink-v15-typed-document-relations".to_owned(),
        reconciliation_timings: None,
    }));
    let dirty = Arc::new(Mutex::new(HashMap::new()));
    let query_index = Arc::new(RwLock::new(None));
    let query_reader = Arc::new(RwLock::new(None));
    let passage_index = Arc::new(RwLock::new(None));
    let passage_reader = Arc::new(RwLock::new(None));
    let task_index = Arc::new(RwLock::new(None));
    let task_reader = Arc::new(RwLock::new(None));
    let aggregation_cache = Arc::new(Mutex::new(HashMap::new()));
    let query_fields = Arc::new(RwLock::new(property_query_fields(Path::new(&project_root))));
    let worker_status = Arc::clone(&status);
    let worker_dirty = Arc::clone(&dirty);
    let worker_index = Arc::clone(&query_index);
    let worker_reader = Arc::clone(&query_reader);
    let worker_passage_index = Arc::clone(&passage_index);
    let worker_passage_reader = Arc::clone(&passage_reader);
    let worker_task_index = Arc::clone(&task_index);
    let worker_task_reader = Arc::clone(&task_reader);
    let worker_aggregation_cache = Arc::clone(&aggregation_cache);
    let last_read = Arc::new(Mutex::new(Instant::now()));
    let worker_last_read = Arc::clone(&last_read);
    let thread_index_path = index_path.clone();
    let worker_project_root = project_root.clone();
    let worker = thread::Builder::new()
        .name("bluink-tantivy".to_owned())
        .spawn(move || {
            service_worker(
                PathBuf::from(worker_project_root),
                PathBuf::from(thread_index_path),
                receiver,
                WorkerState {
                    last_read: worker_last_read,
                    status: worker_status,
                    dirty: worker_dirty,
                    index: worker_index,
                    reader: worker_reader,
                    passage_index: worker_passage_index,
                    passage_reader: worker_passage_reader,
                    task_index: worker_task_index,
                    task_reader: worker_task_reader,
                    aggregation_cache: worker_aggregation_cache,
                },
            );
        })
        .map_err(internal_error)?;
    registered.insert(
        index_path,
        Service {
            last_read,
            worker: Some(worker),
            project_root,
            query_fields,
            sender,
            status: Arc::clone(&status),
            dirty,
            index: query_index,
            reader: query_reader,
            passage_index,
            passage_reader,
            task_index,
            task_reader,
            aggregation_cache,
        },
    );
    let status = status.lock().map_err(internal_error)?.clone();
    Ok(status)
}

/// Queue an item changed outside this process. The worker reloads the
/// canonical document and avoids writes when its opaque revision is unchanged.
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn enqueue_external_index_item(index_path: String, id: String) -> Result<()> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(&index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    queue_item(service, id, None)
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the service registered throughout operations that use its index and state."
)]
pub(crate) fn index_service_status_typed(index_path: &str) -> Result<ServiceStatus> {
    let registered = services().lock().map_err(internal_error)?;
    let service = registered
        .get(index_path)
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    let mut status = service.status.lock().map_err(internal_error)?.clone();
    // Queued writes enter the dirty overlay before the worker publishes its
    // next status. The overlay stays dirty until every related reader reloads.
    let accepted = service.dirty.lock().map_err(internal_error)?.len();
    status.pending = status.pending.max(accepted);
    if accepted > 0 && status.state == "ready" {
        "reconciling".clone_into(&mut status.state);
        "Indexing changed files".clone_into(&mut status.phase);
    }
    Ok(status)
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn shutdown_index_service(index_path: String) -> Result<()> {
    let removed = services()
        .lock()
        .map_err(internal_error)?
        .remove(&index_path);
    if let Some(mut service) = removed {
        // A failed startup can already have stopped the worker. Shutdown is
        // cleanup in that case and must not mask the original index error.
        let _ = service.sender.send(Work::Shutdown);
        if let Some(worker) = service.worker.take() {
            worker
                .join()
                .map_err(|_| internal_error("Search worker panicked during shutdown"))?;
        }
    }
    Ok(())
}
