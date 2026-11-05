// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

mod analytics;
mod automatic_sync;
pub(crate) mod catalogue_writes;
pub(crate) mod changes;
pub(crate) mod commands;
pub(crate) mod drafts;
mod history;
mod publishing;
mod sync;
mod sync_decisions;
#[cfg(test)]
mod tests;

mod clock;
pub(crate) mod collections;
pub(crate) mod error;
pub(crate) mod item_reads;
pub(crate) mod item_writes;
pub(crate) mod operations;
mod query_documents;
pub(crate) mod request;
mod settings;
mod setups;
pub(crate) mod snippets;
mod themes;

use self::error::{ApplicationError, Result, internal_error, internal_message};
use chrono::Utc;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Mutex, MutexGuard, RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const OPENING_DATABASE: &str = "Opening project database";

#[derive(Clone)]
pub(crate) struct ApplicationOptions {
    pub(crate) project_root: String,
    /// Packaged, UI-authored workspace starter JSON.
    pub(crate) workspace_template_path: String,
    pub(crate) timezone: Option<String>,
    pub(crate) public_dir: Option<String>,
    /// Per-user local state, including this device's Git identity.
    pub(crate) user_data_dir: Option<String>,
    pub(crate) web_font_folder: Option<String>,
    pub(crate) pdf_font_folder: Option<String>,
    pub(crate) docx_font_folder: Option<String>,
    pub(crate) typst_binary: Option<String>,
}

struct State {
    database: Option<std::sync::Arc<crate::store::Store>>,
    phase: &'static str,
    history_files: Option<(usize, usize)>,
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
    exports: std::sync::Arc<crate::export_jobs::jobs::Manager>,
    history: std::sync::Arc<crate::versioning::History>,
    history_session: Mutex<Option<crate::versioning::HistorySession>>,
    automatic_sync: Mutex<Option<automatic_sync::Worker>>,
}

impl Application {
    pub(crate) fn project_root(&self) -> &std::path::Path {
        &self.root
    }

    /// Browser-sized assets are disposable; canonical assets live in Git's object store.
    pub(crate) fn image_cache(&self) -> PathBuf {
        self.root.join(".cache").join("images-v1")
    }

    pub(crate) fn database(&self) -> Result<std::sync::Arc<crate::store::Store>> {
        self.state()?
            .database
            .clone()
            .ok_or(ApplicationError::NotInitialized(
                "Initialize the project database first.",
            ))
    }

    pub(crate) fn project_timezone(&self) -> Result<chrono_tz::Tz> {
        let config = self.database()?.read(crate::store::project::read_config)?;
        clock::timezone(
            config["analytics"]["timezone"]
                .as_str()
                .ok_or_else(|| error::invalid_request("Project has no timezone."))?,
        )
    }

    pub(crate) fn new(options: ApplicationOptions) -> Result<Self> {
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
        .map_err(internal_message)?;
        crate::domain::project::workspace::validate_content(&workspace_template)
            .map_err(internal_message)?;
        crate::versioning::log_build_info();
        Ok(Self {
            history: crate::versioning::History::register(&root, options.user_data_dir.as_deref())
                .map_err(internal_error)?,
            history_session: Mutex::new(None),
            automatic_sync: Mutex::new(None),
            workspace_template,
            options,
            root,
            timezone,
            state: Mutex::new(State {
                database: None,
                phase: "Opening project",
                history_files: None,
                ready: false,
                error: None,
            }),
            lifecycle: Mutex::new(()),
            operations: RwLock::new(()),
            stopping: AtomicBool::new(false),
            exports: std::sync::Arc::default(),
        })
    }

    fn state(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| internal_message("Application state lock was poisoned.".to_owned()))
    }

    pub(crate) fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
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
        if !self.state()?.ready {
            return Err(ApplicationError::NotInitialized(
                "Initialize the project before using it.",
            ));
        }
        Ok(guard)
    }

    pub(crate) fn initialize_sync(&self) -> Result<()> {
        let _span = tracing::info_span!("application.initialize").entered();
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| internal_message("Application lifecycle lock was poisoned.".to_owned()))?;
        let _operations = self
            .operations
            .write()
            .map_err(crate::application::error::poison_error)?;
        self.check_running()?;
        let result = self.open_sync();
        if let Err(error) = &result {
            self.state()?.database = None;
            let mut state = self.state()?;
            state.ready = false;
            state.error = Some(error.to_string());
            state.phase = "Project initialization failed";
            state.history_files = None;
        }
        result
    }
    fn open_sync(&self) -> Result<()> {
        self.state()?.phase = OPENING_DATABASE;
        let (database, config) = crate::store::project::open_or_create(
            &self.root,
            &self.timezone,
            &self.workspace_template,
        )?;
        let zone = config["analytics"]["timezone"]
            .as_str()
            .ok_or_else(|| error::invalid_request("Project timezone is missing."))?;
        clock::timezone(zone)?;
        {
            let mut state = self.state()?;
            state.database = Some(database);
        }
        self.check_running()?;
        self.prepare_local_history()?;
        crate::allocator::release_unused();
        let mut state = self.state()?;
        state.ready = true;
        state.error = None;
        state.phase = "Project ready";
        state.history_files = None;
        drop(state);
        Ok(())
    }

    fn prepare_local_history(&self) -> Result<()> {
        self.state()?.phase = "Preparing local history";
        self.history
            .initialize_with_progress(&|phase, processed, total| {
                if let Ok(mut state) = self.state() {
                    state.phase = phase;
                    state.history_files = (total > 0).then_some((processed, total));
                }
            })
            .map_err(internal_error)?;
        let mut session = self.history_session.lock().map_err(error::poison_error)?;
        if session.is_none() {
            *session = Some(self.history.acquire_session().map_err(internal_error)?);
        }
        drop(session);
        Ok(())
    }
    pub(crate) fn status(&self) -> Result<Value> {
        let state = self.state()?;
        Ok(json!({
            "ready": state.ready,
            "stopping": self.stopping.load(Ordering::Acquire),
            "phase": state.phase,
            "progress": state.history_files.map(|(processed,total)| json!({"processed":processed,"total":total})),
            "error": state.error,
        }))
    }
    pub(crate) fn activity_date(&self) -> Result<String> {
        Ok(clock::activity_date(Utc::now(), self.project_timezone()?))
    }

    pub(crate) fn cancel_owned_sync(&self) {
        if self
            .history_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            self.history.cancel_sync();
        }
    }

    pub(crate) fn stop_sync(&self) -> Result<()> {
        let _span = tracing::info_span!("application.stop").entered();
        self.stopping.store(true, Ordering::Release);
        self.cancel_owned_sync();
        let automatic_sync_result = self.stop_automatic_sync();
        let (_lifecycle, lifecycle_result) =
            error::recover_for_cleanup(self.lifecycle.lock(), "Application lifecycle");
        let (_operations, operations_result) =
            error::recover_for_cleanup(self.operations.write(), "Application operations");
        let (mut state, state_result) =
            error::recover_for_cleanup(self.state.lock(), "Application state");
        if state.phase == "Stopped" {
            drop(state);
            return lifecycle_result
                .and(automatic_sync_result)
                .and(operations_result)
                .and(state_result);
        }
        let database = state.database.take();
        state.ready = false;
        state.phase = "Stopping";
        drop(state);
        self.exports.stop();
        let (mut session, history_session_result) =
            error::recover_for_cleanup(self.history_session.lock(), "Application history session");
        let lease = session.take();
        drop(session);
        let history_result = lease.map_or(Ok(()), |mut session| {
            session.close().map_err(internal_error)
        });
        // Attempt every component's cleanup before reporting the first failure.
        drop(database);
        let (mut state, final_state_result) =
            error::recover_for_cleanup(self.state.lock(), "Application state");
        state.phase = "Stopped";
        drop(state);
        lifecycle_result
            .and(automatic_sync_result)
            .and(operations_result)
            .and(state_result)
            .and(history_result)
            .and(history_session_result)
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
