// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Note-owned task projections in the shared index, rebuilt from item.json.

use crate::application::error::Result;
use std::collections::HashMap;
use std::time::Instant;
use tantivy::collector::{Count, DocSetCollector, TopDocs};
use tantivy::query::{BooleanQuery, Occur, TermSetQuery};
use tantivy::schema::{Schema, TantivyDocument, Value};
use tantivy::{Order, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::indexing::coordinator::MainSnapshot;
use crate::indexing::service::query_snapshot;
use crate::model::{ResultPage, TaskRecord};
use crate::projection::SearchDocument;
use crate::query::document::QueryDocument;
use crate::relations::exact_term_query;

/// All task entry points apply the same validation before touching storage.
pub(crate) fn validate_query(status: &str, limit: u32) -> Result<()> {
    if !matches!(status, "open" | "completed") {
        return Err(invalid_request("Choose open or completed tasks."));
    }
    if !(1..=200).contains(&limit) {
        return Err(invalid_request("Choose a valid task page."));
    }
    Ok(())
}

pub(crate) fn query_collection(
    index_path: &str,
    document: Option<QueryDocument>,
    note_id: Option<String>,
    status: String,
    limit: u32,
    offset: u32,
    pin_scope_id: Option<&str>,
) -> Result<ResultPage<TaskRecord>> {
    let started = Instant::now();
    validate_query(&status, limit)?;
    if note_id
        .as_deref()
        .is_some_and(|id| !crate::engine::project::identity::is_item_id(id))
    {
        return Err(invalid_request("Invalid note ID."));
    }
    let snapshot = query_snapshot(index_path)?;
    let note_ids = if let Some(document) =
        document.filter(|document| note_id.is_none() && !document.is_empty())
    {
        if let Some(pages) =
            matching_pages_if_selective(&snapshot, &document, pin_scope_id, &status)?
        {
            Some(pages)
        } else {
            let candidates = task_owner_ids(&snapshot, &status)?;
            Some(crate::search::matching_candidate_ids(
                &snapshot,
                &document,
                pin_scope_id,
                Some(&candidates),
            )?)
        }
    } else {
        None
    };
    let selected = Instant::now();
    let result = query_service(&snapshot, note_ids, note_id, status, limit, offset)?;
    tracing::debug!(
        operation = "query_tasks",
        candidate_ms = selected.duration_since(started).as_secs_f64() * 1000.0,
        query_ms = selected.elapsed().as_secs_f64() * 1000.0,
        returned = result.items.len(),
        count = result.count
    );
    Ok(result)
}

/// A selective workspace query can find its pages before touching task rows.
/// Broad queries still start from task owners, avoiding a large page ID set.
fn matching_pages_if_selective(
    snapshot: &MainSnapshot,
    document: &QueryDocument,
    pin_scope_id: Option<&str>,
    status: &str,
) -> Result<Option<Vec<String>>> {
    let schema = snapshot.index.schema();
    let (base, _, _) = crate::query::compile_query_document(
        &snapshot.index,
        document,
        &snapshot.query_fields,
        pin_scope_id,
    )?;
    let pages = BooleanQuery::intersection(vec![
        base,
        exact_term_query(
            schema.get_field("item_type").map_err(internal_error)?,
            "page",
        ),
        exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
    ]);
    let tasks = BooleanQuery::intersection(vec![
        exact_term_query(
            schema.get_field("item_type").map_err(internal_error)?,
            "task",
        ),
        exact_term_query(
            schema.get_field("task_status").map_err(internal_error)?,
            status,
        ),
        exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
    ]);
    let searcher = snapshot.reader.searcher();
    let page_count = searcher.search(&pages, &Count).map_err(internal_error)?;
    let task_count = searcher.search(&tasks, &Count).map_err(internal_error)?;
    tracing::debug!(
        operation = "plan_task_candidates",
        page_count,
        task_count,
        page_first = page_count <= task_count
    );
    if page_count > task_count {
        return Ok(None);
    }
    crate::indexing::fast::matching_ids(&searcher, &pages)
        .map(Some)
        .map_err(internal_error)
}

fn task_owner_ids(snapshot: &MainSnapshot, status: &str) -> Result<Vec<String>> {
    let schema = snapshot.index.schema();
    let query = BooleanQuery::intersection(vec![
        exact_term_query(
            schema.get_field("task_status").map_err(internal_error)?,
            status,
        ),
        exact_term_query(
            schema.get_field("item_type").map_err(internal_error)?,
            "task",
        ),
    ]);
    let searcher = snapshot.reader.searcher();
    let mut owners = std::collections::HashSet::new();
    for address in searcher
        .search(&query, &DocSetCollector)
        .map_err(internal_error)?
    {
        let column = searcher
            .segment_reader(address.segment_ord)
            .fast_fields()
            .str("note_id")
            .map_err(internal_error)?
            .ok_or_else(|| internal_error("Task owner field is missing."))?;
        if let Some(ordinal) = column.term_ords(address.doc_id).next() {
            let mut id = String::new();
            column
                .ord_to_str(ordinal, &mut id)
                .map_err(internal_error)?;
            owners.insert(id);
        }
    }
    Ok(owners.into_iter().collect())
}

fn sort_key(task: &TaskRecord) -> String {
    task.completed_at.as_ref().map_or_else(
        || {
            let due = task.due_date.as_deref().unwrap_or("9999-12-31");
            let inverse = u64::MAX
                - chrono::DateTime::parse_from_rfc3339(&task.modified_at)
                    .map_or(0, |date| date.timestamp_millis().max(0).cast_unsigned());
            format!("{due}:{inverse:020}:{}", task.task_id)
        },
        |completed| {
            let inverse = u64::MAX
                - chrono::DateTime::parse_from_rfc3339(completed)
                    .map_or(0, |date| date.timestamp_millis().max(0).cast_unsigned());
            format!("{inverse:020}:{}", task.task_id)
        },
    )
}
pub(crate) fn add_fields(
    schema: &Schema,
    entry: &mut TantivyDocument,
    document: &SearchDocument,
) -> Result<()> {
    let Some(task) = &document.task else {
        return Ok(());
    };
    for (name, value) in [
        ("note_id", task.note_id.as_str()),
        ("task_id", task.task_id.as_str()),
        (
            "task_status",
            if task.completed_at.is_some() {
                "completed"
            } else {
                "open"
            },
        ),
        ("text", task.text.as_str()),
        ("completed_at", task.completed_at.as_deref().unwrap_or("")),
        ("due_date", task.due_date.as_deref().unwrap_or("")),
    ] {
        entry.add_text(schema.get_field(name).map_err(internal_error)?, value);
    }
    entry.add_text(
        schema.get_field("sort_key").map_err(internal_error)?,
        sort_key(task),
    );
    Ok(())
}

fn decode(schema: &Schema, document: &TantivyDocument) -> Result<TaskRecord> {
    let string = |name: &str| -> Result<String> {
        let field = schema.get_field(name).map_err(internal_error)?;
        Ok(document
            .get_first(field)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned())
    };
    let optional = |name: &str| -> Result<Option<String>> {
        Ok(Some(string(name)?).filter(|value| !value.is_empty()))
    };
    Ok(TaskRecord {
        note_id: string("note_id")?,
        task_id: string("task_id")?,
        revision: string("revision")?,
        note_title: string("note_title")?,
        text: string("text")?,
        created_at: string("created_at")?,
        completed_at: optional("completed_at")?,
        due_date: optional("due_date")?,
        modified_at: string("modified_at")?,
    })
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The service consumes its decoded request or owned work at the application boundary."
)]
pub(crate) fn query_service(
    snapshot: &MainSnapshot,
    ids: Option<Vec<String>>,
    note_id: Option<String>,
    status: String,
    limit: u32,
    offset: u32,
) -> Result<ResultPage<TaskRecord>> {
    validate_query(&status, limit)?;
    let index = &snapshot.index;
    let reader = &snapshot.reader;
    let schema = index.schema();
    let mut terms = vec![
        (
            Occur::Must,
            exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
        ),
        (
            Occur::Must,
            exact_term_query(
                schema.get_field("item_type").map_err(internal_error)?,
                "task",
            ),
        ),
        (
            Occur::Must,
            exact_term_query(
                schema.get_field("task_status").map_err(internal_error)?,
                &status,
            ),
        ),
    ];
    if ids.as_ref().is_some_and(Vec::is_empty) {
        return Ok(ResultPage {
            items: Vec::new(),
            count: 0,
        });
    }
    let owners = match (&note_id, &ids) {
        (Some(id), _) => vec![id.clone()],
        (_, Some(ids)) => ids.clone(),
        _ => task_owner_ids(snapshot, &status)?,
    };
    if let Some(id) = note_id {
        terms.push((
            Occur::Must,
            exact_term_query(schema.get_field("note_id").map_err(internal_error)?, &id),
        ));
    } else if let Some(ids) = ids {
        let field = schema.get_field("note_id").map_err(internal_error)?;
        terms.push((
            Occur::Must,
            Box::new(TermSetQuery::new(
                ids.into_iter().map(|id| Term::from_field_text(field, &id)),
            )),
        ));
    }
    let parent_id = schema.get_field("id").map_err(internal_error)?;
    let parent_query = BooleanQuery::intersection(vec![
        exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
        exact_term_query(
            schema.get_field("item_type").map_err(internal_error)?,
            "page",
        ),
        Box::new(TermSetQuery::new(
            owners.iter().map(|id| Term::from_field_text(parent_id, id)),
        )),
    ]);
    let active_parents = crate::indexing::fast::matching_ids(&reader.searcher(), &parent_query)
        .map_err(internal_error)?;
    let parent_field = schema.get_field("note_id").map_err(internal_error)?;
    terms.push((
        Occur::Must,
        Box::new(TermSetQuery::new(
            active_parents
                .iter()
                .map(|id| Term::from_field_text(parent_field, id)),
        )),
    ));
    let query = BooleanQuery::new(terms);
    let searcher = reader.searcher();
    let count = searcher.search(&query, &Count).map_err(internal_error)?;
    let hits = searcher
        .search(
            &query,
            &TopDocs::with_limit(limit as usize)
                .and_offset(offset as usize)
                .order_by_string_fast_field("sort_key", Order::Asc),
        )
        .map_err(internal_error)?;
    let mut items = Vec::new();
    for (_, address) in hits {
        let doc: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        items.push(decode(&schema, &doc)?);
    }
    let titles = parent_titles(&searcher, &schema, &items)?;
    for task in &mut items {
        if let Some(title) = titles.get(&task.note_id) {
            task.note_title.clone_from(title);
        }
    }
    Ok(ResultPage { items, count })
}

fn parent_titles(
    searcher: &tantivy::Searcher,
    schema: &Schema,
    tasks: &[TaskRecord],
) -> Result<HashMap<String, String>> {
    if tasks.is_empty() {
        return Ok(HashMap::new());
    }
    let id = schema.get_field("id").map_err(internal_error)?;
    let title = schema.get_field("title").map_err(internal_error)?;
    let query = TermSetQuery::new(
        tasks
            .iter()
            .map(|task| Term::from_field_text(id, &task.note_id)),
    );
    let mut titles = HashMap::new();
    for address in searcher
        .search(&query, &DocSetCollector)
        .map_err(internal_error)?
    {
        let parent: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        if let (Some(id), Some(title)) = (
            parent.get_first(id).and_then(|value| value.as_str()),
            parent.get_first(title).and_then(|value| value.as_str()),
        ) {
            titles.insert(id.to_owned(), title.to_owned());
        }
    }
    Ok(titles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::error::ApplicationError;

    #[test]
    fn invalid_task_queries_are_rejected_before_canonical_or_index_access() {
        let temporary = tempfile::tempdir().unwrap();
        let missing_index = temporary.path().join("missing-index");
        for (status, limit, expected) in [
            ("invalid", 50, "Choose open or completed tasks."),
            ("open", 0, "Choose a valid task page."),
            ("completed", 201, "Choose a valid task page."),
        ] {
            let results = [query_collection(
                missing_index.to_str().unwrap(),
                None,
                None,
                status.into(),
                limit,
                0,
                None,
            )];
            for result in results {
                assert!(
                    matches!(result, Err(ApplicationError::InvalidRequest(message)) if message == expected)
                );
            }
        }
        assert!(!missing_index.exists());
        temporary.close().unwrap();
    }
}
