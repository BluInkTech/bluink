// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

mod analytics;
mod bridge;
mod cache;
mod catalogue_writes;
mod commands;
mod exports;
#[cfg(test)]
mod tests;

pub use bridge::{ApplicationOptions, BluinkApplication};
mod clock;
pub(crate) mod error;
pub(crate) mod items;
pub(crate) mod mutations;
pub(crate) mod operation;
pub(crate) mod queries;
mod query_documents;
pub(crate) mod request;
mod settings;
mod snippets;
mod themes;
mod watch;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};

use self::cache::CacheLease;
use self::error::{ApplicationError, Result, internal_error};
use crate::indexing::service;

struct OwnedIndex {
    path: String,
    events: watch::Coordinator,
    _lease: CacheLease,
}

struct State {
    config: Option<Value>,
    index: Option<OwnedIndex>,
    phase: &'static str,
    ready: bool,
    error: Option<String>,
}

pub(crate) struct Application {
    pub(crate) options: ApplicationOptions,
    root: PathBuf,
    timezone: String,
    workspace_template: Value,
    state: Mutex<State>,
    lifecycle: Mutex<()>,
    operations: RwLock<()>,
    stopping: AtomicBool,
    pub(crate) http: crate::http::Runtime,
    exports: std::sync::Arc<crate::export::jobs::Manager>,
}

impl Application {
    pub(crate) fn project_root(&self) -> &std::path::Path {
        &self.root
    }

    /// Where browser-sized copies of image assets are kept: beside the search
    /// index and, like it, a projection that can be deleted and remade.
    pub(crate) fn image_cache(&self) -> Option<std::path::PathBuf> {
        let index = self.index_path().ok()?;
        Some(std::path::Path::new(&index).parent()?.join("images-v1"))
    }

    fn new(options: ApplicationOptions) -> Result<Self> {
        let root = PathBuf::from(&options.project_root);
        if !root.is_absolute() {
            return Err(ApplicationError::InvalidRequest(
                "Project folder must be an absolute path.".to_owned(),
            ));
        }
        let timezone = options
            .timezone
            .clone()
            .unwrap_or_else(|| iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned()));
        clock::timezone(&timezone)?;
        let workspace_template: Value = serde_json::from_slice(
            &std::fs::read(&options.workspace_template_path).map_err(internal_error)?,
        )
        .map_err(internal_error)?;
        crate::engine::project::workspace::validate_content(&workspace_template)
            .map_err(internal_error)?;
        Ok(Self {
            workspace_template,
            options,
            root,
            timezone,
            state: Mutex::new(State {
                config: None,
                index: None,
                phase: "Opening project",
                ready: false,
                error: None,
            }),
            lifecycle: Mutex::new(()),
            operations: RwLock::new(()),
            stopping: AtomicBool::new(false),
            http: crate::http::Runtime::default(),
            exports: std::sync::Arc::default(),
        })
    }

    fn state(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| internal_error("Application state lock was poisoned.".to_owned()))
    }

    pub(crate) fn check_running(&self) -> Result<()> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApplicationError::ShuttingDown);
        }
        Ok(())
    }

    pub(crate) fn begin_operation(&self) -> Result<RwLockReadGuard<'_, ()>> {
        let _wait =
            crate::diagnostics::waits::Wait::new(crate::diagnostics::waits::Kind::Admission);
        self.check_running()?;
        let guard = self
            .operations
            .read()
            .map_err(crate::application::error::poison_error)?;
        self.check_running()?;
        if self.state()?.config.is_none() {
            return Err(ApplicationError::NotInitialized(
                "Initialize the project before using it.",
            ));
        }
        Ok(guard)
    }

    fn index_path(&self) -> Result<String> {
        self.state()?
            .index
            .as_ref()
            .map(|index| index.path.clone())
            .ok_or(ApplicationError::NotInitialized(
                "Initialize the search index before querying items.",
            ))
    }

    fn initialize_sync(&self, with_index: bool) -> Result<Value> {
        self.initialize_sync_deadline(with_index, None)
    }

    fn initialize_sync_deadline(
        &self,
        with_index: bool,
        deadline: Option<Instant>,
    ) -> Result<Value> {
        let _span = tracing::info_span!("application.initialize", with_index).entered();
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| internal_error("Application lifecycle lock was poisoned.".to_owned()))?;
        let _operations = self
            .operations
            .write()
            .map_err(crate::application::error::poison_error)?;
        self.check_running()?;
        let result = self.open_sync(with_index, deadline);
        if let Err(error) = &result {
            self.release_index()?;
            let mut state = self.state()?;
            state.ready = false;
            state.error = Some(error.to_string());
            state.phase = "Project initialization failed";
        }
        result
    }
    fn open_sync(&self, with_index: bool, deadline: Option<Instant>) -> Result<Value> {
        if !with_index {
            use crate::engine::project::{io::LockMode, transaction};
            let _mutation =
                transaction::lock(&self.root, LockMode::Exclusive).map_err(internal_error)?;
            transaction::recover(&self.root).map_err(internal_error)?;
        }
        let config = crate::engine::project::config::open_or_create_project_with_workspace(
            &self.root,
            &self.timezone,
            &self.workspace_template,
        )
        .map_err(internal_error)?;
        let zone = config["analytics"]["timezone"].as_str().ok_or_else(|| {
            ApplicationError::InvalidRequest("Project timezone is missing.".to_owned())
        })?;
        clock::timezone(zone)?;
        self.state()?.config = Some(config.clone());
        if with_index && self.state()?.index.is_none() {
            self.state()?.phase = "Opening search indexes";
            let id = config["id"].as_str().ok_or_else(|| {
                ApplicationError::InvalidRequest("Project ID is missing.".to_owned())
            })?;
            let path = cache::resolve(&self.root, id, self.options.cache_location.as_deref())?;
            let lease = CacheLease::acquire(&path)?;
            {
                use crate::engine::project::{io::LockMode, transaction};
                let _mutation =
                    transaction::lock(&self.root, LockMode::Exclusive).map_err(internal_error)?;
                transaction::recover(&self.root).map_err(internal_error)?;
            }
            let path = path
                .to_str()
                .ok_or_else(|| {
                    ApplicationError::InvalidRequest("Cache path must be valid Unicode.".to_owned())
                })?
                .to_owned();
            service::start_index_service_typed(self.options.project_root.clone(), &path)?;
            let events = match watch::Coordinator::start(self.root.clone(), path.clone()) {
                Ok(events) => events,
                Err(error) => {
                    let _ = service::shutdown_index_service(path);
                    return Err(error);
                }
            };
            self.state()?.index = Some(OwnedIndex {
                path: path.clone(),
                events,
                _lease: lease,
            });
            let readiness_span =
                tracing::info_span!(target: "bluink.index", "index.readiness").entered();
            let readiness_wait =
                crate::diagnostics::waits::Wait::new(crate::diagnostics::waits::Kind::Readiness);
            loop {
                self.check_running()?;
                let status = service::index_service_status_typed(&path)?;
                match status.state.as_str() {
                    "ready" => break,
                    "error" => {
                        return Err(internal_error(status.error.unwrap_or_else(|| {
                            "Search index reconciliation failed.".to_owned()
                        })));
                    }
                    _ => {
                        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            return Err(error::invalid_request(
                                "Publication indexing exceeded five minutes.",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
            }
            drop(readiness_wait);
            drop(readiness_span);
            self.state()?.phase = "Reconciling writing activity";
            crate::engine::project::activity::reconcile_activity(self.options.project_root.clone())
                .map_err(internal_error)?;
        }
        self.check_running()?;
        crate::memory::release_unused();
        let mut state = self.state()?;
        state.ready = true;
        state.error = None;
        state.phase = "Project ready";
        Ok(json!({
            "projectRoot": self.root,
            "config": config,
            "indexPath": state.index.as_ref().map(|index| &index.path),
        }))
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep application state guarded throughout admission checks and related operations."
    )]
    pub(crate) fn status(&self) -> Result<Value> {
        let state = self.state()?;
        let index = if let Some(index) = &state.index {
            Some(service::index_service_status_typed(&index.path)?)
        } else {
            None
        };
        Ok(json!({
            "ready": state.ready,
            "stopping": self.stopping.load(Ordering::Acquire),
            "phase": state.phase,
            "error": state.error,
            "watcherError": state.index.as_ref().and_then(|index| index.events.failure()),
            "index": index,
        }))
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep application state guarded throughout admission checks and related operations."
    )]
    pub(crate) fn activity_date(&self) -> Result<String> {
        let state = self.state()?;
        let zone = state
            .config
            .as_ref()
            .and_then(|config| config["analytics"]["timezone"].as_str())
            .ok_or(ApplicationError::NotInitialized(
                "Initialize the project first.",
            ))?;
        Ok(clock::activity_date(Utc::now(), clock::timezone(zone)?))
    }

    fn release_index(&self) -> Result<()> {
        let index = self.state()?.index.take();
        Self::stop_index(index)
    }

    fn stop_index(index: Option<OwnedIndex>) -> Result<()> {
        if let Some(mut index) = index {
            let events = index.events.stop();
            let service = service::shutdown_index_service(index.path.clone());
            // The worker has joined before its cache lease is dropped.
            drop(index);
            events?;
            service?;
        }
        Ok(())
    }
    fn stop_sync(&self) -> Result<()> {
        let _span = tracing::info_span!("application.stop").entered();
        self.stopping.store(true, Ordering::Release);
        let (_lifecycle, lifecycle_result) =
            error::recover_for_cleanup(self.lifecycle.lock(), "Application lifecycle");
        let (_operations, operations_result) =
            error::recover_for_cleanup(self.operations.write(), "Application operations");
        let (mut state, state_result) =
            error::recover_for_cleanup(self.state.lock(), "Application state");
        let index = state.index.take();
        state.ready = false;
        state.phase = "Stopping";
        drop(state);
        self.exports.stop();
        // Attempt every component's cleanup before reporting the first failure.
        let index_result = Self::stop_index(index);
        let inventory_result =
            crate::engine::project::named_items::invalidate(&self.root).map_err(internal_error);
        let (mut state, final_state_result) =
            error::recover_for_cleanup(self.state.lock(), "Application state");
        state.phase = "Stopped";
        drop(state);
        lifecycle_result
            .and(operations_result)
            .and(state_result)
            .and(index_result)
            .and(inventory_result)
            .and(final_state_result)
    }
}

impl Drop for Application {
    fn drop(&mut self) {
        if let Err(error) = self.stop_sync() {
            tracing::error!(operation = "application.drop", error = %error, "Application cleanup failed");
        }
    }
}
