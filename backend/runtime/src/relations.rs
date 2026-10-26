// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::indexing::coordinator::MainSnapshot;
use std::collections::HashSet;

use crate::application::error::Result;
use serde_json::{Value as JsonValue, json};
use tantivy::collector::FacetCollector;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{Facet, IndexRecordOption, OwnedValue, Schema, TantivyDocument, Value};
use tantivy::{IndexReader, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::indexing::service::cached_index;
use crate::indexing::state::CachedResult;
use crate::model::TargetCount;

pub(crate) fn exact_term_query(field: tantivy::schema::Field, value: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(
        Term::from_field_text(field, value),
        IndexRecordOption::Basic,
    ))
}
pub(crate) fn relation_facet_path(kind: &str, key: &str) -> Option<Facet> {
    match kind {
        "tag" => Some(Facet::from_path(
            std::iter::once(kind).chain(key.split('/')),
        )),
        "label" => Some(Facet::from_path([kind, key])),
        _ => None,
    }
}

pub(crate) fn relation_facet_value_path(kind: &str, key: &str) -> Option<Facet> {
    let value = if kind == "tag" {
        key.replace('/', "~")
    } else {
        key.to_owned()
    };
    match kind {
        "tag" => Some(Facet::from_path(["tag_values", value.as_str()])),
        "label" => Some(Facet::from_path(["label_values", value.as_str()])),
        _ => None,
    }
}
fn active_query(
    schema: &Schema,
    source_type: Option<&str>,
    required: &[(String, String)],
) -> Result<Box<dyn Query>> {
    let state = schema.get_field("state").map_err(internal_error)?;
    let item_type = schema.get_field("item_type").map_err(internal_error)?;
    let facets = schema.get_field("facet_paths").map_err(internal_error)?;
    let mut clauses: Vec<(Occur, Box<dyn Query>)> =
        vec![(Occur::Must, exact_term_query(state, "active"))];
    if let Some(kind) = source_type {
        clauses.push((Occur::Must, exact_term_query(item_type, kind)));
    }
    for (kind, key) in required {
        let path = relation_facet_path(kind, key)
            .ok_or_else(|| internal_error("Unsupported facet kind"))?;
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_facet(facets, &path),
                IndexRecordOption::Basic,
            )),
        ));
    }
    Ok(Box::new(BooleanQuery::new(clauses)))
}
fn collect_facets(
    reader: &IndexReader,
    query: &dyn Query,
    kind: &str,
    selected: Option<&HashSet<String>>,
) -> Result<Vec<TargetCount>> {
    let root = format!("/{kind}_values");
    let mut collector = FacetCollector::for_field("facet_paths");
    collector.add_facet(&root);
    let counts = reader
        .searcher()
        .search(query, &collector)
        .map_err(internal_error)?;
    let mut output = counts
        .get(&root)
        .filter_map(|(facet, count)| {
            let key = facet.to_path().last()?.replace('~', "/");
            if selected.is_some_and(|keys| !keys.contains(&key)) {
                return None;
            }
            Some(TargetCount {
                tid: key,
                count: usize::try_from(count).unwrap_or(usize::MAX),
            })
        })
        .collect::<Vec<_>>();
    output.sort_by(|a, b| a.tid.cmp(&b.tid));
    Ok(output)
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn facet_service_sync(
    index_path: String,
    field_name: String,
    selected: Vec<String>,
    source_type: Option<String>,
) -> Result<Vec<TargetCount>> {
    let kind = match field_name.as_str() {
        "tags" => "tag",
        "labels" => "label",
        _ => return Err(invalid_request("Unsupported facet field")),
    };
    let cache_key = format!("facet:{field_name}:{source_type:?}:{selected:?}");
    let (snapshot, cache) = cached_index(&index_path)?;
    let cached = cache.get(&cache_key).map_err(internal_error)?;
    if let Some(CachedResult::Facets(value)) = cached {
        return Ok(value);
    }
    let selected: HashSet<String> = selected.into_iter().collect();
    let MainSnapshot { index, reader, .. } = snapshot;
    let required = selected
        .iter()
        .cloned()
        .map(|key| (kind.to_owned(), key))
        .collect::<Vec<_>>();
    let query = active_query(&index.schema(), source_type.as_deref(), &required)?;
    let output = collect_facets(&reader, query.as_ref(), kind, None)?
        .into_iter()
        .filter(|entry| !selected.contains(&entry.tid))
        .collect::<Vec<_>>();
    cache
        .insert(cache_key, CachedResult::Facets(output.clone()))
        .map_err(internal_error)?;
    Ok(output)
}
pub(crate) fn document_values(
    document: &TantivyDocument,
    field: tantivy::schema::Field,
) -> Vec<String> {
    document
        .get_all(field)
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}
pub(crate) fn owned_value_to_json(value: &OwnedValue) -> JsonValue {
    match value {
        OwnedValue::Null => JsonValue::Null,
        OwnedValue::Str(value) => JsonValue::String(value.clone()),
        OwnedValue::U64(value) => json!(value),
        OwnedValue::I64(value) => json!(value),
        OwnedValue::F64(value) => json!(value),
        OwnedValue::Bool(value) => json!(value),
        OwnedValue::Array(values) => {
            JsonValue::Array(values.iter().map(owned_value_to_json).collect())
        }
        OwnedValue::Object(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), owned_value_to_json(value)))
                .collect(),
        ),
        other => JsonValue::String(format!("{other:?}")),
    }
}
