// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::indexing::coordinator::MainSnapshot;
use std::time::Instant;

use crate::application::error::Result;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, Query, TermSetQuery};
use tantivy::schema::{OwnedValue, TantivyDocument, Value};
use tantivy::{DocAddress, Term};

use crate::application::error::internal_error;
use crate::indexing::service::{cached_index, service_index};
use crate::indexing::state::CachedResult;
use crate::model::{SearchHit, SearchResults, empty_object};
use crate::query::sort::{collect_fast_sorted, compare_sort_values, search_hit_sort_value};
use crate::query::{compile_query_document, document::QueryDocument};
use crate::relations::{document_values, exact_term_query, owned_value_to_json};

/// Resolve query membership without sorting or hydrating stored item documents.
/// Activity reports only need IDs, and can match more than one search page.
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn matching_item_ids_service_sync(
    index_path: String,
    document: QueryDocument,
    pin_scope_id: Option<&str>,
) -> Result<Vec<String>> {
    matching_item_ids(&service_index(&index_path)?, &document, pin_scope_id)
}

pub(crate) fn matching_item_ids(
    snapshot: &MainSnapshot,
    document: &QueryDocument,
    pin_scope_id: Option<&str>,
) -> Result<Vec<String>> {
    matching_candidate_ids(snapshot, document, pin_scope_id, None)
}

/// Restrict membership work to the owners of the records being queried.
pub(crate) fn matching_candidate_ids(
    snapshot: &MainSnapshot,
    document: &QueryDocument,
    pin_scope_id: Option<&str>,
    candidates: Option<&[String]>,
) -> Result<Vec<String>> {
    let MainSnapshot {
        index,
        reader,
        query_fields: property_fields,
        ..
    } = snapshot;
    let (query, _, _) = compile_query_document(index, document, property_fields, pin_scope_id)?;
    let query = if let Some(candidates) = candidates {
        let field = index.schema().get_field("id").map_err(internal_error)?;
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, query),
            (
                Occur::Must,
                Box::new(TermSetQuery::new(
                    candidates.iter().map(|id| Term::from_field_text(field, id)),
                )),
            ),
        ])) as Box<dyn Query>
    } else {
        query
    };
    let searcher = reader.searcher();
    collect_matching_ids(&searcher, query.as_ref())
}

/// Resolve only the supplied candidates; never enumerate a workspace's corpus.
pub(crate) fn matching_candidate_note_ids_service_sync(
    index_path: &str,
    document: &QueryDocument,
    workspace_id: &str,
    candidates: &[String],
) -> Result<Vec<String>> {
    let MainSnapshot {
        index,
        reader,
        query_fields: property_fields,
        ..
    } = service_index(index_path)?;
    let (base, _, _) =
        compile_query_document(&index, document, &property_fields, Some(workspace_id))?;
    let schema = index.schema();
    let id = schema.get_field("id").map_err(internal_error)?;
    let query = BooleanQuery::new(vec![
        (Occur::Must, base),
        (
            Occur::Must,
            Box::new(TermSetQuery::new(
                candidates
                    .iter()
                    .map(|candidate| Term::from_field_text(id, candidate)),
            )),
        ),
        (
            Occur::Must,
            exact_term_query(
                schema.get_field("item_type").map_err(internal_error)?,
                "page",
            ),
        ),
        (
            Occur::Must,
            exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
        ),
    ]);
    collect_matching_ids(&reader.searcher(), &query)
}

fn collect_matching_ids(searcher: &tantivy::Searcher, query: &dyn Query) -> Result<Vec<String>> {
    crate::indexing::fast::matching_ids(searcher, query).map_err(internal_error)
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request at the application boundary."
)]
pub(crate) fn search_service_sync(
    index_path: String,
    document: QueryDocument,
    limit: u32,
    offset: u32,
    pin_scope_id: Option<String>,
    prioritize_pins: bool,
) -> Result<SearchResults> {
    let (snapshot, cache) = cached_index(&index_path)?;
    search_snapshot(
        &snapshot,
        &cache,
        &document,
        limit,
        offset,
        pin_scope_id.as_deref(),
        prioritize_pins,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep ranked collection and hydration together."
)]
#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(limit, offset, clause_count = document.clauses.len(), has_pin_scope = pin_scope_id.is_some())
)]
pub(crate) fn search_snapshot(
    snapshot: &MainSnapshot,
    cache: &crate::indexing::coordinator::QueryCache,
    document: &QueryDocument,
    limit: u32,
    offset: u32,
    pin_scope_id: Option<&str>,
    prioritize_pins: bool,
) -> Result<SearchResults> {
    let started_at = Instant::now();
    let identity = document.cache_identity()?;
    let cache_key =
        format!("search:{limit}:{offset}:{identity}:{pin_scope_id:?}:{prioritize_pins}");
    if let Some(CachedResult::Search(cached)) = cache.get(&cache_key).map_err(internal_error)? {
        return Ok(cached);
    }
    let MainSnapshot {
        index,
        reader,
        query_fields: property_fields,
        ..
    } = snapshot;
    let (query, sorts, _) = compile_query_document(index, document, property_fields, pin_scope_id)?;
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
    let mut pinned_hits = Vec::new();
    let mut unpinned_hits = Vec::new();
    let page_size = usize::try_from(
        u64::from(limit.saturating_add(offset))
            .min(searcher.num_docs())
            .max(1),
    )
    .map_err(internal_error)?;
    let (ranked, pinned_ranked_len, used_fast_sort, pinned_count) =
        if let Some(scope_id) = pin_scope_id.filter(|_| prioritize_pins) {
            let pinned_base = query.box_clone();
            let unpinned_base = query.box_clone();
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

    let can_page_before_hydration = used_fast_sort || sorts.is_empty();
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
        used_fast_sort,
        "search operation completed"
    );
    cache
        .insert(cache_key, CachedResult::Search(result.clone()))
        .map_err(internal_error)?;
    Ok(result)
}
