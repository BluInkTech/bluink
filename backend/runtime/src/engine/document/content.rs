// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::engine::document::validation::validate_document;
use crate::engine::project::item::is_item_id;

const ADDRESSABLE_BLOCKS: &[&str] = &[
    "paragraph",
    "heading",
    "code_block",
    "horizontal_rule",
    "blockquote",
    "list",
    "listItem",
    "image",
    "box",
    "boxHeading",
    "figure",
    "table",
    "table_row",
    "table_cell",
    "table_header",
    "embed",
];

/// Prepare a page's canonical content for writing. Existing unique block UUIDs
/// survive edits; copied or missing identities receive fresh `UUIDv7` values.
pub(crate) fn prepare_page_content(mut content: Value) -> Result<Value, String> {
    normalize_block_ids(&mut content, &mut HashSet::new(), 0)?;
    let document = content
        .get("content")
        .filter(|value| value.is_object())
        .unwrap_or(&content);
    validate_document(document).map_err(|error| error.to_string())?;
    Ok(content)
}

fn normalize_block_ids(
    value: &mut Value,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 256 {
        return Err("Document nesting is too deep".to_owned());
    }
    if let Some(values) = value.as_array_mut() {
        for value in values {
            normalize_block_ids(value, seen, depth)?;
        }
        return Ok(());
    }
    let Some(node) = value.as_object_mut() else {
        return Ok(());
    };
    if node
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| ADDRESSABLE_BLOCKS.contains(&kind))
    {
        let attrs = node
            .entry("attrs")
            .or_insert_with(|| Value::Object(Map::new()));
        if !attrs.is_object() {
            *attrs = Value::Object(Map::new());
        }
        let attrs = attrs
            .as_object_mut()
            .expect("attributes were set to an object");
        let current = attrs.get("id").and_then(Value::as_str);
        let id = match current {
            Some(id) if is_item_id(id) && seen.insert(id.to_owned()) => id.to_owned(),
            _ => loop {
                let id = Uuid::now_v7().to_string();
                if seen.insert(id.clone()) {
                    break id;
                }
            },
        };
        attrs.insert("id".to_owned(), Value::String(id));
    }
    if let Some(children) = node.get_mut("content") {
        normalize_block_ids(children, seen, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::prepare_page_content;

    #[test]
    fn keeps_unique_ids_and_replaces_copied_nested_ids() {
        let copied = "01a0e2a0-0000-7000-8000-000000000002";
        let content = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "attrs": {"id": copied}},
                {"type": "blockquote", "content": [
                    {"type": "paragraph", "attrs": {"id": copied}}
                ]}
            ]
        });
        let result = prepare_page_content(content).unwrap();
        assert_eq!(result["content"][0]["attrs"]["id"], copied);
        let replacement = result["content"][1]["content"][0]["attrs"]["id"]
            .as_str()
            .unwrap();
        assert_ne!(replacement, copied);
        assert_eq!(&replacement[14..15], "7");
        assert!(result["content"][1]["attrs"]["id"].is_string());
    }

    #[test]
    fn rejects_invalid_document_after_assigning_ids() {
        let content =
            json!({"type": "doc", "content": [{"type": "heading", "attrs": {"level": 9}}]});
        assert!(
            prepare_page_content(content)
                .unwrap_err()
                .contains("Heading level")
        );
    }
}
