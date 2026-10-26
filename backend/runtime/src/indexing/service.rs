// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, hash_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::application::error::Result;
use crossbeam_channel::unbounded;

use crate::application::error::internal_error;
use crate::indexing::coordinator::{IndexCoordinator, MainSnapshot, QueryCache};
use crate::indexing::index::index_is_cold;
use crate::indexing::lifecycle::ServiceSlot;
use crate::indexing::state::{Service, ServiceHandle, ServiceStatus};
use crate::indexing::worker::service_worker;

static SERVICES: OnceLock<Mutex<HashMap<String, Arc<ServiceSlot>>>> = OnceLock::new();

fn services() -> &'static Mutex<HashMap<String, Arc<ServiceSlot>>> {
    SERVICES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn service_handle(index_path: &str) -> Result<ServiceHandle> {
    let slot = services()
        .lock()
        .map_err(crate::application::error::poison_error)?
        .get(index_path)
        .cloned()
        .ok_or_else(|| internal_error("Tantivy index service has not been started"))?;
    slot.handle().map_err(internal_error)
}
fn coordinator(index_path: &str) -> Result<Arc<IndexCoordinator>> {
    let coordinator = service_handle(index_path)?.coordinator;
    coordinator
        .wait_until_reconciled()
        .map_err(internal_error)?;
    Ok(coordinator)
}

fn project_handles(root: &Path) -> Result<Vec<ServiceHandle>> {
    let root = root.canonicalize().map_err(internal_error)?;
    let candidates: Vec<_> = {
        let registered = services()
            .lock()
            .map_err(crate::application::error::poison_error)?;
        registered.values().cloned().collect()
    };
    candidates
        .into_iter()
        .filter(|slot| Path::new(&slot.project_root).canonicalize().ok().as_ref() == Some(&root))
        .map(|slot| slot.handle().map_err(internal_error))
        .collect()
}
/// Hold a canonical write in the overlay without scheduling it, for deterministic tests.
#[cfg(test)]
pub(crate) fn mark_pending_write(root: &Path, id: String, revision: String) -> Result<()> {
    let coordinator = project_handles(root)?
        .into_iter()
        .next()
        .ok_or_else(|| internal_error("Project index has not been started"))?;
    coordinator
        .coordinator
        .mark_dirty(id, revision)
        .map_err(internal_error)
}

/// Called after canonical storage locks have been released. Success means every
/// attached index has published this write (or a subsequent canonical revision).
pub(crate) fn mark_project_write(root: &Path, id: &str, revision: &str) -> Result<()> {
    publish_project_changes(root, &[(id.to_owned(), revision.to_owned())])
}

/// Publish all records affected by one canonical mutation in one index commit.
pub(crate) fn publish_project_changes(root: &Path, changes: &[(String, String)]) -> Result<()> {
    let _wait = crate::diagnostics::waits::Wait::new(crate::diagnostics::waits::Kind::Publication);
    let publish = || -> Result<()> {
        let mut completions = Vec::new();
        for handle in project_handles(root)? {
            completions.push(
                handle
                    .coordinator
                    .submit_mutation(&handle.sender, changes)
                    .map_err(internal_error)?,
            );
        }
        let deadline = std::time::Instant::now() + super::coordinator::PUBLICATION_TIMEOUT;
        for completion in completions {
            completion
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|error| {
                    internal_error(format!("Index publication did not complete: {error}"))
                })?
                .map_err(internal_error)?;
        }
        Ok(())
    };
    publish().map_err(|error| internal_error(format!(
        "Canonical changes were saved, but search publication failed: {error}. Read the item before retrying a write; recover the index before querying."
    )))
}

fn queue_item(handle: &ServiceHandle, id: String, revision: Option<String>) -> Result<()> {
    handle
        .coordinator
        .submit(&handle.sender, id, revision)
        .map_err(internal_error)
}
pub(crate) fn project_index(root: &Path) -> Result<Option<MainSnapshot>> {
    for handle in project_handles(root)? {
        handle
            .coordinator
            .wait_until_reconciled()
            .map_err(internal_error)?;
        if let Some(snapshot) = handle.coordinator.main_snapshot().map_err(internal_error)? {
            return Ok(Some(snapshot));
        }
    }
    Ok(None)
}

fn property_query_fields(project_root: &Path) -> Result<crate::query::schema::QuerySchema> {
    let metadata = crate::catalogue::read_metadata_fields(&project_root.to_string_lossy())?;
    Ok(crate::query::schema::QuerySchema::from_metadata(metadata))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn refresh_query_fields(index_path: String) -> Result<()> {
    refresh_schema(&service_handle(&index_path)?)
}

fn refresh_schema(handle: &ServiceHandle) -> Result<()> {
    let gate = handle
        .schema_refresh
        .lock()
        .map_err(crate::application::error::poison_error)?;
    let fields = property_query_fields(Path::new(&handle.project_root))?;
    let result = handle
        .coordinator
        .replace_query_fields(fields)
        .map_err(internal_error);
    drop(gate);
    result
}

/// A canonical property change affects every index of the same project.
pub(crate) fn refresh_project_query_fields(root: &Path) -> Result<()> {
    for handle in project_handles(root)? {
        refresh_schema(&handle)?;
    }
    Ok(())
}
pub(crate) fn service_index(index_path: &str) -> Result<MainSnapshot> {
    let coordinator = coordinator(index_path)?;
    coordinator
        .main_snapshot()
        .map_err(internal_error)?
        .ok_or_else(|| internal_error("Tantivy index service is still starting"))
}
pub(crate) fn project_usage_index(root: &Path) -> Result<MainSnapshot> {
    optional_project_usage_index(root)?.ok_or_else(usage_index_unavailable)
}

/// None means no attached runtime, never a partial or failed runtime baseline.
pub(crate) fn optional_project_usage_index(root: &Path) -> Result<Option<MainSnapshot>> {
    let handles = project_handles(root)?;
    if handles.is_empty() {
        return Ok(None);
    }
    for handle in handles {
        if let Some(snapshot) = handle
            .coordinator
            .usage_snapshot()
            .map_err(internal_error)?
        {
            return Ok(Some(snapshot));
        }
    }
    Err(usage_index_unavailable())
}

fn usage_index_unavailable() -> crate::application::error::ApplicationError {
    crate::application::error::invalid_request(
        "Initialize the search index and wait for reconciliation before checking removal relationships.",
    )
}

pub(crate) fn query_snapshot(index_path: &str) -> Result<MainSnapshot> {
    coordinator(index_path)?
        .query_snapshot()
        .map_err(internal_error)?
        .ok_or_else(|| internal_error("Tantivy index service is still starting"))
}

pub(crate) fn cached_index(index_path: &str) -> Result<(MainSnapshot, QueryCache)> {
    coordinator(index_path)?
        .cached_snapshot()
        .map_err(internal_error)?
        .ok_or_else(|| internal_error("Tantivy index service is still starting"))
}

pub(crate) fn planning_index(index_path: &str) -> Result<(MainSnapshot, QueryCache)> {
    coordinator(index_path)?
        .ready_cached_snapshot()
        .map_err(internal_error)?
        .ok_or_else(|| {
            crate::application::error::invalid_request(
                "The search index must finish reconciling before Publication planning.",
            )
        })
}

#[cfg(test)]
pub(crate) fn aggregation_cache(index_path: &str) -> Result<QueryCache> {
    coordinator(index_path)?.cache().map_err(internal_error)
}
/// Remove only this reservation: a failed older start must not erase a retry.
fn remove_slot(index_path: &str, slot: &Arc<ServiceSlot>) -> Result<()> {
    let mut registered = services()
        .lock()
        .map_err(crate::application::error::poison_error)?;
    if registered
        .get(index_path)
        .is_some_and(|current| Arc::ptr_eq(current, slot))
    {
        registered.remove(index_path);
    }
    drop(registered);
    Ok(())
}

#[tracing::instrument(level = "debug", skip_all)]
pub(crate) fn start_index_service_typed(
    project_root: String,
    index_path: &str,
) -> Result<ServiceStatus> {
    let (slot, initialize) = {
        let mut registered = services()
            .lock()
            .map_err(crate::application::error::poison_error)?;
        match registered.entry(index_path.to_owned()) {
            Entry::Occupied(entry) => (Arc::clone(entry.get()), false),
            Entry::Vacant(entry) => {
                let slot = Arc::new(ServiceSlot::new(project_root.clone()));
                entry.insert(Arc::clone(&slot));
                (slot, true)
            }
        }
    };
    if initialize {
        // Publish every initialization outcome so concurrent waiters cannot hang.
        // The worker has its own lifecycle; this covers only pre-registration setup.
        let result = std::panic::catch_unwind(|| initialize_service(project_root, index_path))
            .map_err(|_| "Search service initialization panicked".to_owned())
            .and_then(|result| result.map_err(|error| error.to_string()));
        let failed = result.is_err();
        slot.finish_start(result).map_err(internal_error)?;
        if failed {
            remove_slot(index_path, &slot)?;
        }
    }
    slot.handle()
        .map_err(internal_error)?
        .coordinator
        .status()
        .map_err(internal_error)
}

fn initialize_service(project_root: String, index_path: &str) -> Result<Service> {
    let cold = index_is_cold(Path::new(index_path));
    let (sender, receiver) = unbounded();
    let coordinator = Arc::new(IndexCoordinator::new(
        property_query_fields(Path::new(&project_root))?,
        cold,
    ));
    let worker_coordinator = Arc::clone(&coordinator);
    let observer: Arc<dyn crate::engine::project::usage::WriteObserver> = coordinator.clone();
    crate::engine::project::usage::observe(Path::new(&project_root), &observer)
        .map_err(internal_error)?;
    let thread_index_path = PathBuf::from(index_path);
    let worker_project_root = PathBuf::from(&project_root);
    let worker = thread::Builder::new()
        .name("bluink-tantivy".to_owned())
        .spawn(move || {
            service_worker(
                worker_project_root,
                thread_index_path,
                receiver,
                worker_coordinator,
            );
        })
        .map_err(internal_error)?;
    Ok(Service {
        worker: Some(worker),
        handle: ServiceHandle {
            project_root,
            sender,
            coordinator,
            schema_refresh: Arc::new(Mutex::new(())),
        },
    })
}
/// Queue an item changed outside this process. The worker reloads the
/// canonical document and avoids writes when its opaque revision is unchanged.
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn enqueue_external_index_item(index_path: String, id: String) -> Result<()> {
    queue_item(&service_handle(&index_path)?, id, None)
}
pub(crate) fn index_service_status_typed(index_path: &str) -> Result<ServiceStatus> {
    let coordinator = service_handle(index_path)?.coordinator;
    coordinator.status().map_err(internal_error)
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn shutdown_index_service(index_path: String) -> Result<()> {
    shutdown_registered(services(), &index_path)
}

fn shutdown_registered(
    registry: &Mutex<HashMap<String, Arc<ServiceSlot>>>,
    index_path: &str,
) -> Result<()> {
    let (slot, lookup_result) = {
        let (registered, result) =
            crate::application::error::recover_for_cleanup(registry.lock(), "Search registry");
        (registered.get(index_path).cloned(), result)
    };
    if let Some(slot) = slot {
        let result = slot.shutdown().map_err(internal_error);
        let (mut registered, removal_result) =
            crate::application::error::recover_for_cleanup(registry.lock(), "Search registry");
        let removed = if registered
            .get(index_path)
            .is_some_and(|current| Arc::ptr_eq(current, &slot))
        {
            registered.remove(index_path)
        } else {
            None
        };
        drop(registered);
        drop(removed);
        return lookup_result.and(result).and(removal_result);
    }
    lookup_result
}
#[cfg(test)]
mod property_tests {
    #[test]
    fn poisoned_registry_releases_its_slot_without_clearing_poison() {
        let slot = std::sync::Arc::new(super::ServiceSlot::new("project".into()));
        slot.finish_start(Err("Initialization failed".into()))
            .unwrap();
        let observer = std::sync::Arc::downgrade(&slot);
        let registry = std::sync::Mutex::new(std::collections::HashMap::from([(
            "index".to_owned(),
            slot,
        )]));
        std::thread::scope(|scope| {
            let registry = &registry;
            assert!(
                scope
                    .spawn(move || {
                        let _guard = registry.lock().unwrap();
                        panic!("Injected registry failure");
                    })
                    .join()
                    .is_err()
            );
        });
        assert!(super::shutdown_registered(&registry, "index").is_err());
        assert!(observer.upgrade().is_none());
        assert!(registry.is_poisoned());
        assert!(super::shutdown_registered(&registry, "index").is_err());
    }

    #[test]
    fn query_fields_reject_invalid_canonical_declarations() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::properties::initialize(root.path()).unwrap();
        crate::engine::project::properties::create_property(
            root.path(),
            serde_json::json!({"key":"rating","valueType":"number"}),
        )
        .unwrap();
        let id = crate::engine::project::named_items::id("property", "rating");
        let path = crate::engine::project::identity::ItemId::try_from(id.as_str())
            .unwrap()
            .path(root.path());
        let original = std::fs::read(&path).unwrap();
        for value_type in ["invalid", "Text"] {
            let mut item: serde_json::Value = serde_json::from_slice(&original).unwrap();
            item["content"]["valueType"] = serde_json::json!(value_type);
            std::fs::write(&path, item.to_string()).unwrap();
            assert!(super::property_query_fields(root.path()).is_err());
        }
        std::fs::write(&path, "invalid JSON").unwrap();
        assert!(super::property_query_fields(root.path()).is_err());
    }

    #[test]
    fn query_schema_loads_property_items_and_ignores_presentation_attributes() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::properties::initialize(root.path()).unwrap();
        crate::engine::project::properties::create_property(
            root.path(),
            serde_json::json!({"key":"valid_name","valueType":"text"}),
        )
        .unwrap();
        let id = crate::engine::project::named_items::id("property", "valid_name");
        let path = crate::engine::project::identity::ItemId::try_from(id.as_str())
            .unwrap()
            .path(root.path());
        let mut item: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        item["content"]["color"] = serde_json::json!("blue");
        std::fs::write(&path, item.to_string()).unwrap();
        let fields = super::property_query_fields(root.path()).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields.value_type("valid_name"),
            Some(crate::engine::project::types::MetadataFieldValueType::Text)
        );
    }
}
