// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::indexing::coordinator::MainSnapshot;
use std::collections::BTreeMap;

use crate::application::error::Result;
use tantivy::collector::DocSetCollector;

use crate::application::error::{internal_error, invalid_request};
use crate::indexing::service::cached_index;
use crate::indexing::state::CachedResult;
use crate::model::{CalendarBucket, CalendarResults};
use crate::query::{compile_query_document, document::QueryDocument};

fn is_date(value: &str) -> bool {
    crate::engine::project::item::is_canonical_date(value)
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn calendar_buckets_service_sync(
    index_path: String,
    document: QueryDocument,
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
    let (snapshot, cache) = cached_index(&index_path)?;
    let MainSnapshot {
        index,
        reader,
        query_fields: property_fields,
        ..
    } = snapshot;
    let identity = document.cache_identity()?;
    let cache_key = format!(
        "calendar:{identity}:{field}:{start_date}:{end_date}:{granularity}:{pin_scope_id:?}"
    );
    if let Some(CachedResult::Calendar(cached)) = cache.get(&cache_key).map_err(internal_error)? {
        return Ok(cached);
    }

    let (query, _, _) =
        compile_query_document(&index, &document, &property_fields, pin_scope_id.as_deref())?;
    // Calendar dates are scalar values. JSON fast fields flatten arrays, so do
    // not accidentally aggregate a multi-select property's first string.
    if field.strip_prefix("meta.").is_some_and(|key| {
        property_fields.value_type(key)
            == Some(crate::engine::project::types::MetadataFieldValueType::MultiSelect)
    }) {
        return Ok(CalendarResults {
            buckets: Vec::new(),
        });
    }
    let column_name = match field.as_str() {
        "created" => "created_at".to_owned(),
        "modified" => "modified_at".to_owned(),
        _ => format!(
            "metadata.{}",
            field
                .strip_prefix("meta.")
                .ok_or_else(|| invalid_request("Choose a date property"))?
        ),
    };
    let searcher = reader.searcher();
    let mut addresses: Vec<_> = searcher
        .search(query.as_ref(), &DocSetCollector)
        .map_err(internal_error)?
        .into_iter()
        .collect();
    addresses.sort_unstable();
    let mut buckets = BTreeMap::<String, u64>::new();
    for group in addresses.chunk_by(|a, b| a.segment_ord == b.segment_ord) {
        let Some(column) = searcher
            .segment_reader(group[0].segment_ord)
            .fast_fields()
            .str(&column_name)
            .map_err(internal_error)?
        else {
            continue;
        };
        let rows = crate::indexing::fast::Strings::from(column)
            .rows(group.iter().map(|address| address.doc_id))
            .map_err(internal_error)?;
        for row in rows {
            let Some(value) = row.first() else {
                continue;
            };
            let Some(bucket) = bucket_for(value, &granularity) else {
                continue;
            };
            let length = bucket.len();
            if bucket.as_str() >= &start_date[..length] && bucket.as_str() <= &end_date[..length] {
                *buckets.entry(bucket).or_default() += 1;
            }
        }
    }
    let result = CalendarResults {
        buckets: buckets
            .into_iter()
            .map(|(date, count)| CalendarBucket { date, count })
            .collect(),
    };
    cache
        .insert(cache_key, CachedResult::Calendar(result.clone()))
        .map_err(internal_error)?;
    Ok(result)
}
