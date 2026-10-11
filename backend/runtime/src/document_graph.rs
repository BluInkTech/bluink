// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Capture one index generation, then overlay changed canonical files. Neither
//! pending writes nor files changed outside Bluink may hide relationships.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::engine::document::projection::project_document;
use crate::engine::project::item::read_project_item;
use serde_json::Value as JsonValue;
use tantivy::DocAddress;
use tantivy::Term;
use tantivy::collector::DocSetCollector;
use tantivy::query::{BooleanQuery, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::schema::{TantivyDocument, Value};

use crate::project::{all_item_paths, file_stamp, index_stamp, item_id_from_path};

#[derive(Default)]
pub(crate) struct Edges {
    pub(crate) pages: Vec<String>,
}

pub(crate) fn snapshot(root: &Path) -> Result<Option<HashMap<String, Edges>>, String> {
    let Some((index, reader, dirty)) =
        crate::service::project_index(root).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let dirty: HashSet<_> = dirty
        .lock()
        .map_err(|error| error.to_string())?
        .keys()
        .cloned()
        .collect();
    let schema = index.schema();
    let fields = ["id", "item_type", "reference_targets"]
        .map(|name| schema.get_field(name).map_err(|error| error.to_string()))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let searcher = reader.searcher();
    let mut edges = HashMap::new();
    let mut stamps = HashMap::new();
    for (ordinal, segment) in searcher.segment_readers().iter().enumerate() {
        let fast_stamps = segment
            .fast_fields()
            .str("index_stamp")
            .map_err(|error| error.to_string())?
            .ok_or("Index stamp field is missing.")?;
        for doc_id in segment.doc_ids_alive() {
            let doc: TantivyDocument = searcher
                .doc(DocAddress::new(
                    u32::try_from(ordinal).map_err(|error| error.to_string())?,
                    doc_id,
                ))
                .map_err(|error| error.to_string())?;
            let Some(id) = doc.get_first(fields[0]).and_then(|value| value.as_str()) else {
                continue;
            };
            let mut stamp = String::new();
            if let Some(term) = fast_stamps.term_ords(doc_id).next() {
                fast_stamps
                    .ord_to_str(term, &mut stamp)
                    .map_err(|error| error.to_string())?;
            }
            stamps.insert(id.to_owned(), stamp);
            if doc.get_first(fields[1]).and_then(|value| value.as_str()) != Some("page") {
                continue;
            }
            let targets = |field| {
                doc.get_all(field)
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            };
            edges.insert(
                id.to_owned(),
                Edges {
                    pages: targets(fields[2]),
                },
            );
        }
    }
    let mut present = HashSet::new();
    for path in all_item_paths(root)? {
        let id = item_id_from_path(&path)?;
        present.insert(id.clone());
        let current_stamp = index_stamp(&file_stamp(&path)?);
        if stamps.get(&id) == Some(&current_stamp) && !dirty.contains(&id) {
            continue;
        }
        let Some(item) = read_project_item(root, &id)? else {
            edges.remove(&id);
            continue;
        };
        if item["meta"]["type"] != "page" {
            edges.remove(&id);
            continue;
        }
        let projection = project_document(
            item["content"]
                .get("doc")
                .unwrap_or_else(|| &item["content"]),
        );
        edges.insert(
            id,
            Edges {
                pages: projection.page_links,
            },
        );
    }
    edges.retain(|id, _| present.contains(id));
    Ok(Some(edges))
}

pub(crate) fn snippet_usages(root: &Path, id: &str) -> Result<Vec<JsonValue>, String> {
    snippet_usages_inner(root, id, false)
}

fn snippet_query(schema: &tantivy::schema::Schema, id: &str) -> Result<BooleanQuery, String> {
    Ok(BooleanQuery::union(
        ["snippet_citation_targets", "snippet_embed_targets"]
            .into_iter()
            .map(|name| {
                let field = schema.get_field(name).map_err(|error| error.to_string())?;
                Ok(Box::new(TermQuery::new(
                    Term::from_field_text(field, id),
                    IndexRecordOption::Basic,
                )) as Box<dyn tantivy::query::Query>)
            })
            .collect::<Result<Vec<_>, String>>()?,
    ))
}

/// Counts need no note text or occurrence lists for unchanged indexed pages.
pub(crate) fn snippet_usage_count(root: &Path, id: &str) -> Result<usize, String> {
    crate::engine::project::snippets::validate_snippet_id(id)?;
    let Some((index, reader, dirty)) =
        crate::service::project_index(root).map_err(|error| error.to_string())?
    else {
        return Ok(crate::engine::project::snippets::snippet_usages(root, id)?.len());
    };
    let mut changed: HashSet<String> = dirty
        .lock()
        .map_err(|error| error.to_string())?
        .keys()
        .cloned()
        .collect();
    let pending = changed.clone();
    let searcher = reader.searcher();
    let query = snippet_query(&index.schema(), id)?;
    let mut unchanged = HashSet::new();
    let (mut page_id, mut stamp, mut kind) = (String::new(), String::new(), String::new());
    for address in searcher
        .search(&query, &DocSetCollector)
        .map_err(|error| error.to_string())?
    {
        let fields = searcher.segment_reader(address.segment_ord).fast_fields();
        let ids = fields
            .str("id")
            .map_err(|error| error.to_string())?
            .ok_or("Missing item ID fast field")?;
        let stamps = fields
            .str("index_stamp")
            .map_err(|error| error.to_string())?
            .ok_or("Missing item stamp fast field")?;
        let kinds = fields
            .str("item_type")
            .map_err(|error| error.to_string())?
            .ok_or("Missing item type fast field")?;
        let (Some(id_ord), Some(stamp_ord), Some(kind_ord)) = (
            ids.term_ords(address.doc_id).next(),
            stamps.term_ords(address.doc_id).next(),
            kinds.term_ords(address.doc_id).next(),
        ) else {
            continue;
        };
        ids.ord_to_str(id_ord, &mut page_id)
            .map_err(|error| error.to_string())?;
        stamps
            .ord_to_str(stamp_ord, &mut stamp)
            .map_err(|error| error.to_string())?;
        kinds
            .ord_to_str(kind_ord, &mut kind)
            .map_err(|error| error.to_string())?;
        if kind != "page" {
            continue;
        }
        let Some(current) =
            crate::project::FileStamp::read(&crate::project::item_path(root, &page_id))?
        else {
            continue;
        };
        if !pending.contains(&page_id)
            && crate::project::FileStamp::parse_index(&stamp) == Some(current)
        {
            unchanged.insert(page_id.clone());
        } else {
            changed.insert(page_id.clone());
        }
    }
    unchanged.retain(|id| !changed.contains(id));
    let changed = changed
        .into_iter()
        .filter(|id| crate::project::item_path(root, id).is_file())
        .collect::<Vec<_>>();
    Ok(unchanged.len()
        + crate::engine::project::snippets::snippet_usages_in_pages(root, id, &changed)?.len())
}

pub(crate) fn verified_snippet_usages(root: &Path, id: &str) -> Result<Vec<JsonValue>, String> {
    snippet_usages_inner(root, id, true)
}

fn snippet_usages_inner(
    root: &Path,
    id: &str,
    verify_files: bool,
) -> Result<Vec<JsonValue>, String> {
    crate::engine::project::snippets::validate_snippet_id(id)?;
    let Some((index, reader, dirty)) =
        crate::service::project_index(root).map_err(|error| error.to_string())?
    else {
        return crate::engine::project::snippets::snippet_usages(root, id);
    };
    // Capture dirty IDs before the reader: a committed write is represented by
    // either this overlay or the captured index generation (and possibly both).
    let mut ids: HashSet<String> = dirty
        .lock()
        .map_err(|error| error.to_string())?
        .keys()
        .cloned()
        .collect();
    let schema = index.schema();
    let id_field = schema.get_field("id").map_err(|error| error.to_string())?;
    let query = snippet_query(&schema, id)?;
    let searcher = reader.searcher();
    if verify_files {
        let stamps = crate::index::indexed_stamps_in_searcher(&searcher, &schema, |_, _| {})?;
        for path in all_item_paths(root)? {
            let page_id = item_id_from_path(&path)?;
            if stamps.get(&page_id) != Some(&index_stamp(&file_stamp(&path)?)) {
                ids.insert(page_id);
            }
        }
    }
    for address in searcher
        .search(&query, &DocSetCollector)
        .map_err(|error| error.to_string())?
    {
        let doc: TantivyDocument = searcher.doc(address).map_err(|error| error.to_string())?;
        if let Some(id) = doc.get_first(id_field).and_then(|value| value.as_str()) {
            ids.insert(id.to_owned());
        }
    }
    // Read only matching/pending pages. Canonical content corrects removed
    // references, changed titles and files removed before the watcher catches up.
    let mut ids: Vec<_> = ids
        .into_iter()
        .filter(|id| {
            root.join("items")
                .join(&id[..2])
                .join(id)
                .join("item.json")
                .is_file()
        })
        .collect();
    ids.sort();
    crate::engine::project::snippets::snippet_usages_in_pages(root, id, &ids)
}
