// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::project::config;
use crate::engine::publication::{
    plan::PublicationFormat,
    theme::{self, ThemeSnapshot},
};
use serde::{Deserialize, Serialize};

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use super::items::{PublicItem, canonical_item};

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub(crate) struct SetDefaultWorkspaceRequest {
    id: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoveWorkspaceRequest {
    id: String,
    version: String,
    replacement_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceContext {
    project_id: String,
    default_workspace: PublicItem,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadThemeRequest {
    theme_id: String,
    format: String,
    overrides: Option<Box<crate::engine::publication::theme_format::Overrides>>,
}

impl Application {
    pub(crate) fn workspace_context(&self) -> Result<WorkspaceContext> {
        let _operation = self.begin_operation()?;
        self.workspace_context_snapshot()
    }
    fn workspace_context_snapshot(&self) -> Result<WorkspaceContext> {
        let (project_id, workspace) =
            config::read_workspace_context(&self.root).map_err(internal_error)?;
        Ok(WorkspaceContext {
            project_id,
            default_workspace: canonical_item(workspace, true)?,
        })
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "RPC dispatch owns its decoded request."
    )]
    pub(crate) fn remove_workspace(
        &self,
        request: RemoveWorkspaceRequest,
    ) -> Result<WorkspaceContext> {
        let _operation = self.begin_operation()?;
        let item = crate::engine::project::item::read_project_item(&self.root, &request.id)?
            .ok_or_else(|| {
                invalid_request("This workspace is unavailable. Refresh the workspace list.")
            })?;
        if item["meta"]["type"] != "workspace" || item["meta"]["state"] != "active" {
            return Err(invalid_request("Choose an active workspace to remove."));
        }
        if item["revision"].as_str() != Some(&request.version) {
            return Err(invalid_request(
                "This workspace changed. Reopen the removal confirmation.",
            ));
        }
        let context = self.workspace_context_snapshot()?;
        if context.default_workspace.id == request.id {
            let replacement = request.replacement_id.as_deref().filter(|id| *id != request.id)
				.ok_or_else(|| invalid_request("Choose another default workspace before removing this one. Create a workspace first if this is the last one."))?;
            // Move the default first: even if archiving fails, the project always
            // points to an active workspace. The client refreshes context on failure.
            config::set_default_workspace(&self.root, replacement)?;
        }
        self.finish_mutation(
            crate::engine::project::item_mutations::archive_workspace(
                &self.root,
                &request.id,
                &request.version,
                &self.activity_date()?,
            ),
            false,
        )?;
        self.workspace_context_snapshot()
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "RPC dispatch owns its decoded request."
    )]
    pub(crate) fn set_default_workspace(
        &self,
        request: SetDefaultWorkspaceRequest,
    ) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        canonical_item(
            config::set_default_workspace(&self.root, &request.id)?,
            true,
        )
    }
    pub(crate) fn read_theme(&self, request: ReadThemeRequest) -> Result<ThemeSnapshot> {
        let _operation = self.begin_operation()?;
        let format = PublicationFormat::parse(&request.format)
            .map_err(|_| invalid_request("Choose PDF, Word, HTML, or site as the theme target."))?;
        let mut snapshot =
            theme::capture_theme(&self.root, &request.theme_id, format).map_err(internal_error)?;
        if let Some(overrides) = request.overrides {
            theme::apply_theme_overrides(&mut snapshot, &overrides).map_err(invalid_request)?;
        }
        Ok(snapshot)
    }
}
