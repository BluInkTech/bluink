// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::{Path, PathBuf};

use crate::application::error::Result;
use serde_json::{Value as JsonValue, json};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur};
use tantivy::schema::{FAST, STORED, STRING, Schema, TEXT, TantivyDocument, Value};
use tantivy::{Index, IndexWriter, Order, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::model::{PassageRecord, ResultPage, SearchDocument};
use crate::relations::exact_term_query;
use crate::service::service_passage_index;

const MAX_FRAGMENT_BYTES: usize = 65_536;
const MAX_PREVIEW_CHARS: usize = 8_192;

pub(crate) fn index_path(main_index_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-passages", main_index_path.display()))
}

fn schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("item_id", STRING | FAST | STORED);
    builder.add_text_field("index_stamp", STRING | FAST);
    builder.add_text_field("record_kind", STRING | FAST | STORED);
    builder.add_text_field("passage_id", STRING | FAST | STORED);
    builder.add_text_field("block_id", STRING | STORED);
    builder.add_text_field("labels", STRING | FAST | STORED);
    builder.add_text_field("text", TEXT | STORED);
    builder.add_text_field("fragment", STORED);
    builder.add_text_field("truncated", STRING | STORED);
    builder.add_text_field("revision", STRING | STORED);
    builder.add_text_field("note_title", TEXT | STORED);
    builder.build()
}

pub(crate) fn open_or_create(path: &Path) -> std::result::Result<(Index, bool), String> {
    match Index::open_in_dir(path) {
        Ok(index) if index.schema() == schema() => Ok((index, false)),
        _ => {
            if path.exists() {
                fs::remove_dir_all(path).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(path).map_err(|error| error.to_string())?;
            Index::create_in_dir(path, schema())
                .map(|index| (index, true))
                .map_err(|error| error.to_string())
        }
    }
}

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
    let root = content
        .get("content")
        .filter(|value| value.get("type").and_then(JsonValue::as_str) == Some("doc"))
        .unwrap_or(content);
    let mut projection = Projection {
        item_id,
        revision,
        note_title,
        records: Vec::new(),
    };
    projection.visit(root);
    projection.records
}
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Index writes require exclusive access while the coordinator updates related projections."
)]
pub(crate) fn write_document(
    writer: &mut IndexWriter,
    schema: &Schema,
    document: &SearchDocument,
) -> Result<()> {
    let item_id = schema.get_field("item_id").map_err(internal_error)?;
    let index_stamp = schema.get_field("index_stamp").map_err(internal_error)?;
    let record_kind = schema.get_field("record_kind").map_err(internal_error)?;
    let passage_id = schema.get_field("passage_id").map_err(internal_error)?;
    let block_id = schema.get_field("block_id").map_err(internal_error)?;
    let labels = schema.get_field("labels").map_err(internal_error)?;
    let text = schema.get_field("text").map_err(internal_error)?;
    let fragment = schema.get_field("fragment").map_err(internal_error)?;
    let truncated = schema.get_field("truncated").map_err(internal_error)?;
    let revision = schema.get_field("revision").map_err(internal_error)?;
    let note_title = schema.get_field("note_title").map_err(internal_error)?;
    writer.delete_term(Term::from_field_text(item_id, &document.id));
    let mut marker = TantivyDocument::default();
    marker.add_text(item_id, &document.id);
    marker.add_text(index_stamp, &document.index_stamp);
    marker.add_text(record_kind, "marker");
    writer.add_document(marker).map_err(internal_error)?;
    for record in &document.passages {
        let mut entry = TantivyDocument::default();
        entry.add_text(item_id, &document.id);
        entry.add_text(record_kind, "passage");
        entry.add_text(passage_id, format!("{}:{}", document.id, record.block_id));
        entry.add_text(block_id, &record.block_id);
        for label in &record.labels {
            entry.add_text(labels, label);
        }
        entry.add_text(text, &record.text);
        entry.add_text(
            fragment,
            serde_json::to_string(&record.fragment).map_err(internal_error)?,
        );
        entry.add_text(truncated, if record.truncated { "true" } else { "false" });
        entry.add_text(revision, &record.revision);
        entry.add_text(note_title, &record.note_title);
        writer.add_document(entry).map_err(internal_error)?;
    }
    Ok(())
}

fn decode(schema: &Schema, document: &TantivyDocument) -> Result<PassageRecord> {
    let string = |name: &str| -> Result<String> {
        let field = schema.get_field(name).map_err(internal_error)?;
        Ok(document
            .get_first(field)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned())
    };
    let labels_field = schema.get_field("labels").map_err(internal_error)?;
    let strings = |field| {
        document
            .get_all(field)
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    Ok(PassageRecord {
        item_id: string("item_id")?,
        block_id: string("block_id")?,
        revision: string("revision")?,
        note_title: string("note_title")?,
        labels: strings(labels_field),
        text: string("text")?,
        fragment: serde_json::from_str(&string("fragment")?).map_err(internal_error)?,
        truncated: string("truncated")? == "true",
    })
}

fn results(
    index_path: &str,
    label: &str,
    limit: usize,
    offset: usize,
) -> Result<(usize, Vec<PassageRecord>)> {
    let (index, reader) = service_passage_index(index_path)?;
    let schema = index.schema();
    let label_field = schema.get_field("labels").map_err(internal_error)?;
    let kind_field = schema.get_field("record_kind").map_err(internal_error)?;
    let query = BooleanQuery::new(vec![
        (Occur::Must, exact_term_query(label_field, label)),
        (Occur::Must, exact_term_query(kind_field, "passage")),
    ]);
    let searcher = reader.searcher();
    let count = searcher.search(&query, &Count).map_err(internal_error)?;
    let hits = searcher
        .search(
            &query,
            &TopDocs::with_limit(limit)
                .and_offset(offset)
                .order_by_string_fast_field("passage_id", Order::Asc),
        )
        .map_err(internal_error)?;
    let mut records = Vec::new();
    for (_, address) in hits {
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        records.push(decode(&schema, &document)?);
    }
    Ok((count, records))
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn query_service(
    index_path: String,
    label: String,
    limit: u32,
    offset: u32,
) -> Result<ResultPage<PassageRecord>> {
    if label.trim().is_empty() || limit == 0 || limit > 200 {
        return Err(invalid_request(
            "A label and a limit between 1 and 200 are required",
        ));
    }
    let (count, items) = results(&index_path, &label, limit as usize, offset as usize)?;
    Ok(ResultPage { items, count })
}

#[cfg(test)]
mod tests {
    use super::project;
    use serde_json::json;
    #[test]
    fn projects_labelled_paragraphs_once_with_their_formatting() {
        let content = json!({"type":"doc","content":[
            {"type":"blockquote","content":[
                {"type":"paragraph","attrs":{"id":"block-1","labels":["source","point"]},"content":[
                    {"type":"text","text":"Hello "},
                    {"type":"text","text":"world","marks":[{"type":"strong"}]}
                ]}
            ]},
            {"type":"paragraph","attrs":{"id":"block-2"},"content":[{"type":"text","text":"Unlabelled"}]}
        ]});
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
