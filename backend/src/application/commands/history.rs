// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! CLI options select existing versioning operations; their policies stay in
//! the same application methods and store used by the browser.
use super::{
    Application, CommandRequest, CommandResult, Result, internal_message, invalid_request,
};
use crate::application::collections::EmptyRequest;
use crate::versioning::{
    model::{HistoryPreview, HistoryPreviewRequest, HistoryRequest, HistoryRestoreRequest},
    sync::model::{CloneProject, ConfigureSync},
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewedRestore {
    #[serde(flatten)]
    preview: HistoryPreview,
    review_token: String,
}

impl ReviewedRestore {
    fn new(preview: HistoryPreview) -> Result<Self> {
        // The SQLite generation survives CLI restarts and binds confirmation to
        // the exact project state that was presented for review.
        let review_token =
            crate::domain::project::submissions::hash(&preview).map_err(internal_message)?;
        Ok(Self {
            preview,
            review_token,
        })
    }
}

impl Application {
    pub(super) fn command_history(&self, request: &CommandRequest) -> Result<CommandResult> {
        let item_id = request
            .string("item", "--item needs an item ID")?
            .map(str::to_owned);
        let before = request
            .string("before", "--before needs a checkpoint ID")?
            .map(str::to_owned);
        let limit = request
            .string("limit", "--limit needs a page size")?
            .map(|n| {
                n.parse::<usize>()
                    .map_err(|_| invalid_request("--limit needs a nonnegative whole number"))
            })
            .transpose()?;
        self.read_history(HistoryRequest {
            item_id,
            before,
            limit,
        })
        .map(CommandResult::History)
    }

    pub(super) fn command_restore(&self, request: &CommandRequest) -> Result<CommandResult> {
        let commit_id = request
            .required("checkpoint", "--checkpoint requires a history commit ID")?
            .to_owned();
        let item_id = request
            .string("item", "--item needs an item ID")?
            .map(str::to_owned);
        let confirm = request.flag("confirm")?;
        let token = request.string("review-token", "--review-token needs the reviewed token")?;
        if confirm != token.is_some() {
            return Err(invalid_request(
                "Use --confirm and --review-token together with the token from your preview.",
            ));
        }
        let reviewed =
            ReviewedRestore::new(self.preview_history_restore(HistoryPreviewRequest {
                commit_id: commit_id.clone(),
                item_id: item_id.clone(),
            })?)?;
        let Some(token) = token else {
            return Ok(CommandResult::RestorePreview(reviewed));
        };
        if reviewed.review_token != token {
            return Err(invalid_request(
                "Restore preview changed. Review a fresh preview before confirming.",
            ));
        }
        self.restore_history(HistoryRestoreRequest {
            commit_id,
            item_id,
            expected_head: reviewed.preview.expected_head,
            expected_generation: reviewed.preview.expected_generation,
        })
        .map(CommandResult::Restored)
    }

    pub(super) fn command_sync(&self, request: &CommandRequest) -> Result<CommandResult> {
        if request.values.contains_key("destination") {
            return Err(invalid_request("--destination is only for --open-from"));
        }
        let origin = request.string("origin", "--origin needs an absolute folder or Git URL")?;
        let create = request.flag("create")?;
        if create && origin.is_none() {
            return Err(invalid_request("--create requires --origin"));
        }
        let status = match (origin, request.flag("status")?, request.flag("disconnect")?) {
            (Some(origin), false, false) => self.configure_sync(ConfigureSync {
                origin: Some(origin.to_owned()),
                create: Some(create),
            })?,
            (None, true, false) => self.read_sync_status(EmptyRequest {})?,
            (None, false, true) => self.configure_sync(ConfigureSync {
                origin: None,
                create: None,
            })?,
            (None, false, false) => {
                let status = self.sync_project(EmptyRequest {})?;
                if let Some(error) = &status.error {
                    return Err(invalid_request(error.clone()));
                }
                status
            }
            _ => {
                return Err(invalid_request(
                    "Choose one sync action: --status, --origin, --disconnect or sync without options.",
                ));
            }
        };
        Ok(CommandResult::Sync(status))
    }

    pub(super) fn command_clone(&self, request: &CommandRequest) -> Result<CommandResult> {
        if request.flag("status")?
            || request.flag("create")?
            || request.flag("disconnect")?
            || request.values.contains_key("origin")
        {
            return Err(invalid_request(
                "--open-from cannot be combined with other sync actions",
            ));
        }
        let origin = request
            .required(
                "open-from",
                "--open-from needs an absolute folder or Git URL",
            )?
            .to_owned();
        let destination = request
            .path(request.required(
                "destination",
                "--open-from requires a new --destination folder",
            )?)
            .to_str()
            .ok_or_else(|| invalid_request("Destination path must be valid Unicode"))?
            .to_owned();
        self.history
            .clone_project(&CloneProject {
                origin,
                destination,
            })
            .map(CommandResult::Cloned)
            .map_err(invalid_request)
    }
}
