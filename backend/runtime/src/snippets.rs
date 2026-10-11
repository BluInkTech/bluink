// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Disposable search projection. Canonical files remain the integrity authority.
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;

use crate::application::error::{Result as ApplicationResult, internal_error, invalid_request};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tantivy::collector::{Count, DocSetCollector, TopDocs};
use tantivy::query::{AllQuery, Query, RegexQuery};
use tantivy::schema::{FAST, STORED, STRING, Schema, TantivyDocument, Value};
use tantivy::{Index, IndexReader, IndexWriter, Order};

type Stamps = Vec<(PathBuf, u64, SystemTime)>;
struct Projection {
    index: Index,
    reader: IndexReader,
    stamps: Option<Stamps>,
}
type SharedProjection = Arc<Mutex<Projection>>;
// Weak registry coordinates readers for handles of the same project without
// extending their lifetime past their application's owned reference.
static PROJECTIONS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<Projection>>>>> = OnceLock::new();

#[derive(Default)]
pub(crate) struct SnippetIndex(Mutex<Option<SharedProjection>>);

impl SnippetIndex {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep the shared projection owned throughout the read or rebuild operation."
    )]
    fn acquire(&self, root: &Path) -> Result<SharedProjection, String> {
        let mut owned = self.0.lock().map_err(|error| error.to_string())?;
        if let Some(projection) = owned.as_ref() {
            return Ok(projection.clone());
        }
        let projection = projection(root)?;
        *owned = Some(projection.clone());
        Ok(projection)
    }

    pub(crate) fn stop(&self) -> ApplicationResult<()> {
        self.0.lock().map_err(internal_error)?.take();
        Ok(())
    }
}

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

fn schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("id", STRING | FAST | STORED);
    builder.add_text_field("key_search", STRING);
    builder.add_text_field("text_search", STRING);
    builder.add_text_field("record", STORED);
    builder.build()
}
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
fn stamps(root: &Path) -> Result<Stamps, String> {
    let directory = root.join("snippets");
    if fs::symlink_metadata(&directory).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("The snippet directory cannot be a symbolic link.".to_owned());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if !name
            .to_str()
            .is_some_and(|name| name.starts_with("snippet_") && name.ends_with(".json"))
        {
            continue;
        }
        let meta = entry.metadata().map_err(|error| error.to_string())?;
        result.push((
            entry.path(),
            meta.len(),
            meta.modified().map_err(|error| error.to_string())?,
        ));
    }
    result.sort_unstable();
    Ok(result)
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the shared projection owned throughout the read or rebuild operation."
)]
fn projection(root: &Path) -> Result<SharedProjection, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut projections = PROJECTIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|error| error.to_string())?;
    projections.retain(|_, entry| entry.strong_count() > 0);
    if let Some(existing) = projections.get(&root).and_then(Weak::upgrade) {
        return Ok(existing);
    }
    let path = root.join(".cache/snippets");
    let index = match Index::open_in_dir(&path) {
        Ok(index) if index.schema() == schema() => index,
        _ => {
            if path.exists() {
                fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(&path).map_err(|error| error.to_string())?;
            Index::create_in_dir(&path, schema()).map_err(|error| error.to_string())?
        }
    };
    let reader = index
        .reader_builder()
        .doc_store_cache_num_blocks(crate::index::DOC_STORE_CACHE_BLOCKS)
        .reload_policy(tantivy::ReloadPolicy::Manual)
        .try_into()
        .map_err(|error| error.to_string())?;
    let projection = Arc::new(Mutex::new(Projection {
        index,
        reader,
        stamps: None,
    }));
    projections.insert(root, Arc::downgrade(&projection));
    Ok(projection)
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

// Initial file stamps are process-local. Compare the existing projection with
// canonical records before allocating a writer just to reproduce identical data.
fn matches_records(projection: &Projection, records: &[JsonValue]) -> ApplicationResult<bool> {
    let mut expected = records
        .iter()
        .filter(|record| record["state"] == "active")
        .map(|record| {
            Ok((
                record["id"]
                    .as_str()
                    .ok_or_else(|| internal_error("Snippet has no key"))?,
                serde_json::to_string(record).map_err(internal_error)?,
            ))
        })
        .collect::<ApplicationResult<HashMap<_, _>>>()?;
    let searcher = projection.reader.searcher();
    if usize::try_from(searcher.num_docs()).ok() != Some(expected.len()) {
        return Ok(false);
    }
    let schema = projection.index.schema();
    let id = schema.get_field("id").map_err(internal_error)?;
    let record = schema.get_field("record").map_err(internal_error)?;
    for address in searcher
        .search(&AllQuery, &DocSetCollector)
        .map_err(internal_error)?
    {
        let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
        let Some(key) = document.get_first(id).and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        let cached = document.get_first(record).and_then(|value| value.as_str());
        if expected.remove(key).as_deref() != cached {
            return Ok(false);
        }
    }
    Ok(expected.is_empty())
}

fn rebuild(projection: &Projection, records: Vec<JsonValue>) -> ApplicationResult<()> {
    let schema = projection.index.schema();
    let id = schema.get_field("id").map_err(internal_error)?;
    let key_search = schema.get_field("key_search").map_err(internal_error)?;
    let text_search = schema.get_field("text_search").map_err(internal_error)?;
    let record_field = schema.get_field("record").map_err(internal_error)?;
    let mut writer: IndexWriter =
        crate::index::open_writer(&projection.index).map_err(internal_error)?;
    writer.delete_all_documents().map_err(internal_error)?;
    for record in records {
        if record["state"] != "active" {
            continue;
        }
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
        let mut entry = TantivyDocument::default();
        entry.add_text(id, key);
        entry.add_text(key_search, key);
        entry.add_text(text_search, text);
        entry.add_text(
            record_field,
            serde_json::to_string(&record).map_err(internal_error)?,
        );
        writer.add_document(entry).map_err(internal_error)?;
    }
    writer.commit().map_err(internal_error)?;
    projection.reader.reload().map_err(internal_error)
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep the shared projection owned throughout the read or rebuild operation."
)]
pub(crate) fn query(
    owned: &SnippetIndex,
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
    let shared = owned.acquire(root).map_err(internal_error)?;
    let mut projection = shared.lock().map_err(internal_error)?;
    let current = stamps(root).map_err(internal_error)?;
    let schema = projection.index.schema();
    let key_search = schema.get_field("key_search").map_err(internal_error)?;
    let text_search = schema.get_field("text_search").map_err(internal_error)?;
    let record_field = schema.get_field("record").map_err(internal_error)?;
    if projection.stamps.as_ref() != Some(&current) {
        let records =
            crate::engine::project::snippets::list_snippets(root).map_err(internal_error)?;
        if projection.stamps.is_some() || !matches_records(&projection, &records)? {
            rebuild(&projection, records)?;
        }
        projection.stamps = Some(current);
    }
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
    let searcher = projection.reader.searcher();
    let count = searcher.search(&query, &Count).map_err(internal_error)?;
    let hits = searcher
        .search(
            &query,
            &TopDocs::with_limit(limit as usize)
                .and_offset(offset as usize)
                .order_by_string_fast_field("id", Order::Asc),
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
        let usage_count =
            crate::document_graph::snippet_usage_count(root, &record.id).map_err(internal_error)?;
        items.push(SnippetHit {
            record,
            usage_count,
        });
    }
    Ok(SnippetResults { items, count })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn warm_projection_reuses_identical_records_without_acquiring_a_writer() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(root.path(), "UTC").unwrap();
        crate::engine::project::snippets::write_snippet(
            root.path(),
            "source",
            json!({"type":"paragraph"}),
            None,
        )
        .unwrap();
        let first = SnippetIndex::default();
        assert_eq!(query(&first, root.path(), "", 50, 0).unwrap().count, 1);
        first.stop().unwrap();
        let index = Index::open_in_dir(root.path().join(".cache/snippets")).unwrap();
        let _writer = index
            .writer_with_num_threads::<TantivyDocument>(1, 15_000_000)
            .unwrap();
        let second = SnippetIndex::default();
        assert_eq!(query(&second, root.path(), "", 50, 0).unwrap().count, 1);
        second.stop().unwrap();
    }

    #[test]
    fn application_reader_ownership_survives_other_handle_stop_and_releases_without_main_index() {
        let root =
            std::env::temp_dir().join(format!("bluink-snippet-readers-{}", std::process::id()));
        crate::engine::project::config::open_or_create_project(&root, "UTC").unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "source",
            json!({"type":"paragraph"}),
            None,
        )
        .unwrap();
        let first = SnippetIndex::default();
        let second = SnippetIndex::default();
        assert_eq!(query(&first, &root, "", 50, 0).unwrap().count, 1);
        assert_eq!(query(&second, &root, "", 50, 0).unwrap().count, 1);
        let first_reader = first.acquire(&root).unwrap();
        let second_reader = second.acquire(&root).unwrap();
        assert!(Arc::ptr_eq(&first_reader, &second_reader));
        let weak = Arc::downgrade(&first_reader);
        drop(first_reader);
        drop(second_reader);
        first.stop().unwrap();
        assert!(weak.upgrade().is_some());
        assert_eq!(query(&second, &root, "", 50, 0).unwrap().count, 1);
        second.stop().unwrap();
        assert!(weak.upgrade().is_none());
        fs::remove_dir_all(root.join(".cache/snippets")).unwrap();
        let third = SnippetIndex::default();
        assert_eq!(query(&third, &root, "", 50, 0).unwrap().count, 1);
        third.stop().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
