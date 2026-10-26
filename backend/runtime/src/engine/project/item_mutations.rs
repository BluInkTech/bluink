// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use serde_json::{Value, json};

use crate::engine::project::write::{MutationError, mutate_existing_item};

pub(crate) fn set_note_pin(
    root: &Path,
    id: &str,
    revision: &str,
    pinned: bool,
    scope_id: &str,
    activity_date: &str,
) -> Result<Value, MutationError> {
    mutate_existing_item(root, id, Some(revision), activity_date, |previous| {
        let mut pins: Vec<Value> = previous["pins"].as_array().cloned().unwrap_or_default();
        pins.retain(|value| value.as_str() != Some(scope_id));
        if pinned {
            pins.push(json!(scope_id));
        }
        Ok(json!({"id": id, "meta": {}, "pins": pins}))
    })
}

pub(crate) fn archive_item(
    root: &Path,
    id: &str,
    activity_date: &str,
) -> Result<Value, MutationError> {
    mutate_existing_item(root, id, None, activity_date, |_| {
        Ok(json!({"id": id, "meta": {"state": "archived"}}))
    })
}

pub(crate) fn archive_workspace(
    root: &Path,
    id: &str,
    revision: &str,
    activity_date: &str,
) -> Result<Value, MutationError> {
    mutate_existing_item(root, id, Some(revision), activity_date, |previous| {
        if previous["meta"]["type"] != "workspace" || previous["meta"]["state"] != "active" {
            return Err(MutationError::Validation(
                "Choose an active workspace to remove.".into(),
            ));
        }
        Ok(json!({"id":id,"meta":{"state":"archived"}}))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{archive_item, set_note_pin};
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_item_input;

    #[test]
    fn pin_and_archive_use_the_current_canonical_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config.clone()).unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
        let scope_id = config["defaultWorkspaceId"].as_str().unwrap();
        write_item_input(
            &root,
            json!({"id":scope_id,"meta":{"type":"workspace","title":"All notes"},"content":crate::engine::project::workspace::initial_content()}),
            None,
            "2026-10-06",
        )
        .unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let first = write_item_input(
            &root,
            json!({
                "id": id,
                "meta": {"type": "page", "title": "Pinned draft"},
                "content": {"type": "doc", "content": [{"type": "paragraph"}]}
            }),
            None,
            "2026-10-06",
        )
        .unwrap();
        let revision = first["item"]["revision"].as_str().unwrap();
        let pinned = set_note_pin(&root, &id, revision, true, scope_id, "2026-10-06").unwrap();
        assert_eq!(pinned["item"]["pins"], json!([scope_id]));
        assert!(set_note_pin(&root, &id, revision, false, scope_id, "2026-10-06").is_err());
        let archived = archive_item(&root, &id, "2026-10-06").unwrap();
        assert_eq!(archived["item"]["meta"]["state"], "archived");
        assert_eq!(archived["item"]["content"], first["item"]["content"]);
        temporary.close().unwrap();
    }
}
