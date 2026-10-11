// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate};
use serde_json::Value;

use crate::engine::document::validation::validate_document;

pub(crate) const ITEM_FORMAT_VERSION: u64 = 15;

/// Enumerate canonical item directories. An item with a malformed file is
/// returned here and rejected when its reader is called.
pub(crate) fn list_project_item_ids(root: &Path) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    visit_project_item_ids(root, |id| {
        ids.push(id.to_owned());
        Ok(())
    })?;
    ids.sort_unstable();
    Ok(ids)
}

/// Visit canonical folder identities without allocating a whole-project list.
pub(crate) fn visit_project_item_ids(
    root: &Path,
    mut visit: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let items_root = root.join("items");
    for prefix in
        fs::read_dir(&items_root).map_err(|error| format!("{}: {error}", items_root.display()))?
    {
        let prefix = prefix.map_err(|error| error.to_string())?;
        if !prefix
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
            || prefix.file_name() == "_system"
        {
            continue;
        }
        for entry in fs::read_dir(prefix.path()).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let filename = entry.file_name();
            let Some(name) = filename.to_str() else {
                continue;
            };
            if is_item_id(name) {
                visit(name)?;
            }
        }
    }
    Ok(())
}

/// Read one canonical project item. The caller supplies the expected identity
/// from the path or request so a copied or misplaced file cannot be indexed.
pub(crate) fn read_item(path: &Path, expected_id: &str) -> Result<Value, String> {
    let source = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    parse_item(&source, expected_id).map_err(|error| format!("{}: {error}", path.display()))
}

/// Resolve an item by its canonical UUID path. A missing file is distinct from
/// a malformed file so callers can implement ordinary not-found behavior.
pub(crate) fn read_project_item(root: &Path, id: &str) -> Result<Option<Value>, String> {
    if !is_item_id(id) {
        return Err(format!("Invalid Bluink item id: {id}"));
    }
    let path = root
        .join("items")
        .join(id[..2].to_ascii_lowercase())
        .join(id)
        .join("item.json");
    match fs::read(&path) {
        Ok(source) => parse_item(&source, id)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub(crate) fn parse_item(source: &[u8], expected_id: &str) -> Result<Value, String> {
    if !is_item_id(expected_id) {
        return Err(format!("Invalid Bluink item id: {expected_id}"));
    }
    let item: Value = serde_json::from_slice(source).map_err(|error| error.to_string())?;
    if item.get("format").and_then(Value::as_str) != Some("bluink.item")
        || item.get("formatVersion").and_then(Value::as_u64) != Some(ITEM_FORMAT_VERSION)
        || item.get("id").and_then(Value::as_str) != Some(expected_id)
    {
        return Err(format!(
            "Item {expected_id} has an unsupported or mismatched item file."
        ));
    }
    for field in ["revision", "createdAt", "modifiedAt"] {
        if item
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(format!("Item {expected_id} is missing {field}."));
        }
    }
    if !is_canonical_instant(item["createdAt"].as_str().unwrap_or(""))
        || !is_canonical_instant(item["modifiedAt"].as_str().unwrap_or(""))
    {
        return Err(format!(
            "Item {expected_id} has an invalid canonical timestamp."
        ));
    }
    let meta = item
        .get("meta")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Item {expected_id} has invalid metadata."))?;
    let kind = meta
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Item {expected_id} has no type."))?;
    if !matches!(
        kind,
        "page" | "workspace" | "goal" | "widget" | "query" | "setting" | "publication"
    ) {
        return Err(format!(
            "Item {expected_id} has an unsupported type: {kind}."
        ));
    }
    if !matches!(
        meta.get("state").and_then(Value::as_str),
        Some("active" | "archived")
    ) {
        return Err(format!("Item {expected_id} has an invalid state."));
    }
    let content = item
        .get("content")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("Item {expected_id} has invalid content."))?;
    if !item.get("analytics").is_some_and(Value::is_object) {
        return Err(format!("Item {expected_id} has invalid analytics."));
    }
    validate_meta(meta, expected_id)?;
    if item.get("references").is_some() {
        return Err("Page relationships must be authored in the document.".to_owned());
    }
    validate_ids(item.get("pins"), "pins")?;
    validate_tags(item.get("tags"))?;
    validate_tasks(item.get("tasks"), kind, expected_id)?;
    if kind == "page" {
        let document = content
            .get("content")
            .filter(|value| value.is_object())
            .unwrap_or_else(|| &item["content"]);
        validate_document(document)
            .map_err(|error| format!("Item {expected_id} has invalid document: {error}"))?;
        validate_block_ids(&item["content"], expected_id)?;
    }
    if kind == "goal" {
        validate_goal(&item["content"], expected_id)?;
    }
    if kind == "publication" {
        if meta
            .get("title")
            .and_then(Value::as_str)
            .is_none_or(|title| title.trim().is_empty())
        {
            return Err(format!("Publication {expected_id} needs a title."));
        }
        crate::engine::publication::validate_publication(&item["content"])?;
    }
    Ok(item)
}

fn validate_meta(meta: &serde_json::Map<String, Value>, id: &str) -> Result<(), String> {
    if let Some(color) = meta.get("color")
        && !matches!(
            color.as_str(),
            Some("none" | "blue" | "green" | "amber" | "purple" | "rose" | "cyan")
        )
    {
        return Err(format!("Item {id} must use a supported string colour."));
    }
    for field in ["start_date", "due_date", "published_on"] {
        if let Some(value) = meta.get(field)
            && !value.as_str().is_some_and(is_canonical_date)
        {
            return Err(format!("Item {id} has an invalid {field}."));
        }
    }
    if let Some(value) = meta.get("web_slug")
        && !value.as_str().is_some_and(|slug| {
            !slug.is_empty()
                && slug.len() <= 80
                && slug
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !slug.starts_with('-')
                && !slug.ends_with('-')
                && !slug.contains("--")
        })
    {
        return Err(format!("Item {id} has an invalid web_slug."));
    }
    if let Some(value) = meta.get("completed_at")
        && !value.as_str().is_some_and(is_canonical_instant)
    {
        return Err(format!("Item {id} has an invalid completion timestamp."));
    }
    Ok(())
}

fn validate_ids(value: Option<&Value>, field: &str) -> Result<(), String> {
    if let Some(value) = value {
        let values = value
            .as_array()
            .ok_or_else(|| format!("Item {field} must be an array of UUIDs."))?;
        if !values
            .iter()
            .all(|value| value.as_str().is_some_and(is_item_id))
        {
            return Err(format!("Item {field} contains an invalid UUID."));
        }
    }
    Ok(())
}

fn validate_tags(value: Option<&Value>) -> Result<(), String> {
    if let Some(value) = value {
        let values = value
            .as_array()
            .ok_or("Item tags must be an array of keys.")?;
        if !values
            .iter()
            .all(|value| value.as_str().is_some_and(is_tag_key))
        {
            return Err("Item tags contain an invalid tag key.".to_owned());
        }
    }
    Ok(())
}

fn validate_tasks(value: Option<&Value>, kind: &str, id: &str) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    let tasks = value.as_array().ok_or("Item tasks must be an array.")?;
    if !tasks.is_empty() && kind != "page" {
        return Err(format!("Only a page can own tasks: {id}"));
    }
    let mut seen = std::collections::HashSet::new();
    for task in tasks {
        let task_id = task.get("id").and_then(Value::as_str).unwrap_or("");
        if !is_item_id(task_id) || !seen.insert(task_id) {
            return Err("Note tasks need unique UUIDs.".to_owned());
        }
        let text = task
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() || text.encode_utf16().count() > 240 {
            return Err("Enter task text of at most 240 characters.".to_owned());
        }
        if !task
            .get("createdAt")
            .and_then(Value::as_str)
            .is_some_and(is_canonical_instant)
            || task
                .get("completedAt")
                .is_some_and(|value| !value.as_str().is_some_and(is_canonical_instant))
        {
            return Err("Note task timestamps must be canonical UTC instants.".to_owned());
        }
        if !task.as_object().is_some_and(|object| {
            object
                .keys()
                .all(|key| matches!(key.as_str(), "id" | "text" | "createdAt" | "completedAt"))
        }) {
            return Err("Note tasks contain only identity, text and timestamps.".to_owned());
        }
    }
    Ok(())
}

fn validate_goal(content: &Value, id: &str) -> Result<(), String> {
    let _: super::types::WritingGoalContent = serde_json::from_value(content.clone())
        .map_err(|error| format!("Writing goal {id} has invalid configuration: {error}"))?;
    let start = content
        .get("startDate")
        .and_then(Value::as_str)
        .unwrap_or("");
    let end = content.get("endDate").and_then(Value::as_str).unwrap_or("");
    let scope = content.get("scope").and_then(Value::as_str).unwrap_or("");
    let start_date = NaiveDate::parse_from_str(start, "%Y-%m-%d").ok();
    let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d").ok();
    let valid = content.get("kind").and_then(Value::as_str) == Some("writing_challenge")
        && matches!(scope, "all_notes" | "view")
        && is_canonical_date(start)
        && is_canonical_date(end)
        && start_date.zip(end_date).is_some_and(|(start, end)| {
            let days = (end - start).num_days();
            (0..=365).contains(&days)
        })
        && content
            .get("dailyWordTarget")
            .is_some_and(positive_safe_integer)
        && content
            .get("totalWordTarget")
            .is_none_or(positive_safe_integer)
        && match scope {
            "view" => {
                content
                    .get("viewId")
                    .and_then(Value::as_str)
                    .is_some_and(is_item_id)
                    && content.get("query").is_some_and(Value::is_string)
            }
            "all_notes" => content.get("viewId").is_none() && content.get("query").is_none(),
            _ => false,
        };
    if valid {
        Ok(())
    } else {
        Err(format!("Writing goal {id} has invalid configuration."))
    }
}

fn validate_block_ids(value: &Value, item_id: &str) -> Result<(), String> {
    fn visit(value: &Value, seen: &mut std::collections::HashSet<String>) -> bool {
        if let Some(array) = value.as_array() {
            return array.iter().all(|value| visit(value, seen));
        }
        let Some(node) = value.as_object() else {
            return true;
        };
        if node
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "paragraph"
                        | "heading"
                        | "code_block"
                        | "horizontal_rule"
                        | "blockquote"
                        | "list"
                        | "listItem"
                        | "image"
                        | "box"
                        | "boxHeading"
                        | "figure"
                        | "table"
                        | "table_row"
                        | "table_cell"
                        | "table_header"
                        | "embed"
                )
            })
        {
            let id = node
                .get("attrs")
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str);
            let Some(id) = id.filter(|id| is_item_id(id)) else {
                return false;
            };
            if !seen.insert(id.to_owned()) {
                return false;
            }
        }
        node.get("content")
            .is_none_or(|children| visit(children, seen))
    }
    if visit(value, &mut std::collections::HashSet::new()) {
        Ok(())
    } else {
        Err(format!(
            "Item {item_id} has a missing, invalid, or duplicate block UUID."
        ))
    }
}

fn positive_safe_integer(value: &Value) -> bool {
    value
        .as_i64()
        .is_some_and(|value| (1..=9_007_199_254_740_991).contains(&value))
}

pub(crate) fn is_canonical_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|index| bytes[*index].is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

pub(crate) fn is_canonical_instant(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes.iter().enumerate().all(|(index, byte)| {
            [4, 7, 10, 13, 16, 19, 23].contains(&index) || byte.is_ascii_digit()
        })
        && matches!(bytes.get(4), Some(b'-'))
        && matches!(bytes.get(7), Some(b'-'))
        && matches!(bytes.get(10), Some(b'T'))
        && matches!(bytes.get(13), Some(b':'))
        && matches!(bytes.get(16), Some(b':'))
        && matches!(bytes.get(19), Some(b'.'))
        && matches!(bytes.get(23), Some(b'Z'))
        && DateTime::parse_from_rfc3339(value).is_ok()
}

pub(crate) fn is_tag_key(value: &str) -> bool {
    value.split('/').enumerate().all(|(index, segment)| {
        is_snake_token(segment)
            || (index > 0
                && (1..=4).contains(&segment.len())
                && segment.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn is_snake_token(value: &str) -> bool {
    let mut segments = value.split('_');
    segments.next().is_some_and(|first| {
        first.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && first
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    }) && segments.all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

#[must_use]
pub(crate) fn is_item_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
        && (b'1'..=b'8').contains(&bytes[14])
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{list_project_item_ids, parse_item, read_item};

    const ID: &str = "01a0e2a0-0000-7000-8000-000000000001";

    #[test]
    fn enumerates_canonical_item_directories_in_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let ids = list_project_item_ids(&root).unwrap();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.contains(&ID.to_owned()));
        assert!(ids.iter().all(|id| super::is_item_id(id)));
    }

    #[test]
    fn reads_portable_page_item() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("test-fixtures/pdf-project/items/01/{ID}/item.json"));
        let item = read_item(&path, ID).unwrap();
        assert_eq!(item["id"], ID);
    }

    #[test]
    fn rejects_mismatched_identity_and_invalid_document() {
        let mut item = json!({
            "format": "bluink.item",
            "formatVersion": 15,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"content": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]}},
            "analytics": {}
        });
        let source = serde_json::to_vec(&item).unwrap();
        assert!(parse_item(&source, ID).is_ok());
        assert!(parse_item(&source, "01a0e2a0-0000-7000-8000-000000000002").is_err());
        item["content"]["content"]["content"][0]["type"] = json!("widget");
        assert!(parse_item(&serde_json::to_vec(&item).unwrap(), ID).is_err());
    }

    #[test]
    fn accepts_direct_document_content() {
        let item = json!({
            "format": "bluink.item",
            "formatVersion": 15,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]},
            "analytics": {}
        });
        assert!(parse_item(&serde_json::to_vec(&item).unwrap(), ID).is_ok());
    }

    #[test]
    fn rejects_invalid_item_dates_tags_tasks_and_block_ids() {
        let base = json!({
            "format": "bluink.item",
            "formatVersion": 15,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]},
            "analytics": {}
        });
        let mut invalid_date = base.clone();
        invalid_date["createdAt"] = json!("2026-02-30T11:37:24.017Z");
        assert!(
            parse_item(&serde_json::to_vec(&invalid_date).unwrap(), ID)
                .unwrap_err()
                .contains("timestamp")
        );

        let mut invalid_tag = base.clone();
        invalid_tag["tags"] = json!(["not A tag"]);
        assert!(
            parse_item(&serde_json::to_vec(&invalid_tag).unwrap(), ID)
                .unwrap_err()
                .contains("tag")
        );

        let mut invalid_task = base.clone();
        invalid_task["tasks"] = json!([{
            "id": "01a0e2a0-0000-7000-8000-000000000003",
            "text": "Review",
            "createdAt": "2026-09-27T11:37:24.017Z",
            "anchor": {"kind": "block", "blockId": "bad"}
        }]);
        assert!(
            parse_item(&serde_json::to_vec(&invalid_task).unwrap(), ID)
                .unwrap_err()
                .contains("identity, text and timestamps")
        );

        let mut invalid_block = base;
        invalid_block["content"]["content"][0]["attrs"]["id"] = json!("bad");
        assert!(
            parse_item(&serde_json::to_vec(&invalid_block).unwrap(), ID)
                .unwrap_err()
                .contains("block UUID")
        );
    }
}
