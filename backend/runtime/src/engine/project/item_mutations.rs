// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use serde_json::{Value, json};

use crate::engine::project::write::{ALL_NOTES_PIN_SCOPE_ID, MutationError, mutate_existing_item};

pub(crate) fn set_note_pin(
    root: &Path,
    id: &str,
    revision: &str,
    pinned: bool,
    scope_id: Option<&str>,
    activity_date: &str,
) -> Result<Value, MutationError> {
    let scope_id = scope_id.unwrap_or(ALL_NOTES_PIN_SCOPE_ID);
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{archive_item, set_note_pin};
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_item_input;

    #[test]
    fn pin_and_archive_use_the_current_canonical_revision() {
        let root = std::env::temp_dir().join(format!("bluink-pin-{}", uuid::Uuid::now_v7()));
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
                "meta": {"type": "page", "title": "Pinned draft"},
                "content": {"type": "doc", "content": [{"type": "paragraph"}]}
            }),
            None,
            "2026-10-06",
        )
        .unwrap();
        let revision = first["item"]["revision"].as_str().unwrap();
        let pinned = set_note_pin(&root, &id, revision, true, None, "2026-10-06").unwrap();
        assert_eq!(
            pinned["item"]["pins"],
            json!(["00000000-0000-7000-8000-000000000001"])
        );
        assert!(set_note_pin(&root, &id, revision, false, None, "2026-10-06").is_err());
        let archived = archive_item(&root, &id, "2026-10-06").unwrap();
        assert_eq!(archived["item"]["meta"]["state"], "archived");
        assert_eq!(archived["item"]["content"], first["item"]["content"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
