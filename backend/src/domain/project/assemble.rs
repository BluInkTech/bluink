// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::document::{content::prepare_page_content, projection::project_document};
use crate::domain::project::identity::is_item_id;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Effectful callers capture these values once before canonical preparation.
pub(crate) struct AuthoringValues<'a> {
    pub(crate) timestamp: &'a str,
    pub(crate) ids: &'a mut dyn Iterator<Item = String>,
}

impl AuthoringValues<'_> {
    pub(crate) fn next_id(&mut self) -> Result<String, String> {
        let id = self
            .ids
            .next()
            .ok_or_else(|| "Missing supplied authoring identity".to_owned())?;
        if !is_item_id(&id) {
            return Err("Invalid supplied authoring identity".into());
        }
        Ok(id)
    }
}

/// An RPC write owns its content and can move a large document into preparation.
pub(crate) fn assemble_owned_item(
    input: Value,
    existing: Option<&Value>,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<Value, String> {
    let Value::Object(mut input) = input else {
        return Err("Item write input must be an object.".into());
    };
    let content = input.remove("content");
    assemble(&input, content, existing, timezone, values)
}

fn assemble(
    input: &serde_json::Map<String, Value>,
    content: Option<Value>,
    existing: Option<&Value>,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<Value, String> {
    let id = match input.get("id") {
        Some(Value::String(id)) if is_item_id(id) => id.clone(),
        Some(_) => return Err("Invalid Bluink item id.".to_owned()),
        None => values.next_id()?,
    };
    let provided_meta = input
        .get("meta")
        .and_then(Value::as_object)
        .ok_or("Item metadata must be an object.")?;
    let mut meta = existing
        .and_then(|item| item["meta"].as_object())
        .cloned()
        .unwrap_or_default();
    meta.extend(
        provided_meta
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    let state = provided_meta
        .get("state")
        .filter(|value| !value.is_null())
        .or_else(|| existing.and_then(|item| item["meta"].get("state")))
        .cloned()
        .unwrap_or_else(|| json!("active"));
    meta.insert("state".to_owned(), state);
    let kind = meta
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Item has no type.")?;
    let content = content
        .or_else(|| existing.and_then(|item| item.get("content")).cloned())
        .ok_or("Item content is missing.")?;
    let content = if matches!(kind, "page" | "template") {
        prepare_page_content(content, values.ids)?
    } else {
        content
    };
    let text = project_document(crate::domain::document::document_root(&content)).text;
    let content_hash = if kind == "task" {
        hash_text(&content.to_string())
    } else {
        hash_text(&text)
    };
    let timestamp = values.timestamp;
    let mut next = json!({
        "format": "bluink.item",
        "formatVersion": crate::domain::project::item::ITEM_FORMAT_VERSION,
        "id": id,
        "revision": values.next_id()?,
        "createdAt": existing.map_or_else(|| json!(timestamp), |item| item["createdAt"].clone()),
        "modifiedAt": timestamp,
        "meta": meta,
        "content": content,
        "analytics": {
            "analyzerVersion": 1,
            "timezone": timezone,
            "current": {"contentHash": content_hash, "wordCount": count_words(&text)}
        }
    });
    if input.get("references").is_some() {
        return Err("Page relationships must be authored in the document.".to_owned());
    }
    for field in ["tags", "pins"] {
        if let Some(values) = normalized_strings(
            input
                .get(field)
                .or_else(|| existing.and_then(|item| item.get(field))),
        )? {
            next[field] = Value::Array(values);
        }
    }
    if input.contains_key("tasks") {
        return Err("Tasks are independent items; use task mutations.".to_owned());
    }
    Ok(next)
}

fn normalized_strings(value: Option<&Value>) -> Result<Option<Vec<Value>>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or("Item relation values must be arrays.")?;
    if values.is_empty() {
        return Ok(None);
    }
    let mut strings: Vec<&str> = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or("Item relation values must be strings.")
        })
        .collect::<Result<_, _>>()?;
    strings.sort_unstable();
    strings.dedup();
    Ok(Some(
        strings
            .into_iter()
            .map(|value| Value::String(value.to_owned()))
            .collect(),
    ))
}

#[expect(
    clippy::expect_used,
    reason = "The cached regular expression is a compile-time literal covered by word-count tests."
)]
pub(crate) fn count_words(text: &str) -> usize {
    static WORDS: OnceLock<Regex> = OnceLock::new();
    WORDS
        .get_or_init(|| {
            Regex::new(r"[\p{L}\p{N}]+(?:['’][\p{L}\p{N}]+)*").expect("valid word pattern")
        })
        .find_iter(text)
        .count()
}

pub(crate) fn hash_text(text: &str) -> String {
    let mut hash = 0x811c_9dc5_u32;
    for unit in text.encode_utf16() {
        hash = (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// Borrowed candidates use the same pure preparation as owned authoring input.
pub(crate) fn assemble_item(
    input: &Value,
    existing: Option<&Value>,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<Value, String> {
    assemble_owned_item(input.clone(), existing, timezone, values)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{count_words, hash_text};
    fn assemble_item(
        input: &serde_json::Value,
        existing: Option<&serde_json::Value>,
        timezone: &str,
    ) -> Result<serde_json::Value, String> {
        let mut ids = (1..1024).map(|i| format!("11111111-1111-7111-8111-{i:012x}"));
        let mut values = super::AuthoringValues {
            timestamp: "2026-11-05T12:00:00.000Z",
            ids: &mut ids,
        };
        super::assemble_item(input, existing, timezone, &mut values)
    }

    #[test]
    fn matches_javascript_word_and_utf16_hash_rules() {
        assert_eq!(count_words("A Māori title, two words and don't split."), 8);
        assert_eq!(count_words("🌌 stars' light"), 2);
        assert_eq!(hash_text(""), "811c9dc5");
        assert_eq!(hash_text("😀"), "cb31c4b8");
    }

    #[test]
    fn assembles_a_page_and_inherits_optional_relations() {
        let existing = json!({
            "createdAt":"2026-09-01T00:00:00.000Z",
            "meta":{"type":"page","title":"First","state":"active"},
            "tags":["z","a"]
        });
        let next = assemble_item(&json!({
            "id":"01a0e2a0-0000-7000-8000-000000000001",
            "meta":{"title":"Second"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A sample"}]}]}}
        }), Some(&existing), "Europe/London").unwrap();
        assert_eq!(next["meta"]["title"], "Second");
        assert_eq!(next["meta"]["type"], "page");
        assert_eq!(next["tags"], json!(["a", "z"]));
        assert!(next.get("tasks").is_none());
        assert_eq!(next["analytics"]["current"]["wordCount"], 2);
        assert!(next["content"]["document"]["content"][0]["attrs"]["id"].is_string());
    }
}
