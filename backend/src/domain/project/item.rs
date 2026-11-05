// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::identity::is_item_id;
use crate::domain::document::validation::validate_canonical_document;
use chrono::{DateTime, NaiveDate};
use serde::Deserialize as _;
use serde_json::Value;
#[cfg(test)]
use std::path::Path;

pub(crate) const ITEM_FORMAT_VERSION: u64 = 20;
/// The fields a page or template's `content` may hold.
const PAGE_CONTENT_KEYS: [&str; 2] = ["document", "ignoredFindings"];

/// Enumerate canonical identities in one snapshot without opening any bodies.
#[cfg(test)]
pub(crate) fn list_project_item_ids(root: &Path) -> Result<Vec<String>, String> {
    crate::store::Store::shared(root)
        .and_then(|store| {
            store.read(|tx| {
                Ok(tx
                    .prepare("SELECT uuid FROM item ORDER BY uuid")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
        })
        .map_err(|error| error.to_string())
}

pub(crate) fn validate_item(item: &Value, expected_id: &str) -> Result<(), String> {
    let envelope = super::envelope::ItemEnvelope::read(item, expected_id)?;
    let meta = envelope.metadata;
    let kind = envelope.kind.as_str();
    let content = envelope.content;
    if kind == "conflict" {
        crate::domain::merge::decisions::validate_item(item, expected_id)?;
    }
    validate_metadata(meta, expected_id)?;
    if item.get("references").is_some() {
        return Err("Page relationships must be authored in the document.".to_owned());
    }
    validate_memberships(item)?;
    if item.get("tasks").is_some() {
        return Err("Tasks must be independent item records.".to_owned());
    }
    if kind == "task" {
        super::task::validate_content(content)?;
    }
    // A template is validated as the page it starts.
    if matches!(kind, "page" | "template") {
        // Page content is the document and the page's ignored findings.
        validate_page_fields(content, expected_id)?;
        let document = crate::domain::document::document_root(content);
        validate_canonical_document(document)
            .map_err(|error| format!("Item {expected_id} has invalid document: {error}"))?;
    }
    if kind == "template" {
        super::template::validate(item, expected_id)?;
    }
    if kind == "goal" {
        validate_goal(&item["content"], expected_id)?;
    }
    if kind == "workspace" {
        crate::domain::project::workspace::validate_content(&item["content"])?;
    }
    if matches!(kind, "query" | "widget" | "goal")
        && let Some(value) = content.get("query")
    {
        validate_query_value(value, expected_id)?;
    }
    if kind == "publication" {
        crate::domain::publication::validate_publication(&item["content"])?;
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

pub(crate) fn validate_memberships(item: &Value) -> Result<(), String> {
    validate_ids(item.get("pins"), "pins")?;
    validate_tags(item.get("tags"))
}

pub(crate) fn validate_page_fields(content: &Value, id: &str) -> Result<(), String> {
    if let Some(key) = content.as_object().and_then(|fields| {
        fields
            .keys()
            .find(|key| !PAGE_CONTENT_KEYS.contains(&key.as_str()))
    }) {
        return Err(format!("Item {id} has unsupported content field {key}."));
    }
    validate_ignored_findings(content, id)
}

fn validate_query_value(value: &Value, id: &str) -> Result<(), String> {
    let query: super::query::QueryDocument = serde::Deserialize::deserialize(value)
        .map_err(|error| format!("Item {id} has an invalid structured query: {error}"))?;
    if query.version != 1 {
        return Err(format!("Item {id} needs query document version 1."));
    }
    Ok(())
}

pub(crate) fn validate_metadata(
    meta: &serde_json::Map<String, Value>,
    id: &str,
) -> Result<(), String> {
    // Generic writes carry metadata as an untyped map; the envelope fields
    // must still have the types readers and the editor's drafts expect.
    super::types::Metadata::deserialize(Value::Object(meta.clone()))
        .map_err(|error| format!("Item {id} has invalid metadata: {error}"))?;
    if meta.get("type").and_then(Value::as_str) == Some("publication")
        && meta
            .get("title")
            .and_then(Value::as_str)
            .is_none_or(|title| title.trim().is_empty())
    {
        return Err(format!("Publication {id} needs a title."));
    }
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
                        serde_json::from_value::<crate::domain::project::query::QueryDocument>(
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

/// The most writing findings one note may ignore.
const MAX_IGNORED_FINDINGS: usize = 2_000;
/// The longest fingerprint of an ignored finding, in characters.
const MAX_IGNORED_FINDING_LENGTH: usize = 400;

/// Writing findings a page ignores belong to the page: readable fingerprints
/// the editor compares with each check's results, kept beside the document so
/// edits and undo never touch them.
fn validate_ignored_findings(content: &Value, item_id: &str) -> Result<(), String> {
    let Some(value) = content.get("ignoredFindings") else {
        return Ok(());
    };
    let Some(findings) = value.as_array() else {
        return Err(format!("Item {item_id} ignored findings must be a list."));
    };
    if findings.len() > MAX_IGNORED_FINDINGS {
        return Err(format!(
            "Item {item_id} ignores more than {MAX_IGNORED_FINDINGS} writing findings."
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for finding in findings {
        let Some(text) = finding.as_str() else {
            return Err(format!(
                "Item {item_id} has an ignored finding that is not text."
            ));
        };
        if text.trim().is_empty() || text.chars().count() > MAX_IGNORED_FINDING_LENGTH {
            return Err(format!(
                "Item {item_id} has an ignored finding that is empty or longer than {MAX_IGNORED_FINDING_LENGTH} characters."
            ));
        }
        if !seen.insert(text) {
            return Err(format!(
                "Item {item_id} ignores the same writing finding twice."
            ));
        }
    }
    Ok(())
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
    #[cfg(test)]
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        MAX_IGNORED_FINDING_LENGTH, MAX_IGNORED_FINDINGS, list_project_item_ids, validate_item,
    };

    const ID: &str = "01a0e2a0-0000-7000-8000-000000000001";

    #[test]
    fn enumerates_canonical_item_directories_in_order() {
        let (_fixture, root) = crate::domain::test_support::portable_fixture();
        let ids = list_project_item_ids(&root).unwrap();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.contains(&ID.to_owned()));
        assert!(ids.iter().all(|id| super::is_item_id(id)));
    }

    #[test]
    fn envelope_validation_preserves_extensions_and_absent_fields() {
        let path = crate::domain::project::identity::ItemId::try_from(ID)
            .unwrap()
            .path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project"));
        let mut original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        original["customExtension"] = json!({"nested": [null, {"value": 42}]});
        original["meta"]["custom_property"] = json!({"text": "Preserve me", "unset": null});
        original.as_object_mut().unwrap().remove("pins");
        validate_item(&original, ID).unwrap();
        let envelope = super::super::envelope::ItemEnvelope::read(&original, ID).unwrap();
        assert_eq!(envelope.kind, crate::domain::project::types::ItemType::Page);
        assert_eq!(envelope.revision, original["revision"].as_str().unwrap());
        assert_eq!(
            envelope.metadata["custom_property"],
            original["meta"]["custom_property"]
        );
        assert_eq!(envelope.content, &original["content"]);
    }

    #[test]
    fn icons_and_covers_take_canonical_shapes_and_own_uploads() {
        let path = crate::domain::project::identity::ItemId::try_from(ID)
            .unwrap()
            .path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project"));
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let with = |icon: serde_json::Value, cover: serde_json::Value| {
            let mut item = original.clone();
            item["meta"]["icon"] = icon;
            item["meta"]["cover"] = cover;
            validate_item(&item, ID)
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
    fn rejects_mismatched_identity_and_invalid_document() {
        let mut item = json!({
            "format": "bluink.item",
            "formatVersion": 20,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]}},
            "analytics": {}
        });
        assert!(validate_item(&item, ID).is_ok());
        assert!(validate_item(&item, "01a0e2a0-0000-7000-8000-000000000002").is_err());
        item["content"]["document"]["content"][0]["type"] = json!("widget");
        assert!(validate_item(&item, ID).is_err());
    }

    #[test]
    fn keeps_the_document_under_content_document_only() {
        let mut item = json!({
            "format": "bluink.item",
            "formatVersion": 20,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]}},
            "analytics": {}
        });
        let parse = |item: &Value| validate_item(item, ID);
        assert!(parse(&item).is_ok());
        // Neither the document in place of content nor the old field name.
        let document = item["content"]["document"].clone();
        item["content"] = document.clone();
        assert!(parse(&item).is_err());
        item["content"] = json!({"content": document});
        assert!(
            parse(&item)
                .unwrap_err()
                .contains("unsupported content field content")
        );
    }

    #[test]
    fn keeps_valid_ignored_findings_beside_the_document_and_rejects_bad_ones() {
        let page = |findings: Value| {
            json!({
                "format": "bluink.item",
                "formatVersion": 20,
                "id": ID,
                "revision": ID,
                "createdAt": "2026-09-27T11:37:24.017Z",
                "modifiedAt": "2026-09-27T11:37:24.017Z",
                "meta": {"type": "page", "state": "active"},
                "content": {
                    "document": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]},
                    "ignoredFindings": findings
                },
                "analytics": {}
            })
        };
        let parse = |item: Value| validate_item(&item, ID);
        assert!(parse(page(json!([r#"["SpellCheck","Xander","we said"]"#]))).is_ok());
        assert!(parse(page(json!([]))).is_ok());
        for (findings, message) in [
            (json!("Xander"), "must be a list"),
            (json!([3]), "not text"),
            (json!([" "]), "empty or longer"),
            (
                json!(["x".repeat(MAX_IGNORED_FINDING_LENGTH + 1)]),
                "empty or longer",
            ),
            (json!(["same", "same"]), "same writing finding twice"),
            (
                Value::Array(
                    (0..=MAX_IGNORED_FINDINGS)
                        .map(|index| json!(index.to_string()))
                        .collect(),
                ),
                "more than",
            ),
        ] {
            let error = parse(page(findings)).unwrap_err();
            assert!(error.contains(message), "{message}: {error}");
        }
    }

    #[test]
    fn rejects_invalid_item_dates_tags_tasks_and_block_ids() {
        let base = json!({
            "format": "bluink.item",
            "formatVersion": 20,
            "id": ID,
            "revision": ID,
            "createdAt": "2026-09-27T11:37:24.017Z",
            "modifiedAt": "2026-09-27T11:37:24.017Z",
            "meta": {"type": "page", "state": "active"},
            "content": {"document": {"type": "doc", "content": [{"type": "paragraph", "attrs": {"id": "01a0e2a0-0000-7000-8000-000000000002"}}]}},
            "analytics": {}
        });
        let mut invalid_date = base.clone();
        invalid_date["createdAt"] = json!("2026-02-30T11:37:24.017Z");
        assert!(
            validate_item(&invalid_date, ID)
                .unwrap_err()
                .contains("timestamp")
        );

        let mut invalid_tag = base.clone();
        invalid_tag["tags"] = json!(["not A tag"]);
        assert!(validate_item(&invalid_tag, ID).unwrap_err().contains("tag"));

        let mut invalid_task = base.clone();
        invalid_task["tasks"] = json!([{
            "id": "01a0e2a0-0000-7000-8000-000000000003",
            "text": "Review",
            "createdAt": "2026-09-27T11:37:24.017Z",
            "anchor": {"kind": "block", "blockId": "bad"}
        }]);
        assert!(
            validate_item(&invalid_task, ID)
                .unwrap_err()
                .contains("Tasks must be independent item records")
        );

        let mut invalid_block = base;
        invalid_block["content"]["document"]["content"][0]["attrs"]["id"] = json!("bad");
        assert!(
            validate_item(&invalid_block, ID)
                .unwrap_err()
                .contains("block UUID")
        );
    }
}
