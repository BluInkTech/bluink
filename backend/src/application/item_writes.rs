// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) use super::error::MutationError;
use super::{
    Application,
    error::{Result, internal_error, internal_message, invalid_request},
};
use crate::records::items::{PublicItem, ReadOptions, canonical_item};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreateItemRequest {
    #[serde(default)]
    #[cfg_attr(
        test,
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>>",
            optional
        )
    )]
    meta: Map<String, Value>,
    #[serde(default = "empty_content")]
    #[cfg_attr(
        test,
        ts(
            as = "Option<std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>>",
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
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
    )]
    meta: Map<String, Value>,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
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
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
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
    change: crate::domain::project::types::TaskChange,
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
    scope: crate::domain::project::types::GoalScope,
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
    setup: crate::domain::project::types::OpaqueJson,
    title: String,
}

impl Application {
    pub(super) fn finish_mutation(
        result: std::result::Result<Value, MutationError>,
        with_content: bool,
    ) -> Result<PublicItem> {
        let mut result = result?;
        Ok(canonical_item(result["item"].take(), with_content)?)
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
            let mut content = crate::store::commands::with_authoring_values(|values| {
                crate::domain::project::workspace::instantiate(&self.workspace_template, values)
            })
            .map_err(internal_message)?;
            content
                .as_object_mut()
                .ok_or_else(|| internal_message("Invalid workspace template."))?
                .extend(provided.clone());
            request.content = content;
        }
        Ok(canonical_item(
            crate::store::commands::write(
                self.database()?.as_ref(),
                serde_json::to_value(request).map_err(internal_error)?,
                None,
                &self.activity_date()?,
            )?,
            true,
        )?)
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
        let mut result = super::setups::create_workspace_from_setup(
            self.database()?.as_ref(),
            &request.setup.0,
            &request.title,
        )?;
        Ok(canonical_item(result["item"].take(), true)?)
    }

    pub(crate) fn update_item(&self, request: UpdateItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        let revision = request.version.clone();
        Ok(canonical_item(
            crate::store::commands::write(
                self.database()?.as_ref(),
                serde_json::to_value(request).map_err(internal_error)?,
                Some(&revision),
                &self.activity_date()?,
            )?,
            true,
        )?)
    }
    pub(crate) fn update_item_meta(&self, request: UpdateItemMetaRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        Ok(canonical_item(
            crate::store::commands::metadata(
                self.database()?.as_ref(),
                &request.id,
                Some(&request.version),
                request.meta,
                request.options.content == Some(true),
            )?,
            request.options.content == Some(true),
        )?)
    }
    pub(crate) fn set_item_tags(&self, request: SetItemTagsRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        Ok(canonical_item(
            crate::store::commands::memberships(
                self.database()?.as_ref(),
                &request.id,
                &request.version,
                crate::store::commands::MembershipChange::Tags(request.tags),
                true,
            )?,
            true,
        )?)
    }
    pub(crate) fn mutate_note_task(&self, request: MutateNoteTaskRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        Self::finish_mutation(
            mutate_note_task(
                self.database()?.as_ref(),
                &request.note_id,
                &serde_json::to_value(request.change).map_err(internal_error)?,
                &self.activity_date()?,
            ),
            true,
        )
    }
    pub(crate) fn set_note_pin(&self, request: SetNotePinRequest) -> Result<PinResult> {
        let _operation = self.begin_operation()?;
        let item = canonical_item(
            crate::store::commands::memberships(
                self.database()?.as_ref(),
                &request.note_id,
                &request.version,
                crate::store::commands::MembershipChange::Pin {
                    scope: request.scope_id,
                    pinned: request.pinned,
                },
                false,
            )?,
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
        crate::store::commands::metadata(
            self.database()?.as_ref(),
            &request.id,
            None,
            serde_json::Map::from_iter([("state".to_owned(), json!("archived"))]),
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
        let store = self.database()?;
        Self::finish_mutation(
            create_writing_goal(&store, &request, &self.activity_date()?),
            true,
        )
    }
}

/// Prepare authoring intent from a canonical snapshot, then revision-check its commit.
pub(crate) fn mutate_existing_item(
    store: &Store,
    id: &str,
    expected_revision: Option<&str>,
    activity_date: &str,
    change: impl FnOnce(&Value) -> std::result::Result<Value, MutationError>,
) -> std::result::Result<Value, MutationError> {
    let previous = store
        .read(|tx| crate::store::item::read(tx, id, true))?
        .ok_or_else(|| MutationError::NotFound(format!("Item {id} not found")))?;
    let revision = previous["revision"]
        .as_str()
        .ok_or_else(|| MutationError::internal("Item has no revision"))?;
    if expected_revision.is_some_and(|expected| expected != revision) {
        return Err(MutationError::RevisionConflict(
            "The item changed since it was read.".to_owned(),
        ));
    }
    let input = change(&previous)?;
    let next = crate::store::commands::write(store, input, Some(revision), activity_date)?;
    Ok(json!({"item":next}))
}

/// Assemble and commit authoring intent. The caller sends an expected revision;
/// Rust owns the canonical envelope and the writer rechecks the revision.
pub(crate) fn write_item_input_typed(
    store: &Store,
    input: Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> std::result::Result<Value, MutationError> {
    let next = crate::store::commands::write(store, input, expected_revision, activity_date)?;
    Ok(json!({"item":next}))
}

/// Test support: write authoring intent to the project at `root` through the
/// same store command as the application.
#[cfg(test)]
pub(crate) fn write_item_input(
    root: &std::path::Path,
    input: Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> std::result::Result<Value, String> {
    Store::shared(root)
        .map_err(MutationError::from)
        .and_then(|store| write_item_input_typed(&store, input, expected_revision, activity_date))
        .map_err(|error| error.to_string())
}

/// Load the owner/task facts, calculate the transition, and revision-check its write.
pub(crate) fn mutate_note_task(
    store: &Store,
    note_id: &str,
    change: &Value,
    activity_date: &str,
) -> std::result::Result<Value, MutationError> {
    use crate::domain::project::task;
    let kind = change["kind"]
        .as_str()
        .ok_or_else(|| MutationError::Validation("Choose a task action.".into()))?;
    if kind == "create" {
        let parent = store.read(|tx| crate::store::item::read(tx, note_id, false))?;
        let input =
            task::new_task(note_id, change, parent.as_ref()).map_err(MutationError::Validation)?;
        return write_item_input_typed(store, input, None, activity_date);
    }
    let id = change["taskId"]
        .as_str()
        .ok_or_else(|| MutationError::Validation("Choose a task in this note.".into()))?;
    let revision = change["revision"]
        .as_str()
        .ok_or_else(|| MutationError::Validation("A task revision is required.".into()))?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    mutate_existing_item(store, id, Some(revision), activity_date, |previous| {
        task::change_task(note_id, change, previous, &now).map_err(MutationError::Validation)
    })
}

pub(crate) fn create_writing_goal(
    store: &Store,
    request: &Value,
    activity_date: &str,
) -> std::result::Result<Value, MutationError> {
    let view = if request["scope"] == "view" {
        request["viewId"]
            .as_str()
            .map(|id| store.read(|tx| crate::store::item::read(tx, id, true)))
            .transpose()?
            .flatten()
    } else {
        None
    };
    let input = crate::domain::project::goal::prepare_writing_goal(request, view.as_ref())
        .map_err(MutationError::Validation)?;
    if input["content"]["scope"] == "view" {
        let query = serde_json::from_value(input["content"]["query"].clone())?;
        let scoped = crate::domain::project::query::QueryDocument::active_pages()
            .compose(&query)
            .map_err(|errors| MutationError::Validation(errors[0].message.clone()))?;
        store.read(|tx| {
            let timezone = crate::store::reads::timezone(tx)?;
            crate::store::query::compile(tx, &scoped, None, timezone).map(drop)
        })?;
    }
    write_item_input_typed(store, input, None, activity_date)
}

pub(crate) fn write_asset(
    root: &std::path::Path,
    id: &str,
    filename: &str,
    data: &[u8],
) -> std::result::Result<String, MutationError> {
    crate::store::assets::upload(root, id, filename, data).map_err(|error| match error {
        crate::store::Error::Invalid(message) => MutationError::Validation(message),
        crate::store::Error::NotFound => MutationError::NotFound(format!("Item {id} not found")),
        other => MutationError::Database(other),
    })
}
