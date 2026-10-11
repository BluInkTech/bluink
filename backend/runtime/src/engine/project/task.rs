// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::engine::project::write::{MutationError, mutate_existing_item};

pub(crate) fn mutate_note_task(
    root: &Path,
    note_id: &str,
    revision: &str,
    change: &Value,
    activity_date: &str,
) -> Result<Value, MutationError> {
    mutate_existing_item(root, note_id, Some(revision), activity_date, |previous| {
        if previous["meta"]["type"] != "page" || previous["meta"]["state"] != "active" {
            return Err(expected("Tasks belong to an active note."));
        }
        let change = change
            .as_object()
            .ok_or_else(|| expected("Choose a task action."))?;
        if change.contains_key("anchor") {
            return Err(expected(
                "Tasks belong to the note, not a text or block anchor.",
            ));
        }
        let kind = change
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| expected("Choose a task action."))?;
        let mut tasks = previous["tasks"].as_array().cloned().unwrap_or_default();
        if kind == "create" {
            let text = task_text(change.get("text"))?;
            tasks.push(json!({
                "id": Uuid::now_v7().to_string(),
                "text": text,
                "createdAt": timestamp(),
            }));
        } else {
            let task_id = change
                .get("taskId")
                .and_then(Value::as_str)
                .ok_or_else(|| expected("Choose a task in this note."))?;
            let index = tasks
                .iter()
                .position(|task| task["id"].as_str() == Some(task_id))
                .ok_or_else(|| expected("That task is no longer in this note."))?;
            match kind {
                "edit" => {
                    tasks[index]["text"] = Value::String(task_text(change.get("text"))?);
                }
                "complete" => {
                    if tasks[index].get("completedAt").is_none() {
                        tasks[index]["completedAt"] = Value::String(timestamp());
                    }
                }
                "reopen" => {
                    tasks[index]
                        .as_object_mut()
                        .expect("validated task")
                        .remove("completedAt");
                }
                "delete" => {
                    tasks.remove(index);
                }
                _ => return Err(expected("Choose a supported task action.")),
            }
        }
        Ok(json!({"id": note_id, "meta": {}, "tasks": tasks}))
    })
}

fn task_text(value: Option<&Value>) -> Result<String, MutationError> {
    let text = value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if text.is_empty() || text.encode_utf16().count() > 240 {
        return Err(expected("Enter task text of at most 240 characters."));
    }
    Ok(text.to_owned())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn expected(message: &str) -> MutationError {
    MutationError::Expected(message.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::mutate_note_task;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::item::read_project_item;
    use crate::engine::project::write::write_item_input;

    #[test]
    fn task_actions_keep_content_and_reject_stale_concurrent_writes() {
        let root = std::env::temp_dir().join(format!("bluink-task-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let first = write_item_input(
            &root,
            json!({
                "id": id,
                "meta": {"type": "page", "title": "Draft"},
                "content": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Keep this text"}]}]}
            }),
            None,
            "2026-10-06",
        )
        .unwrap();
        let revision = first["item"]["revision"].as_str().unwrap().to_owned();
        let root_a = root.clone();
        let root_b = root.clone();
        let id_a = id.clone();
        let id_b = id.clone();
        let revision_a = revision.clone();
        let revision_b = revision;
        let left = std::thread::spawn(move || {
            mutate_note_task(
                &root_a,
                &id_a,
                &revision_a,
                &json!({"kind": "create", "text": "First task"}),
                "2026-10-06",
            )
        });
        let right = std::thread::spawn(move || {
            mutate_note_task(
                &root_b,
                &id_b,
                &revision_b,
                &json!({"kind": "create", "text": "Second task"}),
                "2026-10-06",
            )
        });
        let results = [left.join().unwrap(), right.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let created = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(created["item"]["content"], first["item"]["content"]);
        let task_id = created["item"]["tasks"][0]["id"].as_str().unwrap();
        let completed = mutate_note_task(
            &root,
            &id,
            created["item"]["revision"].as_str().unwrap(),
            &json!({"kind": "complete", "taskId": task_id}),
            "2026-10-06",
        )
        .unwrap();
        assert!(completed["item"]["tasks"][0]["completedAt"].is_string());
        let reopened = mutate_note_task(
            &root,
            &id,
            completed["item"]["revision"].as_str().unwrap(),
            &json!({"kind": "reopen", "taskId": task_id}),
            "2026-10-06",
        )
        .unwrap();
        assert!(reopened["item"]["tasks"][0].get("completedAt").is_none());
        assert_eq!(
            read_project_item(&root, &id).unwrap().unwrap(),
            reopened["item"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
