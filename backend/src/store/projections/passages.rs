// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::records::PassageRecord;
use serde_json::{Value as JsonValue, json};

const MAX_FRAGMENT_BYTES: usize = 65_536;
const MAX_PREVIEW_CHARS: usize = 8_192;

fn labels(value: Option<&JsonValue>) -> Vec<String> {
    let mut result = value
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(JsonValue::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

fn text_of(node: &JsonValue, output: &mut String) {
    if let Some(text) = node.get("text").and_then(JsonValue::as_str) {
        output.push_str(text);
    }
    if let Some(children) = node.get("content").and_then(JsonValue::as_array) {
        for child in children {
            text_of(child, output);
        }
    }
}

fn bounded_fragment(node: &JsonValue) -> (JsonValue, bool) {
    let complete = json!({"type":"doc","content":[node]});
    if serde_json::to_vec(&complete).is_ok_and(|bytes| bytes.len() <= MAX_FRAGMENT_BYTES) {
        return (complete, false);
    }
    let mut text = String::new();
    text_of(node, &mut text);
    let clipped = text.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
    (
        json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":clipped}]}]}),
        true,
    )
}

struct Projection<'a> {
    item_id: &'a str,
    revision: &'a str,
    note_title: &'a str,
    records: Vec<PassageRecord>,
}

impl Projection<'_> {
    fn visit(&mut self, node: &JsonValue) {
        if node["type"] == "paragraph" {
            let labels = labels(node.get("attrs").and_then(|attrs| attrs.get("labels")));
            let block_id = node["attrs"]["id"].as_str().unwrap_or_default();
            if labels.is_empty() || block_id.is_empty() {
                return;
            }
            let mut text = String::new();
            text_of(node, &mut text);
            if text.trim().is_empty() {
                return;
            }
            let (fragment, truncated) = bounded_fragment(node);
            self.records.push(PassageRecord {
                item_id: self.item_id.to_owned(),
                block_id: block_id.to_owned(),
                revision: self.revision.to_owned(),
                note_title: self.note_title.to_owned(),
                labels,
                text: text.chars().take(MAX_PREVIEW_CHARS).collect(),
                fragment,
                truncated,
            });
            return;
        }
        if let Some(children) = node.get("content").and_then(JsonValue::as_array) {
            for child in children {
                self.visit(child);
            }
        }
    }
}

pub(crate) fn project(
    item_id: &str,
    revision: &str,
    note_title: &str,
    content: &JsonValue,
) -> Vec<PassageRecord> {
    let root = crate::domain::document::document_root(content);
    let mut projection = Projection {
        item_id,
        revision,
        note_title,
        records: Vec::new(),
    };
    projection.visit(root);
    projection.records
}
#[cfg(test)]
mod tests {
    use super::project;
    use serde_json::json;
    #[test]
    fn projects_labelled_paragraphs_once_with_their_formatting() {
        let content = json!({"document": {"type":"doc","content":[
            {"type":"box","attrs":{"kind":"quote"},"content":[
                {"type":"paragraph","attrs":{"id":"block-1","labels":["source","point"]},"content":[
                    {"type":"text","text":"Hello "},
                    {"type":"text","text":"world","marks":[{"type":"strong"}]}
                ]}
            ]},
            {"type":"paragraph","attrs":{"id":"block-2"},"content":[{"type":"text","text":"Unlabelled"}]}
        ]}});
        let records = project("item", "revision", "Title", &content);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].labels, vec!["point", "source"]);
        assert_eq!(records[0].text, "Hello world");
        assert_eq!(
            records[0].fragment["content"][0]["content"][1]["marks"][0]["type"],
            "strong"
        );
    }
}
