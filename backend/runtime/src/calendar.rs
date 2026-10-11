// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;

use crate::application::error::Result;
use serde_json::Value as JsonValue;
use tantivy::collector::TopDocs;
use tantivy::schema::{OwnedValue, TantivyDocument, Value};

use crate::application::error::{internal_error, invalid_request};
use crate::model::{CachedResult, CalendarBucket, CalendarResults};
use crate::query::compile_native_query;
use crate::relations::owned_value_to_json;
use crate::service::{aggregation_cache, cache_result, query_fields, service_index};

fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn bucket_for(value: &str, granularity: &str) -> Option<String> {
    let day = value.get(..10)?;
    if !is_date(day) {
        return None;
    }
    match granularity {
        "day" => Some(day.to_owned()),
        "month" => day.get(..7).map(str::to_owned),
        "year" => day.get(..4).map(str::to_owned),
        _ => None,
    }
}

fn value_for(
    document: &TantivyDocument,
    field: &str,
    created: tantivy::schema::Field,
    modified: tantivy::schema::Field,
    metadata: tantivy::schema::Field,
) -> Option<String> {
    match field {
        "created" => document.get_first(created)?.as_str().map(str::to_owned),
        "modified" => document.get_first(modified)?.as_str().map(str::to_owned),
        value if value.starts_with("meta.") => {
            let key = value.strip_prefix("meta.")?;
            let metadata = document.get_first(metadata).map(OwnedValue::from)?;
            let JsonValue::Object(values) = owned_value_to_json(&metadata) else {
                return None;
            };
            values
                .get(key)
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        }
        _ => None,
    }
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn calendar_buckets_service_sync(
    index_path: String,
    query_text: String,
    field: String,
    start_date: String,
    end_date: String,
    granularity: String,
    pin_scope_id: Option<String>,
) -> Result<CalendarResults> {
    if !is_date(&start_date) || !is_date(&end_date) || start_date > end_date {
        return Err(invalid_request(
            "Calendar dates must be an inclusive YYYY-MM-DD range",
        ));
    }
    if !matches!(granularity.as_str(), "day" | "month" | "year") {
        return Err(invalid_request(
            "Calendar granularity must be day, month, or year",
        ));
    }
    if field != "created" && field != "modified" && !field.starts_with("meta.") {
        return Err(invalid_request(
            "Calendar field must be created, modified, or a date property",
        ));
    }
    let (index, reader, dirty) = service_index(&index_path)?;
    let cache = aggregation_cache(&index_path)?;
    let cache_key = format!(
        "calendar:{query_text}:{field}:{start_date}:{end_date}:{granularity}:{pin_scope_id:?}"
    );
    let dirty = dirty.lock().map_err(internal_error)?.clone();
    if dirty.is_empty()
        && let Some(CachedResult::Calendar(cached)) = cache
            .lock()
            .map_err(internal_error)?
            .get(&cache_key)
            .cloned()
    {
        return Ok(cached);
    }

    let property_fields = query_fields(&index_path)?;
    let (query, _, _) = compile_native_query(
        &index,
        &query_text,
        &property_fields,
        pin_scope_id.as_deref(),
    )?;
    let schema = index.schema();
    let date_field = match field.as_str() {
        "created" => Some("created_at"),
        "modified" => Some("modified_at"),
        _ => None,
    };
    let searcher = reader.searcher();
    if dirty.is_empty()
        && let Some(date_field) = date_field
    {
        let columns = searcher
            .segment_readers()
            .iter()
            .map(|segment| {
                segment
                    .fast_fields()
                    .str(date_field)
                    .map_err(internal_error)?
                    .ok_or_else(|| internal_error("Calendar date fast field is missing"))
            })
            .collect::<Result<Vec<_>>>()?;
        let addresses = searcher
            .search(
                query.as_ref(),
                &TopDocs::with_limit(
                    (usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX)).max(1),
                )
                .order_by_score(),
            )
            .map_err(internal_error)?;
        let mut buckets = BTreeMap::<String, u64>::new();
        let mut value = String::new();
        for (_, address) in addresses {
            let column = &columns[address.segment_ord as usize];
            let Some(ordinal) = column.term_ords(address.doc_id).next() else {
                continue;
            };
            value.clear();
            column
                .ord_to_str(ordinal, &mut value)
                .map_err(internal_error)?;
            let Some(bucket) = bucket_for(&value, &granularity) else {
                continue;
            };
            let in_range = match granularity.as_str() {
                "day" => bucket >= start_date && bucket <= end_date,
                "month" => bucket.as_str() >= &start_date[..7] && bucket.as_str() <= &end_date[..7],
                _ => bucket.as_str() >= &start_date[..4] && bucket.as_str() <= &end_date[..4],
            };
            if in_range {
                *buckets.entry(bucket).or_default() += 1;
            }
        }
        let result = CalendarResults {
            buckets: buckets
                .into_iter()
                .map(|(date, count)| CalendarBucket { date, count })
                .collect(),
        };
        cache_result(&cache, cache_key, CachedResult::Calendar(result.clone()));
        return Ok(result);
    }

    let id = schema.get_field("id").map_err(internal_error)?;
    let created = schema.get_field("created_at").map_err(internal_error)?;
    let modified = schema.get_field("modified_at").map_err(internal_error)?;
    let metadata = schema.get_field("metadata").map_err(internal_error)?;
    let addresses = searcher
        .search(
            query.as_ref(),
            &TopDocs::with_limit(
                (usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX)).max(1),
            )
            .order_by_score(),
        )
        .map_err(internal_error)?;
    let mut buckets = BTreeMap::<String, u64>::new();
    for (_, address) in addresses {
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        let Some(item_id) = document.get_first(id).and_then(|value| value.as_str()) else {
            continue;
        };
        if dirty.contains_key(item_id) {
            continue;
        }
        let Some(value) = value_for(&document, &field, created, modified, metadata) else {
            continue;
        };
        let Some(bucket) = bucket_for(&value, &granularity) else {
            continue;
        };
        let in_range = match granularity.as_str() {
            "day" => bucket >= start_date && bucket <= end_date,
            "month" => bucket.as_str() >= &start_date[..7] && bucket.as_str() <= &end_date[..7],
            _ => bucket.as_str() >= &start_date[..4] && bucket.as_str() <= &end_date[..4],
        };
        if in_range {
            *buckets.entry(bucket).or_default() += 1;
        }
    }
    let result = CalendarResults {
        buckets: buckets
            .into_iter()
            .map(|(date, count)| CalendarBucket { date, count })
            .collect(),
    };
    if dirty.is_empty() {
        cache_result(&cache, cache_key, CachedResult::Calendar(result.clone()));
    }
    Ok(result)
}
