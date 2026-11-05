// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Tasks are canonical items. A task owns its revision and references one page.
use super::{identity::ItemId, item::is_canonical_instant};
use serde_json::{Value, json};

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
    task_text(content.get("text"))?;
    if content
        .get("completedAt")
        .is_some_and(|value| !value.as_str().is_some_and(is_canonical_instant))
    {
        return Err("Task completion must be a canonical UTC instant.".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_owner(next: &Value, parent: Option<&Value>) -> Result<(), String> {
    let parent = parent.ok_or("The task's owning note no longer exists.")?;
    if parent["meta"]["type"] != "page"
        || (next["meta"]["state"] == "active" && parent["meta"]["state"] != "active")
    {
        return Err("Active tasks belong to an active note.".to_owned());
    }
    Ok(())
}

pub(crate) fn new_task(
    note_id: &str,
    change: &Value,
    parent: Option<&Value>,
) -> Result<Value, String> {
    let text = task_text(change.get("text"))?;
    ItemId::try_from(note_id)?;
    let mut meta = json!({"type":"task","title":text});
    if let Some(due_date) = parent.and_then(|parent| parent["meta"].get("due_date")) {
        meta["due_date"] = due_date.clone();
    }
    Ok(json!({"meta":meta,"content":{"noteId":note_id,"text":text}}))
}

pub(crate) fn change_task(
    note_id: &str,
    change: &Value,
    previous: &Value,
    completed_at: &str,
) -> Result<Value, String> {
    let kind = change["kind"].as_str().ok_or("Choose a task action.")?;
    let task_id = change["taskId"]
        .as_str()
        .ok_or("Choose a task in this note.")?;
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
                content["completedAt"] = json!(completed_at);
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
        "move" => {
            // The write rechecks that the new owner is an active page.
            let target = change["toNoteId"]
                .as_str()
                .ok_or_else(|| expected("Choose a page to move the task to."))?;
            ItemId::try_from(target)?;
            if target == note_id {
                return Err(expected("The task is already on that page."));
            }
            content["noteId"] = json!(target);
        }
        _ => return Err(expected("Choose a supported task action.")),
    }
    Ok(json!({"id":task_id,"meta":meta,"content":content}))
}

fn task_text(value: Option<&Value>) -> Result<String, String> {
    let text = value.and_then(Value::as_str).map_or("", str::trim);
    if text.is_empty() || text.encode_utf16().count() > 240 {
        return Err(expected("Enter task text of at most 240 characters."));
    }
    Ok(text.to_owned())
}
fn expected(message: &str) -> String {
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::item_writes::{mutate_note_task, write_item_input_typed};
    use crate::store::Store;

    #[test]
    fn completion_uses_the_supplied_instant_and_preserves_an_existing_completion() {
        let note_id = "11111111-1111-7111-8111-111111111111";
        let task_id = "22222222-2222-7222-8222-222222222222";
        let previous = json!({"id":task_id,"meta":{"type":"task","state":"active"},
            "content":{"noteId":note_id,"text":"Review"}});
        let change = json!({"kind":"complete","taskId":task_id});
        let timestamp = "2026-11-05T12:00:00.000Z";
        let completed = change_task(note_id, &change, &previous, timestamp).unwrap();
        assert_eq!(completed["content"]["completedAt"], timestamp);
        assert_eq!(
            completed,
            change_task(note_id, &change, &previous, timestamp).unwrap()
        );
        let mut already = previous;
        already["content"] = completed["content"].clone();
        assert_eq!(
            change_task(note_id, &change, &already, "2026-11-06T12:00:00.000Z").unwrap()["content"]
                ["completedAt"],
            timestamp
        );
    }

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
    fn a_task_moves_only_to_another_active_page() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        super::super::config::open_or_create_project(&root, "UTC").unwrap();
        let store = Store::shared(&root).unwrap();
        let page = |title: &str| {
            write_item_input_typed(
                &store,
                json!({"meta":{"type":"page","title":title},"content":{"document": {"type":"doc","content":[{"type":"paragraph"}]}}}),
                None,
                "2026-10-24",
            )
            .unwrap()["item"]
                .clone()
        };
        let (inbox, project, archived) = (page("Inbox"), page("Project"), page("Old"));
        let id = |item: &Value| item["id"].as_str().unwrap().to_owned();
        crate::store::commands::metadata(
            &store,
            &id(&archived),
            None,
            serde_json::from_value(json!({"state":"archived"})).unwrap(),
            false,
        )
        .unwrap();
        let task = mutate_note_task(
            &store,
            &id(&inbox),
            &json!({"kind":"create","text":"Call the plumber"}),
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let change =
            |owner: &str, kind: Value| mutate_note_task(&store, owner, &kind, "2026-10-24");
        let refused = change(
            &id(&inbox),
            json!({"kind":"move","taskId":task["id"],"revision":task["revision"],"toNoteId":id(&archived)}),
        )
        .unwrap_err();
        assert!(refused.to_string().contains("active note"), "{refused}");
        assert!(
            change(
                &id(&inbox),
                json!({"kind":"move","taskId":task["id"],"revision":task["revision"],"toNoteId":id(&inbox)}),
            )
            .is_err()
        );
        let moved = change(
            &id(&inbox),
            json!({"kind":"move","taskId":task["id"],"revision":task["revision"],"toNoteId":id(&project)}),
        )
        .unwrap()["item"]
            .clone();
        assert_eq!(moved["content"]["noteId"], project["id"]);
        // The task now answers to its new page only.
        let complete = json!({"kind":"complete","taskId":moved["id"],"revision":moved["revision"]});
        assert!(change(&id(&inbox), complete.clone()).is_err());
        assert!(change(&id(&project), complete).is_ok());
    }
}
