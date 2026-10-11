// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed sort values and Tantivy fast-field collection, independent of parsing.
use serde_json::Value as JsonValue;
use tantivy::collector::TopDocs;
use tantivy::collector::sort_key::SortByErasedType;
use tantivy::query::Query;
use tantivy::{DocAddress, Order};

use crate::model::{SearchHit, SortSpec};

pub(crate) fn search_hit_sort_value(hit: &SearchHit, field: &str) -> Option<JsonValue> {
    match field {
        "id" => Some(JsonValue::String(hit.id.clone())),
        "title" => hit
            .metadata
            .get("title")
            .and_then(JsonValue::as_str)
            .map(|value| JsonValue::String(value.to_lowercase())),
        "type" | "state" => hit.metadata.get(field).cloned(),
        "created" => Some(JsonValue::String(hit.created_at.clone())),
        "modified" => Some(JsonValue::String(hit.modified_at.clone())),
        field => hit.metadata.get(field).cloned(),
    }
}

pub(crate) fn compare_sort_values(
    left: Option<JsonValue>,
    right: Option<JsonValue>,
    descending: bool,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let compared = match (left, right) {
        (None | Some(JsonValue::Null), None | Some(JsonValue::Null)) => Ordering::Equal,
        (None | Some(JsonValue::Null), _) => return Ordering::Greater,
        (_, None | Some(JsonValue::Null)) => return Ordering::Less,
        (Some(JsonValue::Number(left)), Some(JsonValue::Number(right))) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(Ordering::Equal),
        (Some(JsonValue::Bool(left)), Some(JsonValue::Bool(right))) => left.cmp(&right),
        (Some(JsonValue::String(left)), Some(JsonValue::String(right))) => {
            left.to_lowercase().cmp(&right.to_lowercase())
        }
        (Some(left), Some(right)) => left.to_string().cmp(&right.to_string()),
    };
    if descending {
        compared.reverse()
    } else {
        compared
    }
}

pub(crate) fn fast_sort(sort: &SortSpec) -> (SortByErasedType, Order) {
    let field = match sort.field.as_str() {
        "title" => "title_sort".to_owned(),
        "type" => "item_type".to_owned(),
        "created" => "created_at_date".to_owned(),
        "modified" => "modified_at_date".to_owned(),
        "id" | "state" => sort.field.clone(),
        field => format!("metadata.{field}"),
    };
    (
        SortByErasedType::for_field(field),
        if sort.descending {
            Order::Desc
        } else {
            Order::Asc
        },
    )
}

pub(crate) fn collect_fast_sorted(
    searcher: &tantivy::Searcher,
    query: &dyn Query,
    sorts: &[SortSpec],
    limit: usize,
) -> tantivy::Result<Vec<DocAddress>> {
    let addresses = match sorts {
        [first] => searcher
            .search(
                query,
                &TopDocs::with_limit(limit).order_by(fast_sort(first)),
            )?
            .into_iter()
            .map(|(_, address)| address)
            .collect(),
        [first, second] => searcher
            .search(
                query,
                &TopDocs::with_limit(limit).order_by((fast_sort(first), fast_sort(second))),
            )?
            .into_iter()
            .map(|(_, address)| address)
            .collect(),
        [first, second, third] => searcher
            .search(
                query,
                &TopDocs::with_limit(limit)
                    .order_by((fast_sort(first), (fast_sort(second), fast_sort(third)))),
            )?
            .into_iter()
            .map(|(_, address)| address)
            .collect(),
        [first, second, third, fourth] => searcher
            .search(
                query,
                &TopDocs::with_limit(limit).order_by((
                    fast_sort(first),
                    (fast_sort(second), (fast_sort(third), fast_sort(fourth))),
                )),
            )?
            .into_iter()
            .map(|(_, address)| address)
            .collect(),
        _ => {
            return Err(tantivy::TantivyError::InvalidArgument(
                "At most four sort fields are supported".to_owned(),
            ));
        }
    };
    Ok(addresses)
}
