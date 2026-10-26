// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Tasks are canonical items. A task owns its revision and references one page.
use super::item::{is_canonical_instant, read_project_item};
use super::write::{MutationError, mutate_existing_item, write_item_input_typed};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn validate_content(content: &Value) -> Result<(), String> {
    let object = content
        .as_object()
        .ok_or("Task content must be an object.")?;
    if !object
        .keys()
        .all(|key| matches!(key.as_str(), "noteId" | "text" | "completedAt"))
    {
        return Err("Task content contains an unsupported field.".to_owned());
    }
    if !content["noteId"]
        .as_str()
        .is_some_and(super::identity::is_item_id)
    {
        return Err("A task needs its owning note ID.".to_owned());
    }
    task_text(content.get("text")).map_err(|error| error.to_string())?;
    if content
        .get("completedAt")
        .is_some_and(|value| !value.as_str().is_some_and(is_canonical_instant))
    {
        return Err("Task completion must be a canonical UTC instant.".to_owned());
    }
    Ok(())
}

/// Called under the shared relationship lock by every item writer.
pub(crate) fn validate_item_change(
    root: &Path,
    next: &Value,
    existing: Option<&Value>,
) -> Result<(), MutationError> {
    if next["meta"]["type"] != "task" {
        return Ok(());
    }
    if existing.is_some_and(|previous| previous["content"]["noteId"] != next["content"]["noteId"]) {
        return Err(expected("A task cannot be moved to a different note."));
    }
    let note_id = next["content"]["noteId"]
        .as_str()
        .ok_or_else(|| expected("A task needs its owning note ID."))?;
    let parent = read_project_item(root, note_id)
        .map_err(MutationError::Read)?
        .ok_or_else(|| expected("The task's owning note no longer exists."))?;
    if parent["meta"]["type"] != "page"
        || (next["meta"]["state"] == "active" && parent["meta"]["state"] != "active")
    {
        return Err(expected("Active tasks belong to an active note."));
    }
    Ok(())
}

pub(crate) fn mutate_note_task(
    root: &Path,
    note_id: &str,
    change: &Value,
    activity_date: &str,
) -> Result<Value, MutationError> {
    let kind = change["kind"]
        .as_str()
        .ok_or_else(|| expected("Choose a task action."))?;
    if kind == "create" {
        let text = task_text(change.get("text"))?;
        let mut meta = json!({"type":"task","title":text});
        if let Some(parent) = read_project_item(root, note_id).map_err(MutationError::Read)?
            && let Some(due_date) = parent["meta"].get("due_date")
        {
            meta["due_date"] = due_date.clone();
        }
        return write_item_input_typed(
            root,
            json!({"meta":meta,
            "content":{"noteId":note_id,"text":text}}),
            None,
            activity_date,
        );
    }
    let task_id = change["taskId"]
        .as_str()
        .ok_or_else(|| expected("Choose a task in this note."))?;
    let revision = change["revision"]
        .as_str()
        .ok_or_else(|| expected("A task revision is required."))?;
    mutate_existing_item(root, task_id, Some(revision), activity_date, |previous| {
        if previous["meta"]["type"] != "task"
            || previous["content"]["noteId"] != note_id
            || previous["meta"]["state"] != "active"
        {
            return Err(expected("That task is no longer active in this note."));
        }
        let mut content = previous["content"].clone();
        let mut meta = json!({});
        match kind {
            "edit" => {
                let text = task_text(change.get("text"))?;
                content["text"] = json!(text);
                meta["title"] = json!(text);
            }
            "complete" => {
                if content.get("completedAt").is_none() {
                    content["completedAt"] =
                        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
                }
            }
            "reopen" => {
                content
                    .as_object_mut()
                    .ok_or_else(|| expected("Invalid task content."))?
                    .remove("completedAt");
            }
            "delete" => {
                meta["state"] = json!("archived");
            }
            _ => return Err(expected("Choose a supported task action.")),
        }
        Ok(json!({"id":task_id,"meta":meta,"content":content}))
    })
}

/// Indexed candidates plus pending canonical writes avoid scanning unrelated files.
/// An offline engine caller without an index can still inspect its portable project.
pub(crate) fn owned_active_items(root: &Path, note_id: &str) -> Result<Vec<Value>, MutationError> {
    let snapshot = crate::indexing::service::project_index(root)
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    let ids = if let Some(snapshot) = snapshot {
        let schema = snapshot.index.schema();
        let field = |name| {
            schema
                .get_field(name)
                .map_err(|error| MutationError::Internal(error.to_string()))
        };
        let query = tantivy::query::BooleanQuery::intersection(vec![
            crate::relations::exact_term_query(field("item_type")?, "task"),
            crate::relations::exact_term_query(field("note_id")?, note_id),
        ]);
        let mut ids = crate::indexing::fast::matching_ids(&snapshot.reader.searcher(), &query)
            .map_err(MutationError::Internal)?;
        ids.extend(snapshot.dirty.keys().cloned());
        ids.sort_unstable();
        ids.dedup();
        ids
    } else {
        super::item::list_project_item_ids(root).map_err(MutationError::Internal)?
    };
    let mut tasks = Vec::new();
    for id in ids {
        if let Some(item) = read_project_item(root, &id).map_err(MutationError::Read)?
            && item["meta"]["type"] == "task"
            && item["meta"]["state"] == "active"
            && item["content"]["noteId"] == note_id
        {
            tasks.push(item);
        }
    }
    Ok(tasks)
}

/// Prepare children for the writer's durable multi-item transaction.
pub(crate) fn archive_children(
    root: &Path,
    next: &Value,
    timezone: &str,
) -> Result<Vec<Value>, MutationError> {
    if next["meta"]["type"] != "page" || next["meta"]["state"] == "active" {
        return Ok(Vec::new());
    }
    let note_id = next["id"]
        .as_str()
        .ok_or_else(|| expected("Note has no ID."))?;
    owned_active_items(root, note_id)?
        .into_iter()
        .map(|task| {
            super::assemble::assemble_item(
                &json!({"id":task["id"],"meta":{"state":"archived"}}),
                Some(&task),
                timezone,
            )
            .map_err(MutationError::Validation)
        })
        .collect()
}

fn task_text(value: Option<&Value>) -> Result<String, MutationError> {
    let text = value.and_then(Value::as_str).map_or("", str::trim);
    if text.is_empty() || text.encode_utf16().count() > 240 {
        return Err(expected("Enter task text of at most 240 characters."));
    }
    Ok(text.to_owned())
}
fn expected(message: &str) -> MutationError {
    MutationError::Validation(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_task_validation_rejects_invalid_owners_and_payloads() {
        let owner = "11111111-1111-7111-8111-111111111111";
        assert!(validate_content(&json!({"noteId":owner,"text":"Review"})).is_ok());
        for content in [
            json!({"noteId":"../outside","text":"Review"}),
            json!({"noteId":owner,"text":" "}),
            json!({"noteId":owner,"text":"Review","completedAt":"yesterday"}),
            json!({"noteId":owner,"text":"Review","anchor":"paragraph"}),
        ] {
            assert!(validate_content(&content).is_err());
        }
    }

    #[test]
    fn task_owner_is_immutable_and_must_be_an_active_page() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        super::super::config::open_or_create_project(&root, "UTC").unwrap();
        let page = write_item_input_typed(
            &root,
            json!({"meta":{"type":"page","title":"Owner"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
            None,
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let other = write_item_input_typed(
            &root,
            json!({"meta":{"type":"page","title":"Other"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
            None,
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let task = mutate_note_task(
            &root,
            page["id"].as_str().unwrap(),
            &json!({"kind":"create","text":"Review"}),
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let changed = write_item_input_typed(
            &root,
            json!({"id":task["id"],"meta":{},"content":{"noteId":other["id"],"text":"Review"}}),
            task["revision"].as_str(),
            "2026-10-24",
        )
        .unwrap_err();
        assert!(changed.to_string().contains("different note"));
        assert!(
            mutate_note_task(
                &root,
                other["id"].as_str().unwrap(),
                &json!({"kind":"complete","taskId":task["id"],"revision":task["revision"]}),
                "2026-10-24"
            )
            .is_err()
        );
    }
}
