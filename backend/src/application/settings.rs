// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    Application,
    error::{Result, internal_message, invalid_request},
};
use crate::domain::publication::{
    plan::PublicationFormat,
    theme::{self, ThemeSnapshot},
};
use crate::records::items::PublicItem;
use serde::{Deserialize, Serialize};

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
    overrides: Option<Box<crate::domain::publication::theme_format::Overrides>>,
}

impl Application {
    pub(crate) fn workspace_context(&self) -> Result<WorkspaceContext> {
        let _operation = self.begin_operation()?;
        self.workspace_context_snapshot()
    }
    fn workspace_context_snapshot(&self) -> Result<WorkspaceContext> {
        let (project_id, workspace) = self
            .database()?
            .read(crate::store::reads::items::workspace_context)?;
        Ok(WorkspaceContext {
            project_id,
            default_workspace: workspace,
        })
    }

    pub(crate) fn remove_workspace(
        &self,
        request: RemoveWorkspaceRequest,
    ) -> Result<WorkspaceContext> {
        let _operation = self.begin_operation()?;
        let store = self.database()?;
        let (item, mut config, config_seen) = store.read(|tx| {
            Ok((
                crate::store::item::read(tx, &request.id, true)?
                    .ok_or(crate::store::Error::NotFound)?,
                crate::store::project::read_config(tx)?,
                config_revision(tx)?,
            ))
        })?;
        if item["meta"]["type"] != "workspace" || item["meta"]["state"] != "active" {
            return Err(invalid_request("Choose an active workspace to remove."));
        }
        if item["revision"].as_str() != Some(request.version.as_str()) {
            return Err(invalid_request(
                "This workspace changed. Reopen the removal confirmation.",
            ));
        }
        let replacement = if config["defaultWorkspaceId"] == request.id {
            Some(
                request
                    .replacement_id
                    .clone()
                    .filter(|id| *id != request.id)
                    .ok_or_else(|| {
                        invalid_request(
                            "Choose another default workspace before removing this one.",
                        )
                    })?,
            )
        } else {
            None
        };
        let mut records = Vec::new();
        if let Some(id) = &replacement {
            config["defaultWorkspaceId"] = serde_json::json!(id);
            records.push(("bluink.json".to_owned(), Some(config.clone())));
        }
        let next = crate::store::commands::assemble_item(
            &serde_json::json!({"id":request.id,"meta":{"state":"archived"}}),
            Some(&item),
            config["analytics"]["timezone"]
                .as_str()
                .ok_or_else(|| invalid_request("Project has no timezone"))?,
        )
        .map_err(invalid_request)?;
        let delta = crate::store::portable::PreparedDelta::new(
            vec![(next, Some(request.version))],
            Vec::new(),
            records,
            Vec::new(),
        )?;
        // The workspace carries its expected revision. Guard only what else this
        // change depends on, so unrelated saves cannot fail the removal.
        store.write(move |tx| {
            if let Some(id) = &replacement {
                if config_revision(tx)? != config_seen {
                    return Err(crate::store::Error::RevisionConflict);
                }
                let active: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND type='workspace' AND state='active')",
                    [id],
                    |row| row.get(0),
                )?;
                if !active {
                    return Err(crate::store::Error::Invalid(
                        "Choose an active workspace as the default.".to_owned(),
                    ));
                }
            }
            let generation = tx.query_row::<i64, _, _>(
                "SELECT generation FROM project_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            delta.apply(tx, generation)
        })?;
        self.workspace_context_snapshot()
    }

    pub(crate) fn read_theme(&self, request: ReadThemeRequest) -> Result<ThemeSnapshot> {
        let _operation = self.begin_operation()?;
        let format = PublicationFormat::parse(&request.format)
            .map_err(|_| invalid_request("Choose PDF, Word, HTML, or site as the theme target."))?;
        let mut snapshot =
            crate::export_jobs::theme::capture_theme(&self.root, &request.theme_id, format)
                .map_err(internal_message)?;
        if let Some(overrides) = request.overrides {
            theme::apply_theme_overrides(&mut snapshot, &overrides).map_err(invalid_request)?;
        }
        Ok(snapshot)
    }
}

fn config_revision(tx: &rusqlite::Transaction<'_>) -> crate::store::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(tx
        .query_row(
            "SELECT revision FROM project_record WHERE path='bluink.json'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}
