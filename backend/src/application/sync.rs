// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    Application,
    collections::EmptyRequest,
    error::{Result, internal_error, invalid_request},
};
use crate::versioning::sync::model::*;
use crate::versioning::{
    sync::auth::*,
    transport::{self, model::*},
};

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch consumes decoded requests."
)]
impl Application {
    pub(crate) fn generate_sync_key(&self, request: GenerateSyncKey) -> Result<GeneratedKey> {
        let _operation = self.begin_operation()?;
        self.history
            .generate_sync_key(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn read_history_download(&self, _: EmptyRequest) -> Result<HistoryDownload> {
        let _operation = self.begin_operation()?;
        self.history.history_download().map_err(invalid_request)
    }
    pub(crate) fn read_sync_auth(&self, request: SyncOrigin) -> Result<AuthStatus> {
        let _operation = self.begin_operation()?;
        self.history
            .read_sync_auth(&request.origin)
            .map_err(invalid_request)
    }
    pub(crate) fn configure_sync_auth(&self, request: ConfigureSyncAuth) -> Result<AuthStatus> {
        let _operation = self.begin_operation()?;
        self.history
            .configure_sync_auth(request)
            .map_err(invalid_request)
    }
    pub(crate) fn inspect_sync_host(&self, request: SyncOrigin) -> Result<HostIdentity> {
        let _operation = self.begin_operation()?;
        transport::inspect_host(
            &transport::Remote::parse(&request.origin).map_err(invalid_request)?,
        )
        .map_err(invalid_request)
    }
    pub(crate) fn trust_sync_host(&self, request: TrustSyncHost) -> Result<AuthStatus> {
        let _operation = self.begin_operation()?;
        self.history
            .trust_sync_host(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn read_sync_status(&self, _: EmptyRequest) -> Result<SyncStatus> {
        let _operation = self.begin_operation()?;
        self.with_decision_count(self.history.sync_status().map_err(internal_error)?)
    }
    fn with_decision_count(&self, mut status: SyncStatus) -> Result<SyncStatus> {
        status.decisions = self.decision_count()?;
        Ok(status)
    }
    pub(crate) fn configure_sync(&self, request: ConfigureSync) -> Result<SyncStatus> {
        let _operation = self.begin_operation()?;
        self.with_decision_count(
            self.history
                .configure_sync(&request)
                .map_err(invalid_request)?,
        )
    }
    pub(crate) fn download_history_assets(&self, _: EmptyRequest) -> Result<SyncStatus> {
        let _operation = self.begin_operation()?;
        self.with_decision_count(
            self.history
                .download_history_assets()
                .map_err(invalid_request)?,
        )
    }
    pub(crate) fn clone_project(&self, request: CloneProject) -> Result<ClonedProject> {
        let _operation = self.begin_operation()?;
        self.history
            .clone_project(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn sync_project(&self, _: EmptyRequest) -> Result<SyncStatus> {
        let _sync = self
            .history
            .sync_serial
            .try_lock()
            .map_err(|_| invalid_request("Another sync is already running"))?;
        let result = self.sync_attempts();
        self.history
            .finish_sync(result.err())
            .map_err(internal_error)?;
        self.read_sync_status(EmptyRequest {})
    }
    fn sync_attempts(&self) -> crate::versioning::Result<()> {
        for attempt in 0..3 {
            let prepared = {
                let _operation = self.begin_operation().map_err(|e| e.to_string())?;
                self.history.prepare_sync()?
            };
            let accepted = {
                let _operation = self.begin_operation().map_err(|e| e.to_string())?;
                self.history.apply_sync(&prepared)
            };
            let result = accepted.and_then(|()| {
                let _operation = self.begin_operation().map_err(|e| e.to_string())?;
                self.history.push_sync(&prepared)
            });
            match result {
                Ok(()) => return Ok(()),
                // A save landed after preparation, or the destination moved:
                // checkpoint, fetch and merge again.
                Err(error)
                    if attempt < 2 && matches!(error.state, "push_rejected" | "local_changed") => {}
                Err(error) => return Err(error),
            }
        }
        Err(
            "The destination keeps changing. Your accepted changes are preserved; retry sync."
                .to_owned()
                .into(),
        )
    }
    pub(super) fn accept_sync(&self, prepared: &crate::versioning::sync::Prepared) -> Result<()> {
        let _operation = self.begin_operation()?;
        self.history.apply_sync(prepared).map_err(invalid_request)
    }
}
