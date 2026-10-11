// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Disposable, note-owned follow-up index. Every document is rebuilt from item.json.
use std::fs;
use std::path::{Path, PathBuf};

use crate::application::error::Result;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, TermSetQuery};
use tantivy::schema::{FAST, STORED, STRING, Schema, TantivyDocument, Value};
use tantivy::{Index, IndexWriter, Order, Term};

use crate::application::error::{internal_error, invalid_request};
use crate::model::{ResultPage, SearchDocument, TaskRecord};
use crate::relations::exact_term_query;
use crate::service::service_task_index;

pub(crate) fn query_collection(
    index_path: String,
    query_text: Option<String>,
    note_id: Option<String>,
    status: String,
    limit: u32,
    offset: u32,
) -> Result<ResultPage<TaskRecord>> {
    let note_ids = if note_id.is_some()
        || query_text.as_deref().is_none_or(|query| {
            query.trim().is_empty() || query.trim() == "+type:page +state:active sort:modified-desc"
        }) {
        None
    } else {
        Some(crate::search::matching_item_ids_service_sync(
            index_path.clone(),
            query_text.unwrap(),
        )?)
    };
    query_service(index_path, note_ids, note_id, status, limit, offset)
}

pub(crate) fn index_path(main_index_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-tasks", main_index_path.display()))
}

fn schema() -> Schema {
    let mut builder = Schema::builder();
    for name in [
        "note_id",
        "index_stamp",
        "record_kind",
        "task_id",
        "state",
        "sort_key",
    ] {
        builder.add_text_field(name, STRING | FAST | STORED);
    }
    for name in [
        "revision",
        "note_title",
        "text",
        "created_at",
        "completed_at",
        "due_date",
        "modified_at",
    ] {
        builder.add_text_field(name, STORED);
    }
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
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Index writes require exclusive access while the coordinator updates related projections."
)]
pub(crate) fn write_document(
    writer: &mut IndexWriter,
    schema: &Schema,
    document: &SearchDocument,
) -> Result<()> {
    let note_id = schema.get_field("note_id").map_err(internal_error)?;
    let mut marker = TantivyDocument::default();
    writer.delete_term(Term::from_field_text(note_id, &document.id));
    marker.add_text(note_id, &document.id);
    marker.add_text(
        schema.get_field("index_stamp").map_err(internal_error)?,
        &document.index_stamp,
    );
    marker.add_text(
        schema.get_field("record_kind").map_err(internal_error)?,
        "marker",
    );
    writer.add_document(marker).map_err(internal_error)?;
    for task in &document.tasks {
        let mut entry = TantivyDocument::default();
        let mut add = |name: &str, value: &str| -> Result<()> {
            entry.add_text(schema.get_field(name).map_err(internal_error)?, value);
            Ok(())
        };
        add("note_id", &task.note_id)?;
        add("record_kind", "task")?;
        add("task_id", &task.task_id)?;
        add(
            "state",
            if task.completed_at.is_some() {
                "completed"
            } else {
                "open"
            },
        )?;
        add("sort_key", &sort_key(task))?;
        add("revision", &task.revision)?;
        add("note_title", &task.note_title)?;
        add("text", &task.text)?;
        add("created_at", &task.created_at)?;
        add(
            "completed_at",
            task.completed_at.as_deref().unwrap_or_default(),
        )?;
        add("due_date", task.due_date.as_deref().unwrap_or_default())?;
        add("modified_at", &task.modified_at)?;
        writer.add_document(entry).map_err(internal_error)?;
    }
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
    index_path: String,
    ids: Option<Vec<String>>,
    note_id: Option<String>,
    status: String,
    limit: u32,
    offset: u32,
) -> Result<ResultPage<TaskRecord>> {
    if limit == 0 || limit > 200 || !matches!(status.as_str(), "open" | "completed") {
        return Err(invalid_request(
            "Choose open or completed tasks and a limit between 1 and 200",
        ));
    }
    let (index, reader) = service_task_index(&index_path)?;
    let schema = index.schema();
    let mut terms = vec![
        (
            Occur::Must,
            exact_term_query(
                schema.get_field("record_kind").map_err(internal_error)?,
                "task",
            ),
        ),
        (
            Occur::Must,
            exact_term_query(schema.get_field("state").map_err(internal_error)?, &status),
        ),
    ];
    if ids.as_ref().is_some_and(Vec::is_empty) {
        return Ok(ResultPage {
            items: Vec::new(),
            count: 0,
        });
    }
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
    Ok(ResultPage { items, count })
}
