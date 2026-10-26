// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Snippet queries use the project's shared index and publication worker.
use crate::application::error::{Result as ApplicationResult, internal_error, invalid_request};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tantivy::Order;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, RegexQuery};
use tantivy::schema::{Schema, TantivyDocument, Value};

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnippetRecord {
    format: String,
    format_version: u32,
    pub(crate) id: String,
    state: SnippetState,
    revision: String,
    created_at: String,
    modified_at: String,
    paragraph: crate::engine::project::types::ReferenceParagraph,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum SnippetState {
    Active,
    Archived,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnippetHit {
    #[serde(flatten)]
    record: SnippetRecord,
    usage_count: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct SnippetResults {
    items: Vec<SnippetHit>,
    count: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnippetUsage {
    note_id: String,
    note_title: String,
    revision: String,
    occurrences: Vec<SnippetOccurrence>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
struct SnippetOccurrence {
    kind: SnippetOccurrenceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    block_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum SnippetOccurrenceKind {
    Citation,
    Embed,
}

fn regex_literal(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if ".+*?()|[]{}^$\\\"".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!(".*{escaped}.*")
}

/// Add snippet-specific fields to its canonical item's single index document.
pub(crate) fn add_fields(
    schema: &Schema,
    document: &mut TantivyDocument,
    source: &crate::projection::SearchDocument,
) -> ApplicationResult<()> {
    let Some(record) = &source.snippet else {
        return Ok(());
    };
    let key = record["id"]
        .as_str()
        .ok_or_else(|| internal_error("Snippet has no key"))?;
    let text = record["paragraph"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["text"].as_str())
        .collect::<String>()
        .to_lowercase()
        .replace('\n', " ");
    for (name, value) in [
        ("snippet_id", key),
        ("key_search", key),
        ("text_search", text.as_str()),
    ] {
        document.add_text(schema.get_field(name).map_err(internal_error)?, value);
    }
    document.add_text(
        schema.get_field("snippet_record").map_err(internal_error)?,
        serde_json::to_string(record).map_err(internal_error)?,
    );
    Ok(())
}

pub(crate) fn query(
    index_path: &str,
    root: &Path,
    query: &str,
    limit: u32,
    offset: u32,
) -> ApplicationResult<SnippetResults> {
    if !(1..=200).contains(&limit) {
        return Err(invalid_request(
            "Snippet query limit must be between 1 and 200.",
        ));
    }
    let snapshot = crate::indexing::service::query_snapshot(index_path)?;
    let schema = snapshot.index.schema();
    let searcher = snapshot.reader.searcher();
    let key_search = schema.get_field("key_search").map_err(internal_error)?;
    let text_search = schema.get_field("text_search").map_err(internal_error)?;
    let record_field = schema.get_field("snippet_record").map_err(internal_error)?;
    let term = query.trim().to_lowercase().replace('\n', " ");
    let query: Box<dyn Query> = if term.is_empty() {
        Box::new(AllQuery)
    } else {
        let pattern = regex_literal(&term);
        Box::new(tantivy::query::BooleanQuery::union(vec![
            Box::new(RegexQuery::from_pattern(&pattern, key_search).map_err(internal_error)?),
            Box::new(RegexQuery::from_pattern(&pattern, text_search).map_err(internal_error)?),
        ]))
    };
    let query = BooleanQuery::new(vec![
        (Occur::Must, query),
        (
            Occur::Must,
            crate::relations::exact_term_query(
                schema.get_field("item_type").map_err(internal_error)?,
                "snippet",
            ),
        ),
        (
            Occur::Must,
            crate::relations::exact_term_query(
                schema.get_field("state").map_err(internal_error)?,
                "active",
            ),
        ),
    ]);
    let count = searcher.search(&query, &Count).map_err(internal_error)?;
    let hits = searcher
        .search(
            &query,
            &TopDocs::with_limit(limit as usize)
                .and_offset(offset as usize)
                .order_by_string_fast_field("snippet_id", Order::Asc),
        )
        .map_err(internal_error)?;
    let mut items = Vec::new();
    for (_, address) in hits {
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        let record = document
            .get_first(record_field)
            .and_then(|value| value.as_str())
            .ok_or_else(|| internal_error("Indexed snippet is missing its record"))?;
        let record: SnippetRecord = serde_json::from_str(record).map_err(internal_error)?;
        let usage_count = 0;
        items.push(SnippetHit {
            record,
            usage_count,
        });
    }
    let ids: Vec<_> = items.iter().map(|item| item.record.id.as_str()).collect();
    let mut usages =
        crate::document_graph::snippet_usage_counts(root, &ids).map_err(internal_error)?;
    for item in &mut items {
        item.usage_count = usages.remove(&item.record.id).unwrap_or(0);
    }
    Ok(SnippetResults { items, count })
}
