// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use crate::engine::project::write::MutationError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use super::items::{PublicItem, ReadOptions, canonical_item};
use crate::{query, service};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    tasks: Option<Vec<crate::engine::project::types::NoteTask>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    tasks: Option<Vec<crate::engine::project::types::NoteTask>>,
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
    version: String,
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
    scope_id: Option<String>,
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

/// Canonical persistence and projection invalidation are one operation. The
/// old CLI bridge shares this helper until its callers migrate to Application.
pub(crate) fn committed(
    root: &Path,
    result: std::result::Result<Value, MutationError>,
) -> Result<Value> {
    let result = result?;
    let item = &result["item"];
    service::mark_project_write(
        root,
        item["id"]
            .as_str()
            .ok_or_else(|| internal_error("Written item has no ID."))?,
        item["revision"]
            .as_str()
            .ok_or_else(|| internal_error("Written item has no revision."))?,
    )?;
    Ok(result)
}

impl Application {
    fn finish_mutation(
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

    pub(crate) fn create_item(&self, request: CreateItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        if request.meta.get("type").and_then(Value::as_str) == Some("goal") {
            return Err(invalid_request(
                "Create writing goals with create_writing_goal.",
            ));
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
                &request.version,
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
                request.scope_id.as_deref(),
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
                        .map_err(|error| MutationError::Expected(error.to_string()))?;
                    let fields = service::query_fields(&index)
                        .map_err(|error| MutationError::Unexpected(error.to_string()))?;
                    let analysis = query::analyze_bluink_query_with_schema(source, &fields);
                    if analysis.valid {
                        Ok(())
                    } else {
                        Err(MutationError::Expected(
                            analysis.diagnostics.first().map_or_else(
                                || "The saved view has an invalid query.".to_owned(),
                                |diagnostic| diagnostic.message.clone(),
                            ),
                        ))
                    }
                },
            ),
            true,
        )
    }
}
