// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::application::error::Result;
use tantivy::collector::TopDocs;
use tantivy::schema::{
    FAST, FacetOptions, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument, Value,
};
use tantivy::{DateTime, Index, IndexReader, IndexWriter};

use crate::application::error::internal_error;
use crate::projection::SearchDocument;
use crate::relations::{exact_term_query, relation_facet_path, relation_facet_value_path};

pub(crate) const SCHEMA_FINGERPRINT: &str = "bluink-v19-canonical-items";

mod stamps;
pub(crate) use stamps::indexed_file_stamps;

// Store caches are per segment; writers also own an independent merge pool.
// Bound both rather than relying on throughput-oriented library defaults.
pub(crate) const DOC_STORE_CACHE_BLOCKS: usize = 8;

pub(crate) fn open_reader(index: &Index) -> tantivy::Result<IndexReader> {
    index
        .reader_builder()
        .doc_store_cache_num_blocks(DOC_STORE_CACHE_BLOCKS)
        .reload_policy(tantivy::ReloadPolicy::Manual)
        .try_into()
}

pub(crate) fn refresh_reader(index: &Index, path: &Path) -> tantivy::Result<IndexReader> {
    // A reader built from the same directory reuses its live mmap handles, so
    // touched file pages remain resident even after decoded caches are freed.
    // Keep the index's schema, tokenizers and shared segment inventory, while
    // giving the new snapshot independent mappings. Old snapshots keep their
    // mappings and segment leases until their in-flight queries finish.
    let mut fresh = index.clone();
    *fresh.directory_mut() = tantivy::directory::ManagedDirectory::wrap(Box::new(
        tantivy::directory::MmapDirectory::open(path)?,
    ))?;
    open_reader(&fresh)
}

pub(crate) fn open_writer(index: &Index) -> tantivy::Result<IndexWriter> {
    index.writer_with_options(
        tantivy::indexer::IndexWriterOptions::builder()
            .memory_budget_per_thread(15_000_000)
            .num_worker_threads(1)
            .num_merge_threads(1)
            .build(),
    )
}

pub(crate) fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    for name in [
        "note_id",
        "task_id",
        "sort_key",
        "snippet_id",
        "task_status",
    ] {
        builder.add_text_field(name, STRING | FAST | STORED);
    }
    for name in ["completed_at", "due_date", "snippet_record", "label_text"] {
        builder.add_text_field(name, STORED);
    }
    builder.add_text_field("text", TEXT | STORED);
    builder.add_text_field("note_title", TEXT | STORED);
    builder.add_text_field("key_search", STRING);
    builder.add_text_field("text_search", STRING);
    builder.add_text_field("id", STRING | FAST | STORED);
    builder.add_text_field("index_stamp", STRING | FAST | STORED);
    builder.add_text_field("revision", STRING | STORED);
    builder.add_text_field("created_at", STRING | FAST | STORED);
    builder.add_date_field("created_at_date", FAST | STORED | tantivy::schema::INDEXED);
    builder.add_text_field("title", TEXT | STORED);
    builder.add_text_field("title_sort", STRING | FAST | STORED);
    builder.add_text_field("body", TEXT);
    builder.add_text_field("preview", STORED);
    builder.add_text_field("preview_image", STRING | STORED);
    builder.add_text_field("item_type", STRING | FAST | STORED);
    builder.add_text_field("state", STRING | FAST | STORED);
    builder.add_text_field("modified_at", STRING | FAST | STORED);
    builder.add_date_field("modified_at_date", FAST | STORED | tantivy::schema::INDEXED);
    builder.add_text_field("tags", STRING | FAST | STORED);
    builder.add_text_field("labels", STRING | FAST | STORED);
    builder.add_text_field("reference_targets", STRING | FAST | STORED);
    builder.add_text_field("snippet_citation_targets", STRING | FAST | STORED);
    builder.add_text_field("snippet_embed_targets", STRING | FAST | STORED);
    builder.add_text_field("pins", STRING | FAST | STORED);
    builder.add_facet_field("facet_paths", FacetOptions::default());
    builder.add_json_field("metadata", TEXT | FAST | STORED);
    builder.add_json_field("label_counts", FAST);
    builder.add_text_field("metadata_keys", STRING);
    builder.build()
}

pub(crate) fn schema_is_current(schema: &Schema) -> bool {
    *schema == build_schema()
}
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "Index writes require exclusive access while the coordinator updates related projections."
)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the item schema encoding together so every projected field can be audited in one place."
)]
pub(crate) fn write_document(
    writer: &mut IndexWriter,
    schema: &Schema,
    document: SearchDocument,
) -> Result<()> {
    let id = schema.get_field("id").map_err(internal_error)?;
    let index_stamp = schema.get_field("index_stamp").map_err(internal_error)?;
    let revision = schema.get_field("revision").map_err(internal_error)?;
    let created_at = schema.get_field("created_at").map_err(internal_error)?;
    let created_at_date = schema
        .get_field("created_at_date")
        .map_err(internal_error)?;
    let title = schema.get_field("title").map_err(internal_error)?;
    let title_sort = schema.get_field("title_sort").map_err(internal_error)?;
    let body = schema.get_field("body").map_err(internal_error)?;
    let preview = schema.get_field("preview").map_err(internal_error)?;
    let preview_image = schema.get_field("preview_image").map_err(internal_error)?;
    let item_type = schema.get_field("item_type").map_err(internal_error)?;
    let state = schema.get_field("state").map_err(internal_error)?;
    let modified_at = schema.get_field("modified_at").map_err(internal_error)?;
    let modified_at_date = schema
        .get_field("modified_at_date")
        .map_err(internal_error)?;
    let tags = schema.get_field("tags").map_err(internal_error)?;
    let labels = schema.get_field("labels").map_err(internal_error)?;
    let reference_targets = schema
        .get_field("reference_targets")
        .map_err(internal_error)?;
    let pins = schema.get_field("pins").map_err(internal_error)?;
    let facet_paths = schema.get_field("facet_paths").map_err(internal_error)?;
    let metadata = schema.get_field("metadata").map_err(internal_error)?;
    let mut output = TantivyDocument::default();
    crate::tasks::add_fields(schema, &mut output, &document)?;
    crate::snippets::add_fields(schema, &mut output, &document)?;
    output.add_text(
        schema.get_field("label_text").map_err(internal_error)?,
        serde_json::to_string(&document.passages).map_err(internal_error)?,
    );
    let mut label_counts = BTreeMap::<String, OwnedValue>::new();
    for passage in &document.passages {
        for label in &passage.labels {
            let value = label_counts
                .entry(label.clone())
                .or_insert(OwnedValue::U64(0));
            if let OwnedValue::U64(count) = value {
                *count += 1;
            }
        }
    }
    output.add_object(
        schema.get_field("label_counts").map_err(internal_error)?,
        label_counts,
    );
    output.add_text(id, document.id);
    output.add_text(index_stamp, document.index_stamp);
    output.add_text(revision, document.revision);
    output.add_text(created_at, &document.created_at);
    if let Ok(value) = chrono::DateTime::parse_from_rfc3339(&document.created_at) {
        output.add_date(
            created_at_date,
            DateTime::from_timestamp_millis(value.timestamp_millis()),
        );
    }
    output.add_text(title, document.title);
    output.add_text(title_sort, document.title_sort);
    output.add_text(body, document.body);
    output.add_text(preview, document.preview);
    if let Some(value) = document.preview_image {
        output.add_text(preview_image, value);
    }
    output.add_text(item_type, document.item_type);
    output.add_text(state, document.state);
    output.add_text(modified_at, &document.modified_at);
    if let Ok(value) = chrono::DateTime::parse_from_rfc3339(&document.modified_at) {
        output.add_date(
            modified_at_date,
            DateTime::from_timestamp_millis(value.timestamp_millis()),
        );
    }
    for value in &document.tags {
        output.add_text(tags, value.as_str());
    }
    for value in &document.labels {
        output.add_text(labels, value.as_str());
    }
    for value in &document.reference_targets {
        output.add_text(reference_targets, value.as_str());
    }
    for (name, values) in [
        (
            "snippet_citation_targets",
            &document.snippet_citation_targets,
        ),
        ("snippet_embed_targets", &document.snippet_embed_targets),
    ] {
        let field = schema.get_field(name).map_err(internal_error)?;
        for value in values {
            output.add_text(field, value);
        }
    }
    for value in &document.pins {
        output.add_text(pins, value.as_str());
    }
    for (facet_kind, targets) in [("tag", &document.tags), ("label", &document.labels)] {
        for target in targets {
            if let Some(facet) = relation_facet_path(facet_kind, target) {
                output.add_facet(facet_paths, facet);
            }
            if let Some(facet) = relation_facet_value_path(facet_kind, target) {
                output.add_facet(facet_paths, facet);
            }
        }
    }
    let metadata_keys = schema.get_field("metadata_keys").map_err(internal_error)?;
    if let Some(metadata) = document.metadata.as_object() {
        for (key, _) in metadata.iter().filter(|(_, value)| !value.is_null()) {
            output.add_text(metadata_keys, key);
        }
    }
    let values: BTreeMap<String, OwnedValue> =
        serde_json::from_value(document.metadata).map_err(internal_error)?;
    output.add_object(metadata, values);
    writer.add_document(output).map_err(internal_error)?;
    Ok(())
}

pub(crate) fn indexed_revision(
    reader: &IndexReader,
    schema: &Schema,
    id: &str,
) -> Result<Option<String>> {
    let id_field = schema.get_field("id").map_err(internal_error)?;
    let revision_field = schema.get_field("revision").map_err(internal_error)?;
    let searcher = reader.searcher();
    let addresses = searcher
        .search(
            &exact_term_query(id_field, id),
            &TopDocs::with_limit(1).order_by_score(),
        )
        .map_err(internal_error)?;
    let Some((_, address)) = addresses.into_iter().next() else {
        return Ok(None);
    };
    let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
    Ok(document
        .get_first(revision_field)
        .and_then(|value| value.as_str())
        .map(str::to_owned))
}

pub(crate) fn open_or_create_index(path: &Path) -> std::result::Result<(Index, bool), String> {
    open_or_create_projection(path, build_schema())
}

/// Only disposable projection directories may be passed here.
pub(crate) fn open_or_create_projection(
    path: &Path,
    schema: Schema,
) -> std::result::Result<(Index, bool), String> {
    match Index::open_in_dir(path) {
        Ok(index) if index.schema() == schema => Ok((index, false)),
        _ => {
            if path.exists() {
                fs::remove_dir_all(path).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(path).map_err(|error| error.to_string())?;
            Index::create_in_dir(path, schema)
                .map(|index| (index, true))
                .map_err(|error| error.to_string())
        }
    }
}

pub(crate) fn index_is_cold(path: &Path) -> bool {
    Index::open_in_dir(path).map_or(true, |index| !schema_is_current(&index.schema()))
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| {
            u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(crate) fn item_count(reader: &IndexReader) -> Result<usize> {
    usize::try_from(reader.searcher().num_docs()).map_err(internal_error)
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn schema_checks_field_options_not_only_names() {
        let schema = build_schema();
        assert!(schema_is_current(&schema));
        let mut fields = serde_json::to_value(&schema).unwrap();
        let title = fields
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|field| field["name"] == "title")
            .unwrap();
        title["options"]["stored"] = serde_json::json!(false);
        let changed: Schema = serde_json::from_value(fields).unwrap();
        assert!(changed.get_field("title").is_ok());
        assert!(!schema_is_current(&changed));
    }

    #[test]
    fn projection_reuses_current_schema_and_rebuilds_incompatible_schema() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("index");
        let (index, created) = open_or_create_index(&path).unwrap();
        assert!(created);
        drop(index);
        assert!(!index_is_cold(&path));
        let (index, created) = open_or_create_index(&path).unwrap();
        assert!(!created);
        drop(index);
        let mut builder = Schema::builder();
        builder.add_text_field("different", STRING);
        let changed = builder.build();
        let (index, created) = open_or_create_projection(&path, changed.clone()).unwrap();
        assert!(created);
        assert_eq!(index.schema(), changed);
        drop(index);
        assert!(index_is_cold(&path));
        let (index, created) = open_or_create_index(&path).unwrap();
        assert!(created);
        assert!(schema_is_current(&index.schema()));
    }
}
