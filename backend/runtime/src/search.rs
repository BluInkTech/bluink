// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;
use std::time::Instant;

use crate::application::error::Result;
use tantivy::DocAddress;
use tantivy::collector::{Count, DocSetCollector, TopDocs};
use tantivy::query::{BooleanQuery, Occur, Query};
use tantivy::schema::{OwnedValue, TantivyDocument, Value};

use crate::application::error::internal_error;
use crate::model::{CachedResult, SearchHit, SearchResults, empty_object};
use crate::query::compile_native_query;
use crate::query::sort::{collect_fast_sorted, compare_sort_values, search_hit_sort_value};
use crate::relations::{document_values, exact_term_query, owned_value_to_json};
use crate::service::{aggregation_cache, cache_result, query_fields, service_index};

/// Resolve query membership without sorting or hydrating stored item documents.
/// Activity reports only need IDs, and can match more than one search page.
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn matching_item_ids_service_sync(
    index_path: String,
    query_text: String,
) -> Result<Vec<String>> {
    let (index, reader, dirty) = service_index(&index_path)?;
    let property_fields = query_fields(&index_path)?;
    let (query, _, _) = compile_native_query(&index, &query_text, &property_fields, None)?;
    let searcher = reader.searcher();
    let addresses = searcher
        .search(query.as_ref(), &DocSetCollector)
        .map_err(internal_error)?;
    let dirty = dirty.lock().map_err(internal_error)?.clone();
    let mut by_segment: HashMap<u32, Vec<u32>> = HashMap::new();
    for address in addresses {
        by_segment
            .entry(address.segment_ord)
            .or_default()
            .push(address.doc_id);
    }
    let mut ids = Vec::new();
    for (segment_ord, doc_ids) in by_segment {
        let segment = searcher.segment_reader(segment_ord);
        let id_column = segment
            .fast_fields()
            .str("id")
            .map_err(internal_error)?
            .ok_or_else(|| internal_error("Tantivy id fast field is missing"))?;
        for doc_id in doc_ids {
            let Some(id_ord) = id_column.term_ords(doc_id).next() else {
                continue;
            };
            let mut id = String::new();
            id_column
                .ord_to_str(id_ord, &mut id)
                .map_err(internal_error)?;
            if !dirty.contains_key(&id) {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

fn collect_ranked(
    searcher: &tantivy::Searcher,
    query: &dyn Query,
    sorts: &[crate::model::SortSpec],
    limit: usize,
) -> Result<(Vec<(f32, DocAddress)>, bool)> {
    if sorts.is_empty() {
        return searcher
            .search(
                query,
                &TopDocs::with_limit(limit.saturating_add(256)).order_by_score(),
            )
            .map(|hits| (hits, false))
            .map_err(internal_error);
    }
    collect_fast_sorted(searcher, query, sorts, limit).map_or_else(
        |_| {
            searcher
                .search(
                    query,
                    &TopDocs::with_limit(
                        usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX),
                    )
                    .order_by_score(),
                )
                .map(|hits| (hits, false))
                .map_err(internal_error)
        },
        |addresses| {
            Ok((
                addresses
                    .into_iter()
                    .map(|address| (0.0, address))
                    .collect(),
                true,
            ))
        },
    )
}

#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(limit, offset, query_length = query_text.len(), has_pin_scope = pin_scope_id.is_some())
)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn search_service_sync(
    index_path: String,
    query_text: String,
    limit: u32,
    offset: u32,
    pin_scope_id: Option<String>,
) -> Result<SearchResults> {
    let started_at = Instant::now();
    let cache_key = format!("search:{limit}:{offset}:{query_text}:{pin_scope_id:?}");
    let cache = aggregation_cache(&index_path)?;
    let (index, reader, dirty) = service_index(&index_path)?;
    let property_fields = query_fields(&index_path)?;
    let (query, sorts, _) = compile_native_query(
        &index,
        &query_text,
        &property_fields,
        pin_scope_id.as_deref(),
    )?;
    let compiled_at = Instant::now();
    let schema = index.schema();
    let id = schema.get_field("id").map_err(internal_error)?;
    let revision = schema.get_field("revision").map_err(internal_error)?;
    let created_at = schema.get_field("created_at").map_err(internal_error)?;
    let modified_at = schema.get_field("modified_at").map_err(internal_error)?;
    let metadata = schema.get_field("metadata").map_err(internal_error)?;
    let preview = schema.get_field("preview").map_err(internal_error)?;
    let preview_image = schema.get_field("preview_image").map_err(internal_error)?;
    let tags = schema.get_field("tags").map_err(internal_error)?;
    let pins = schema.get_field("pins").map_err(internal_error)?;
    let searcher = reader.searcher();
    // The index worker mutates this small map. Snapshot it before doing any
    // search work so independent readers never serialize behind the mutex.
    let dirty = dirty.lock().map_err(internal_error)?.clone();
    if dirty.is_empty()
        && let Some(CachedResult::Search(cached)) = cache
            .lock()
            .map_err(internal_error)?
            .get(&cache_key)
            .cloned()
    {
        return Ok(cached);
    }
    let mut pinned_hits = Vec::new();
    let mut unpinned_hits = Vec::new();
    let page_size = limit
        .saturating_add(offset)
        .saturating_add(u32::try_from(dirty.len()).unwrap_or(u32::MAX))
        .max(1) as usize;
    let (ranked, pinned_ranked_len, used_fast_sort, pinned_count) =
        if let Some(scope_id) = pin_scope_id.as_deref() {
            let (pinned_base, _, _) = compile_native_query(
                &index,
                &query_text,
                &property_fields,
                pin_scope_id.as_deref(),
            )?;
            let (unpinned_base, _, _) = compile_native_query(
                &index,
                &query_text,
                &property_fields,
                pin_scope_id.as_deref(),
            )?;
            let pinned_query = BooleanQuery::new(vec![
                (Occur::Must, pinned_base),
                (Occur::Must, exact_term_query(pins, scope_id)),
            ]);
            let unpinned_query = BooleanQuery::new(vec![
                (Occur::Must, unpinned_base),
                (Occur::MustNot, exact_term_query(pins, scope_id)),
            ]);
            let pinned_count = searcher
                .search(&pinned_query, &Count)
                .map_err(internal_error)?;
            let (mut pinned, pinned_fast_sort) =
                collect_ranked(&searcher, &pinned_query, &sorts, page_size)?;
            let (unpinned, unpinned_fast_sort) =
                collect_ranked(&searcher, &unpinned_query, &sorts, page_size)?;
            let pinned_ranked_len = pinned.len();
            pinned.extend(unpinned);
            (
                pinned,
                Some(pinned_ranked_len),
                pinned_fast_sort && unpinned_fast_sort,
                Some(pinned_count),
            )
        } else {
            let (ranked, used_fast_sort) =
                collect_ranked(&searcher, query.as_ref(), &sorts, page_size)?;
            (ranked, None, used_fast_sort, None)
        };
    let collected_at = Instant::now();

    let can_page_before_hydration = dirty.is_empty() && (used_fast_sort || sorts.is_empty());
    let mut skipped = 0usize;
    for (rank_index, (score, address)) in ranked.into_iter().enumerate() {
        // Tantivy returns enough ranked addresses to satisfy offset paging. Do
        // not load and decode stored metadata for addresses that will be
        // discarded from a clean index page.
        if can_page_before_hydration && skipped < offset as usize {
            skipped += 1;
            continue;
        }
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        let item_id = document
            .get_first(id)
            .and_then(|value| value.as_str())
            .ok_or_else(|| internal_error("Tantivy document is missing its item id"))?;
        if dirty.contains_key(item_id) {
            continue;
        }
        let hit = SearchHit {
            id: item_id.to_owned(),
            revision: document
                .get_first(revision)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            created_at: document
                .get_first(created_at)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            modified_at: document
                .get_first(modified_at)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            score,
            preview: document
                .get_first(preview)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            preview_image: document
                .get_first(preview_image)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            metadata: document
                .get_first(metadata)
                .map(OwnedValue::from)
                .map_or_else(empty_object, |value| owned_value_to_json(&value)),
            tags: document_values(&document, tags),
        };
        if pinned_ranked_len.is_some_and(|pinned_len| rank_index < pinned_len) {
            pinned_hits.push(hit);
        } else {
            unpinned_hits.push(hit);
        }
        if can_page_before_hydration
            && pinned_hits.len().saturating_add(unpinned_hits.len()) >= limit as usize
        {
            break;
        }
    }
    if !sorts.is_empty() && !used_fast_sort {
        let sort_hits = |hits: &mut Vec<SearchHit>| {
            hits.sort_by(|left, right| {
                for sort in &sorts {
                    let compared = compare_sort_values(
                        search_hit_sort_value(left, &sort.field),
                        search_hit_sort_value(right, &sort.field),
                        sort.descending,
                    );
                    if compared != std::cmp::Ordering::Equal {
                        return compared;
                    }
                }
                left.id.cmp(&right.id)
            });
        };
        // Fallback sorting must preserve the two query partitions: pins are
        // presentation priority, then each partition follows the requested sort.
        sort_hits(&mut pinned_hits);
        sort_hits(&mut unpinned_hits);
    }
    pinned_hits.extend(unpinned_hits);
    let hits = if can_page_before_hydration {
        pinned_hits
    } else {
        pinned_hits
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    };
    let hydrated_at = Instant::now();
    let count = searcher.search(&query, &Count).map_err(internal_error)?;
    let counted_at = Instant::now();
    let result = SearchResults {
        hits,
        count,
        pinned_count,
    };
    tracing::debug!(
        operation = "search",
        compile_ms = compiled_at.duration_since(started_at).as_secs_f64() * 1000.0,
        collect_ms = collected_at.duration_since(compiled_at).as_secs_f64() * 1000.0,
        hydrate_ms = hydrated_at.duration_since(collected_at).as_secs_f64() * 1000.0,
        count_ms = counted_at.duration_since(hydrated_at).as_secs_f64() * 1000.0,
        total_ms = Instant::now().duration_since(started_at).as_secs_f64() * 1000.0,
        limit,
        offset,
        dirty_count = dirty.len(),
        used_fast_sort,
        "search operation completed"
    );
    if dirty.is_empty() {
        cache_result(&cache, cache_key, CachedResult::Search(result.clone()));
    }
    Ok(result)
}
