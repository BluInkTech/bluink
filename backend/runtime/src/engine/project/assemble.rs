// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;
use std::sync::OnceLock;

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::engine::document::content::prepare_page_content;
use crate::engine::document::projection::project_document;
use crate::engine::project::item::is_item_id;

/// Assemble the version-one item envelope from a write request and the
/// current canonical revision, which the caller reads under its item lock.
pub(crate) fn assemble_item(
    input: &Value,
    existing: Option<&Value>,
    timezone: &str,
) -> Result<Value, String> {
    let input = input
        .as_object()
        .ok_or("Item write input must be an object.")?;
    let id = match input.get("id") {
        Some(Value::String(id)) if is_item_id(id) => id.clone(),
        Some(_) => return Err("Invalid Bluink item id.".to_owned()),
        None => Uuid::now_v7().to_string(),
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
    let content = input
        .get("content")
        .or_else(|| existing.and_then(|item| item.get("content")))
        .cloned()
        .ok_or("Item content is missing.")?;
    let content = if kind == "page" {
        prepare_page_content(content)?
    } else {
        content
    };
    let text = project_document(&content).text;
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut next = json!({
        "format": "bluink.item",
        "formatVersion": crate::engine::project::item::ITEM_FORMAT_VERSION,
        "id": id,
        "revision": Uuid::now_v7().to_string(),
        "createdAt": existing.map_or_else(|| json!(timestamp), |item| item["createdAt"].clone()),
        "modifiedAt": timestamp,
        "meta": meta,
        "content": content,
        "analytics": {
            "analyzerVersion": 1,
            "timezone": timezone,
            "current": {"contentHash": hash_text(&text), "wordCount": count_words(&text)}
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
    if let Some(tasks) = normalized_tasks(
        input
            .get("tasks")
            .or_else(|| existing.and_then(|item| item.get("tasks"))),
    )? {
        next["tasks"] = Value::Array(tasks);
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

fn normalized_tasks(value: Option<&Value>) -> Result<Option<Vec<Value>>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let values = value.as_array().ok_or("Item tasks must be an array.")?;
    if values.is_empty() {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    let mut tasks = Vec::with_capacity(values.len());
    for value in values {
        let mut task = value
            .as_object()
            .cloned()
            .ok_or("Note tasks must be objects.")?;
        let id = task.get("id").and_then(Value::as_str).unwrap_or("");
        if !is_item_id(id) || !seen.insert(id.to_owned()) {
            return Err("Note tasks need unique UUIDs.".to_owned());
        }
        let text = task
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        task.insert("text".to_owned(), Value::String(text.to_owned()));
        tasks.push(Value::Object(task));
    }
    tasks.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    Ok(Some(tasks))
}

fn count_words(text: &str) -> usize {
    static WORDS: OnceLock<Regex> = OnceLock::new();
    WORDS
        .get_or_init(|| {
            Regex::new(r"[\p{L}\p{N}]+(?:['’][\p{L}\p{N}]+)*").expect("valid word pattern")
        })
        .find_iter(text)
        .count()
}

fn hash_text(text: &str) -> String {
    let mut hash = 0x811c_9dc5_u32;
    for unit in text.encode_utf16() {
        hash = (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{assemble_item, count_words, hash_text};

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
            "tags":["z","a"],
            "tasks":[{"id":"01a0e2a0-0000-7000-8000-000000000002","text":" Earlier ","createdAt":"2026-09-01T00:00:00.000Z"}]
        });
        let next = assemble_item(&json!({
            "id":"01a0e2a0-0000-7000-8000-000000000001",
            "meta":{"title":"Second"},
            "content":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A sample"}]}]}
        }), Some(&existing), "Europe/London").unwrap();
        assert_eq!(next["meta"]["title"], "Second");
        assert_eq!(next["meta"]["type"], "page");
        assert_eq!(next["tags"], json!(["a", "z"]));
        assert_eq!(next["tasks"][0]["text"], "Earlier");
        assert_eq!(next["analytics"]["current"]["wordCount"], 2);
        assert!(next["content"]["content"][0]["attrs"]["id"].is_string());
    }
}
