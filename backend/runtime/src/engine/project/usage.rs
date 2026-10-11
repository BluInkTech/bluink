// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use serde_json::Value;

use crate::engine::document::projection::project_document;
use crate::engine::project::item::{list_project_item_ids, read_project_item};

fn any_active_item(root: &Path, mut used: impl FnMut(&Value) -> bool) -> Result<bool, String> {
    for id in list_project_item_ids(root)? {
        let Some(item) = read_project_item(root, &id)? else {
            continue;
        };
        if item["meta"]["state"] == "active" && used(&item) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn property_used(root: &Path, key: &str) -> Result<bool, String> {
    any_active_item(root, |item| {
        item["meta"].get(key).is_some_and(|value| !value.is_null())
    })
}

pub(crate) fn facet_used(root: &Path, kind: &str, key: &str) -> Result<bool, String> {
    any_active_item(root, |item| match kind {
        "tag" => item["tags"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(key))),
        "label" if item["meta"]["type"] == "page" => project_document(&item["content"])
            .labels
            .iter()
            .any(|label| label == key),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::engine::project::config::open_or_create_project;
    use crate::engine::project::facets::{create_facet, remove_facet};
    use crate::engine::project::item_mutations::archive_item;
    use crate::engine::project::properties::{create_property, remove_property};
    use crate::engine::project::write::write_item_input;

    #[test]
    fn pending_canonical_usage_blocks_removal_until_the_page_is_archived() {
        let root = std::env::temp_dir().join(format!("bluink-usage-{}", uuid::Uuid::now_v7()));
        open_or_create_project(&root, "UTC").unwrap();
        create_facet(&root, "tag", "astronomy").unwrap();
        create_facet(&root, "label", "citation_text").unwrap();
        create_property(&root, json!({"key": "project_code", "valueType": "text"})).unwrap();
        let page = write_item_input(
            &root,
            json!({
                "meta": {"type": "page", "title": "Pending index", "project_code": "p1"},
                "tags": ["astronomy"],
                "content": {"type": "doc", "content": [{
                    "type": "paragraph", "attrs": {"labels": ["citation_text"]},
                    "content": [{"type": "text", "text": "Evidence"}]
                }]}
            }),
            None,
            "2026-10-06",
        )
        .unwrap();
        assert!(
            remove_property(&root, "project_code")
                .unwrap_err()
                .to_string()
                .contains("used by")
        );
        assert!(
            remove_facet(&root, "tag", "astronomy")
                .unwrap_err()
                .to_string()
                .contains("unused")
        );
        assert!(
            remove_facet(&root, "label", "citation_text")
                .unwrap_err()
                .to_string()
                .contains("unused")
        );
        archive_item(&root, page["item"]["id"].as_str().unwrap(), "2026-10-06").unwrap();
        remove_property(&root, "project_code").unwrap();
        remove_facet(&root, "tag", "astronomy").unwrap();
        remove_facet(&root, "label", "citation_text").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
