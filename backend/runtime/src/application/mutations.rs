// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::indexing::coordinator::MainSnapshot;
use std::path::Path;
use std::time::Instant;

use crate::engine::project::write::MutationError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use super::items::{PublicItem, ReadOptions, canonical_item};
use crate::indexing::service;
use crate::query;

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreateItemRequest {
    #[serde(default)]
    #[cfg_attr(
        test,
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>>",
            optional
        )
    )]
    meta: Map<String, Value>,
    #[serde(default = "empty_content")]
    #[cfg_attr(
        test,
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>>",
            optional
        )
    )]
    content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pins: Option<Vec<String>>,
}

impl CreateItemRequest {
    pub(super) const fn new(meta: Map<String, Value>, content: Value) -> Self {
        Self {
            meta,
            content,
            tags: None,
            pins: None,
        }
    }
}

fn empty_content() -> Value {
    json!({})
}

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdateItemRequest {
    id: String,
    #[serde(skip_serializing)]
    version: String,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    meta: Map<String, Value>,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pins: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdateItemMetaRequest {
    id: String,
    version: String,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    meta: Map<String, Value>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    options: ReadOptions,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct SetItemTagsRequest {
    id: String,
    version: String,
    tags: Vec<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct MutateNoteTaskRequest {
    note_id: String,
    change: crate::engine::project::types::TaskChange,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct SetNotePinRequest {
    note_id: String,
    version: String,
    pinned: bool,
    scope_id: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PinResult {
    pinned: bool,
    version: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct DeleteItemRequest {
    id: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DeleteResult {
    deleted: bool,
}

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreateWritingGoalRequest {
    scope: crate::engine::project::types::GoalScope,
    start_date: String,
    end_date: String,
    daily_word_target: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    total_word_target: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    view_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    title: Option<String>,
}

/// A workspace setup file and the name the person chose for its workspace.
/// The setup's shape belongs to the browser's gallery; Rust checks it when
/// it creates the workspace.
#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateWorkspaceFromSetupRequest {
    setup: crate::engine::project::types::OpaqueJson,
    title: String,
}

/// Finish a canonical write only after attached indexes publish its revision.
/// Storage and relationship locks must be released before entering this helper.
pub(crate) fn committed(
    root: &Path,
    result: std::result::Result<Value, MutationError>,
) -> Result<Value> {
    let result = result?;
    let changes = std::iter::once(&result["item"])
        .chain(result["relatedItems"].as_array().into_iter().flatten())
        .map(|item| {
            Ok((
                item["id"]
                    .as_str()
                    .ok_or_else(|| internal_error("Written item has no ID."))?
                    .to_owned(),
                item["revision"]
                    .as_str()
                    .ok_or_else(|| internal_error("Written item has no revision."))?
                    .to_owned(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let started = Instant::now();
    service::publish_project_changes(root, &changes)?;
    tracing::debug!(
        operation = "publish_item_changes",
        changed_items = changes.len(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0
    );
    match result["item"]["meta"]["type"].as_str() {
        Some("property") => service::refresh_project_query_fields(root)?,
        Some("tag" | "label") => crate::catalogue::invalidate_facets(&root.to_string_lossy())?,
        _ => {}
    }
    Ok(result)
}

impl Application {
    pub(super) fn finish_mutation(
        &self,
        result: std::result::Result<Value, MutationError>,
        with_content: bool,
    ) -> Result<PublicItem> {
        let mut result = committed(&self.root, result)?;
        if result["activityWarning"].is_string() {
            tracing::warn!(
                operation = "activity_reconciliation_pending",
                "Activity reconciliation is pending after an item write"
            );
        }
        canonical_item(result["item"].take(), with_content)
    }

    pub(crate) fn create_item(&self, mut request: CreateItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        if request.meta.get("type").and_then(Value::as_str) == Some("goal") {
            return Err(invalid_request(
                "Create writing goals with create_writing_goal.",
            ));
        }
        if request.meta.get("type").and_then(Value::as_str) == Some("workspace") {
            let provided = request
                .content
                .as_object()
                .ok_or_else(|| invalid_request("Workspace content must be an object."))?;
            let mut content =
                crate::engine::project::workspace::instantiate(&self.workspace_template)
                    .map_err(internal_error)?;
            content
                .as_object_mut()
                .ok_or_else(|| internal_error("Invalid workspace template."))?
                .extend(provided.clone());
            request.content = content;
        }
        self.finish_mutation(
            crate::engine::project::write::write_item_input_typed(
                &self.root,
                serde_json::to_value(request).map_err(internal_error)?,
                None,
                &self.activity_date()?,
            ),
            true,
        )
    }

    /// Creates a workspace with a setup's properties, tags and templates in
    /// one write, then refreshes the fields and tags queries offer.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "RPC dispatch owns its decoded request."
    )]
    pub(crate) fn create_workspace_from_setup(
        &self,
        request: CreateWorkspaceFromSetupRequest,
    ) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        let mut result = committed(
            &self.root,
            crate::engine::project::setup::create_workspace_from_setup(
                &self.root,
                &request.setup.0,
                &request.title,
            ),
        )?;
        service::refresh_project_query_fields(&self.root)?;
        crate::catalogue::invalidate_facets(&self.root.to_string_lossy())?;
        canonical_item(result["item"].take(), true)
    }

    pub(crate) fn update_item(&self, request: UpdateItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        let revision = request.version.clone();
        self.finish_mutation(
            crate::engine::project::write::write_item_input_typed(
                &self.root,
                serde_json::to_value(request).map_err(internal_error)?,
                Some(&revision),
                &self.activity_date()?,
            ),
            true,
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn update_item_meta(&self, request: UpdateItemMetaRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        self.finish_mutation(
            crate::engine::project::write::mutate_existing_item(
                &self.root,
                &request.id,
                Some(&request.version),
                &self.activity_date()?,
                |_| Ok(json!({"id": request.id, "meta": request.meta})),
            ),
            request.options.content == Some(true),
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn set_item_tags(&self, request: SetItemTagsRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        self.finish_mutation(
            crate::engine::project::write::mutate_existing_item(
                &self.root,
                &request.id,
                Some(&request.version),
                &self.activity_date()?,
                |_| Ok(json!({"id": request.id, "meta": {}, "tags": request.tags})),
            ),
            true,
        )
    }
    pub(crate) fn mutate_note_task(&self, request: MutateNoteTaskRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        self.finish_mutation(
            crate::engine::project::task::mutate_note_task(
                &self.root,
                &request.note_id,
                &serde_json::to_value(request.change).map_err(internal_error)?,
                &self.activity_date()?,
            ),
            true,
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn set_note_pin(&self, request: SetNotePinRequest) -> Result<PinResult> {
        let _operation = self.begin_operation()?;
        let item = self.finish_mutation(
            crate::engine::project::item_mutations::set_note_pin(
                &self.root,
                &request.note_id,
                &request.version,
                request.pinned,
                &request.scope_id,
                &self.activity_date()?,
            ),
            false,
        )?;
        Ok(PinResult {
            pinned: request.pinned,
            version: item.version,
        })
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn delete_item(&self, request: DeleteItemRequest) -> Result<DeleteResult> {
        let _operation = self.begin_operation()?;
        self.finish_mutation(
            crate::engine::project::item_mutations::archive_item(
                &self.root,
                &request.id,
                &self.activity_date()?,
            ),
            false,
        )?;
        Ok(DeleteResult { deleted: true })
    }

    pub(crate) fn create_writing_goal(
        &self,
        request: CreateWritingGoalRequest,
    ) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        let request = serde_json::to_value(request).map_err(internal_error)?;
        self.finish_mutation(
            crate::engine::project::goal::create_writing_goal(
                &self.root,
                &request,
                &self.activity_date()?,
                |source| {
                    let index = self
                        .index_path()
                        .map_err(|error| MutationError::Validation(error.to_string()))?;
                    let MainSnapshot {
                        index: native,
                        query_fields: fields,
                        ..
                    } = service::service_index(&index)
                        .map_err(|error| MutationError::Internal(error.to_string()))?;
                    query::compile_query_document(&native, source, &fields, None)
                        .map(|_| ())
                        .map_err(|error| MutationError::Validation(error.to_string()))
                },
            ),
            true,
        )
    }
}
