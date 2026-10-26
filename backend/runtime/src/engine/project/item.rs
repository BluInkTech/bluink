// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::{Path, PathBuf};

use super::identity::{ItemId, is_item_id};
use chrono::{DateTime, NaiveDate};
use serde_json::Value;

use crate::engine::document::validation::validate_document;

pub(crate) const ITEM_FORMAT_VERSION: u64 = 17;

#[derive(Debug)]
pub(crate) enum ItemReadError {
    InvalidIdentity(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidData {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for ItemReadError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentity(message) => output.write_str(message),
            Self::Io { path, source } => write!(output, "{}: {source}", path.display()),
            Self::Json { path, source } => write!(output, "{}: {source}", path.display()),
            Self::InvalidData { path, message } => write!(output, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for ItemReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidIdentity(_) | Self::InvalidData { .. } => None,
        }
    }
}

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
pub(crate) fn read_item(path: &Path, expected_id: &str) -> Result<Value, ItemReadError> {
    let source = fs::read(path).map_err(|source| ItemReadError::Io {
        path: path.to_owned(),
        source,
    })?;
    let item = serde_json::from_slice(&source).map_err(|source| ItemReadError::Json {
        path: path.to_owned(),
        source,
    })?;
    validate_item(&item, expected_id).map_err(|message| ItemReadError::InvalidData {
        path: path.to_owned(),
        message,
    })?;
    Ok(item)
}

/// Resolve an item by its canonical UUID path. A missing file is distinct from
/// a malformed file so callers can implement ordinary not-found behavior.
pub(crate) fn read_project_item(root: &Path, id: &str) -> Result<Option<Value>, ItemReadError> {
    let identity = ItemId::try_from(id).map_err(ItemReadError::InvalidIdentity)?;
    let path = identity.path(root);
    match read_item(&path, id) {
        Ok(item) => Ok(Some(item)),
        Err(ItemReadError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
fn parse_item(source: &[u8], expected_id: &str) -> Result<Value, String> {
    let item = serde_json::from_slice(source).map_err(|error| error.to_string())?;
    validate_item(&item, expected_id)?;
    Ok(item)
}

pub(crate) fn validate_item(item: &Value, expected_id: &str) -> Result<(), String> {
    let envelope = super::envelope::ItemEnvelope::read(item, expected_id)?;
    let meta = envelope.metadata;
    let kind = envelope.kind.as_str();
    let content = envelope.content;
    validate_meta(meta, expected_id)?;
    if item.get("references").is_some() {
        return Err("Page relationships must be authored in the document.".to_owned());
    }
    validate_ids(item.get("pins"), "pins")?;
    validate_tags(item.get("tags"))?;
    if item.get("tasks").is_some() {
        return Err("Tasks must be independent item records.".to_owned());
    }
    if kind == "task" {
        super::task::validate_content(content)?;
    }
    // A template is validated as the page it starts.
    if matches!(kind, "page" | "template") {
        let document = crate::engine::document::document_root(content);
        validate_document(document)
            .map_err(|error| format!("Item {expected_id} has invalid document: {error}"))?;
        validate_block_ids(&item["content"], expected_id)?;
    }
    if kind == "template" {
        super::template::validate(item, expected_id)?;
    }
    if kind == "goal" {
        validate_goal(&item["content"], expected_id)?;
    }
    if kind == "workspace" {
        crate::engine::project::workspace::validate_content(&item["content"])?;
    }
    if matches!(kind, "query" | "widget" | "goal")
        && let Some(value) = content.get("query")
    {
        validate_query_value(value, expected_id)?;
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
    super::named_items::validate_identity(item, expected_id)?;
    if matches!(kind, "tag" | "label") {
        super::facets::validate_content(kind, content)?;
    }
    if kind == "property" {
        super::properties::validate_content(content)?;
    }
    if kind == "snippet" {
        super::snippets::validate_content(item)?;
    }
    Ok(())
}

fn validate_query_value(value: &Value, id: &str) -> Result<(), String> {
    let query: super::query::QueryDocument = serde::Deserialize::deserialize(value)
        .map_err(|error| format!("Item {id} has an invalid structured query: {error}"))?;
    if query.version != 1 {
        return Err(format!("Item {id} needs query document version 1."));
    }
    Ok(())
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
    validate_appearance(meta, id)?;
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

/// An icon or cover image must be one of the item's own uploads, so it moves,
/// exports and is removed with the item.
fn own_asset(url: &str, id: &str) -> bool {
    url.strip_prefix("/assets/")
        .and_then(|rest| rest.strip_prefix(id))
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(super::asset::valid_filename)
}

fn validate_appearance(meta: &serde_json::Map<String, Value>, id: &str) -> Result<(), String> {
    use super::types::{ItemCover, ItemIcon};
    // A null value unsets the field, as merged metadata writes cannot delete keys.
    if let Some(value) = meta.get("icon").filter(|value| !value.is_null()) {
        let icon: ItemIcon = serde::Deserialize::deserialize(value)
            .map_err(|error| format!("Item {id} has an invalid icon: {error}"))?;
        let valid = match &icon {
            ItemIcon::Symbol { name } => {
                !name.is_empty()
                    && name.len() <= 40
                    && name.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            }
            ItemIcon::Emoji { value } => {
                !value.is_empty()
                    && value.len() <= 32
                    && !value
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
            }
            ItemIcon::Image { url } => own_asset(url, id),
        };
        if !valid {
            return Err(format!("Item {id} has an invalid icon."));
        }
    }
    // A null value unsets the field, as merged metadata writes cannot delete keys.
    if let Some(value) = meta.get("cover").filter(|value| !value.is_null()) {
        let cover: ItemCover = serde::Deserialize::deserialize(value)
            .map_err(|error| format!("Item {id} has an invalid cover: {error}"))?;
        if let ItemCover::Image { url, position } = &cover
            && (!own_asset(url, id) || *position > 100)
        {
            return Err(format!("Item {id} has an invalid cover image."));
        }
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

fn validate_goal(content: &Value, id: &str) -> Result<(), String> {
    let _: super::types::WritingGoalContent = serde::Deserialize::deserialize(content)
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
                    && content.get("query").is_some_and(|query| {
                        serde_json::from_value::<crate::engine::project::query::QueryDocument>(
                            query.clone(),
                        )
                        .is_ok()
                    })
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
        super::properties::is_snake_case(segment)
            || (index > 0
                && (1..=4).contains(&segment.len())
                && segment.bytes().all(|b| b.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{list_project_item_ids, parse_item, read_item};

    const ID: &str = "01a0e2a0-0000-7000-8000-000000000001";

    #[test]
    fn read_errors_preserve_path_and_distinguish_io_json_and_invalid_data() {
        use std::error::Error;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("item.json");
        let missing = read_item(&path, ID).unwrap_err();
        assert!(matches!(
            &missing,
            super::ItemReadError::Io { path: actual, source }
                if actual == &path && source.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(
            missing
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .is_some()
        );

        std::fs::write(&path, b"{invalid").unwrap();
        let malformed = read_item(&path, ID).unwrap_err();
        assert!(matches!(
            &malformed,
            super::ItemReadError::Json { path: actual, .. } if actual == &path
        ));
        assert!(
            malformed
                .source()
                .unwrap()
                .downcast_ref::<serde_json::Error>()
                .is_some()
        );

        std::fs::write(&path, b"{}").unwrap();
        let invalid = read_item(&path, ID).unwrap_err();
        assert!(matches!(
            &invalid,
            super::ItemReadError::InvalidData { path: actual, message }
                if actual == &path && message.contains("unsupported or mismatched")
        ));
        assert!(invalid.source().is_none());
        root.close().unwrap();
    }

    #[test]
    fn enumerates_canonical_item_directories_in_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let ids = list_project_item_ids(&root).unwrap();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.contains(&ID.to_owned()));
        assert!(ids.iter().all(|id| super::is_item_id(id)));
    }

    #[test]
    fn envelope_validation_preserves_extensions_and_absent_fields() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("test-fixtures/pdf-project/items/01/{ID}/item.json"));
        let mut original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        original["customExtension"] = json!({"nested": [null, {"value": 42}]});
        original["meta"]["custom_property"] = json!({"text": "Preserve me", "unset": null});
        original.as_object_mut().unwrap().remove("pins");
        let parsed = parse_item(&serde_json::to_vec(&original).unwrap(), ID).unwrap();
        assert_eq!(parsed, original);
        assert!(parsed.get("pins").is_none());
        let envelope = super::super::envelope::ItemEnvelope::read(&parsed, ID).unwrap();
        assert_eq!(envelope.kind, crate::engine::project::types::ItemType::Page);
        assert_eq!(envelope.revision, parsed["revision"].as_str().unwrap());
        assert_eq!(
            envelope.metadata["custom_property"],
            original["meta"]["custom_property"]
        );
        assert_eq!(envelope.content, &parsed["content"]);
    }

    #[test]
    fn icons_and_covers_take_canonical_shapes_and_own_uploads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("test-fixtures/pdf-project/items/01/{ID}/item.json"));
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let with = |icon: serde_json::Value, cover: serde_json::Value| {
            let mut item = original.clone();
            item["meta"]["icon"] = icon;
            item["meta"]["cover"] = cover;
            parse_item(&serde_json::to_vec(&item).unwrap(), ID)
        };
        let own = format!("/assets/{ID}/cover.png");
        let sea = json!({"kind": "gradient", "name": "sea"});
        for (icon, cover) in [
            (
                json!({"kind": "symbol", "name": "book-open"}),
                json!({"kind": "gradient", "name": "dusk"}),
            ),
            (
                json!({"kind": "emoji", "value": "\u{1f9ed}"}),
                json!({"kind": "image", "url": own, "position": 40}),
            ),
            (json!({"kind": "image", "url": own}), sea.clone()),
        ] {
            with(icon, cover).unwrap();
        }
        // Null unsets both.
        with(json!(null), json!(null)).unwrap();
        let other = "/assets/01a0e2a0-0000-7000-8000-000000000002/cover.png";
        let book = json!({"kind": "symbol", "name": "book"});
        for (icon, cover) in [
            (json!({"kind": "symbol", "name": "Book Open"}), sea.clone()),
            (json!({"kind": "emoji", "value": "two words"}), sea.clone()),
            (json!({"kind": "image", "url": other}), sea.clone()),
            (
                json!({"kind": "image", "url": "https://example.com/a.png"}),
                sea.clone(),
            ),
            (
                json!({"kind": "symbol", "name": "book", "color": "blue"}),
                sea,
            ),
            (book.clone(), json!({"kind": "gradient", "name": "neon"})),
            (
                book.clone(),
                json!({"kind": "image", "url": own, "position": 101}),
            ),
            (book, json!({"kind": "image", "url": other, "position": 50})),
        ] {
            assert!(with(icon.clone(), cover.clone()).is_err(), "{icon} {cover}");
        }
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
            "formatVersion": 17,
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
            "formatVersion": 17,
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
            "formatVersion": 17,
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
                .contains("Tasks must be independent item records")
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
