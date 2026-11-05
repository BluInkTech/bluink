// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    Application,
    collections::EmptyRequest,
    error::{Result, internal_error, invalid_request},
};
use crate::records::items::{PublicItem, canonical_item};
use crate::versioning::model::*;

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch consumes decoded requests at the application boundary."
)]
impl Application {
    pub(crate) fn historical_asset(
        &self,
        commit: &str,
        id: &str,
        filename: &str,
    ) -> Result<std::path::PathBuf> {
        self.history
            .historical_asset(commit, id, filename)
            .map_err(invalid_request)
    }
    pub(crate) fn history_status(&self, _: EmptyRequest) -> Result<HistoryStatus> {
        let _operation = self.begin_operation()?;
        self.history.status().map_err(internal_error)
    }
    pub(crate) fn checkpoint_history(&self, _: EmptyRequest) -> Result<HistoryStatus> {
        let _operation = self.begin_operation()?;
        self.history.checkpoint().map_err(internal_error)
    }
    pub(crate) fn read_history(&self, request: HistoryRequest) -> Result<HistoryPage> {
        let _operation = self.begin_operation()?;
        self.history.history(&request).map_err(invalid_request)
    }
    pub(crate) fn read_history_item(&self, request: HistoryItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        Ok(canonical_item(
            self.history
                .historical_item(&request)
                .map_err(invalid_request)?,
            true,
        )?)
    }
    pub(crate) fn create_named_version(
        &self,
        request: NamedVersionRequest,
    ) -> Result<NamedVersion> {
        let _operation = self.begin_operation()?;
        self.history
            .named_version(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn preview_history_restore(
        &self,
        request: HistoryPreviewRequest,
    ) -> Result<HistoryPreview> {
        let _operation = self.begin_operation()?;
        self.history
            .preview_restore(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn history_client_activity(
        &self,
        request: HistoryClientActivityRequest,
    ) -> Result<HistoryClientActivity> {
        let _operation = self.begin_operation()?;
        self.history
            .client_activity(&request)
            .map_err(invalid_request)
    }
    pub(crate) fn restore_history(&self, request: HistoryRestoreRequest) -> Result<HistoryStatus> {
        let _operation = self.begin_operation()?;
        self.history
            .apply_restore(&request)
            .map_err(invalid_request)?;
        self.history.status().map_err(internal_error)
    }
}
