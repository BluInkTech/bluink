// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::application::error::Result;
use serde_json::{Value as JsonValue, json};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, TermSetQuery};
use tantivy::schema::{TantivyDocument, Value};
use tantivy::{Order, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::indexing::service::query_snapshot;
use crate::model::{PassageRecord, ResultPage};
use crate::query::document::QueryDocument;
use crate::relations::exact_term_query;

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
    let root = crate::engine::document::document_root(content);
    let mut projection = Projection {
        item_id,
        revision,
        note_title,
        records: Vec::new(),
    };
    projection.visit(root);
    projection.records
}
fn results(
    index_path: &str,
    label: &str,
    scope: Option<&QueryDocument>,
    limit: usize,
    offset: usize,
    pin_scope_id: Option<&str>,
) -> Result<(usize, Vec<PassageRecord>)> {
    let snapshot = query_snapshot(index_path)?;
    let index = &snapshot.index;
    let reader = &snapshot.reader;
    let schema = index.schema();
    let label_field = schema.get_field("labels").map_err(internal_error)?;
    let kind_field = schema.get_field("item_type").map_err(internal_error)?;
    let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = vec![
        (Occur::Must, exact_term_query(label_field, label)),
        (Occur::Must, exact_term_query(kind_field, "page")),
    ];
    if let Some(scope) = scope.filter(|query| !query.is_empty()) {
        let ids = crate::search::matching_item_ids(&snapshot, scope, pin_scope_id)?;
        let field = schema.get_field("id").map_err(internal_error)?;
        clauses.push((
            Occur::Must,
            Box::new(TermSetQuery::new(
                ids.iter().map(|id| Term::from_field_text(field, id)),
            )),
        ));
    }
    let query = BooleanQuery::new(clauses);
    let searcher = reader.searcher();
    let candidates = searcher
        .search(&query, &tantivy::collector::Count)
        .map_err(internal_error)?;
    if candidates == 0 {
        return Ok((0, Vec::new()));
    }
    let hits = searcher
        .search(
            &query,
            &TopDocs::with_limit(candidates).order_by_string_fast_field("id", Order::Asc),
        )
        .map_err(internal_error)?;
    let payload = schema.get_field("label_text").map_err(internal_error)?;
    let mut count = 0;
    let mut records = Vec::new();
    for (_, address) in hits {
        let column = searcher
            .segment_reader(address.segment_ord)
            .fast_fields()
            .column_opt::<i64>(&format!("label_counts.{label}"))
            .map_err(internal_error)?;
        let matches = usize::try_from(
            column
                .and_then(|column| column.first(address.doc_id))
                .unwrap_or(0),
        )
        .map_err(internal_error)?;
        let start = count;
        count += matches;
        if count <= offset || records.len() >= limit {
            continue;
        }
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        let source = document
            .get_first(payload)
            .and_then(|value| value.as_str())
            .ok_or_else(|| internal_error("Indexed note has no label text"))?;
        let mut passages: Vec<PassageRecord> =
            serde_json::from_str(source).map_err(internal_error)?;
        passages.retain(|passage| passage.labels.iter().any(|key| key == label));
        passages.sort_by(|a, b| a.block_id.cmp(&b.block_id));
        records.extend(
            passages
                .into_iter()
                .skip(offset.saturating_sub(start))
                .take(limit - records.len()),
        );
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
    scope: Option<QueryDocument>,
    limit: u32,
    offset: u32,
    pin_scope_id: Option<&str>,
) -> Result<ResultPage<PassageRecord>> {
    if label.trim().is_empty() || limit == 0 || limit > 200 {
        return Err(invalid_request(
            "A label and a limit between 1 and 200 are required",
        ));
    }
    let (count, items) = results(
        &index_path,
        &label,
        scope.as_ref(),
        limit as usize,
        offset as usize,
        pin_scope_id,
    )?;
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
