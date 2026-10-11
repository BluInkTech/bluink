// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::engine::document::projection::{bounded_preview, project_document};
use crate::engine::project::item::{list_project_item_ids, read_item};
use rayon::prelude::*;
use serde_json::Value as JsonValue;

use crate::model::{SearchDocument, TaskRecord, empty_object};
use crate::passages;

pub(crate) fn item_path(root: &Path, id: &str) -> PathBuf {
    root.join("items")
        .join(id[0..2].to_lowercase())
        .join(id)
        .join("item.json")
}

fn string_at(value: &JsonValue, key: &str) -> String {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn string_array(value: Option<&JsonValue>) -> Vec<String> {
    value
        .and_then(JsonValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn read_document(
    path: &Path,
    index_stamp: String,
) -> std::result::Result<SearchDocument, String> {
    let id = item_id_from_path(path)?;
    let value = read_item(path, &id)?;
    let meta = value.get("meta").cloned().unwrap_or_else(empty_object);
    // Canonical parsing already validates content. Projection only reads it;
    // cloning the entire document doubles the largest per-item allocation.
    let content = &value["content"];
    let projection = project_document(content);
    let passages = if string_at(&meta, "type") == "page" && string_at(&meta, "state") == "active" {
        passages::project(
            &id,
            &string_at(&value, "revision"),
            &string_at(&meta, "title"),
            content,
        )
    } else {
        Vec::new()
    };
    let tasks = if string_at(&meta, "type") == "page" && string_at(&meta, "state") == "active" {
        value
            .get("tasks")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(|task| {
                let task_id = task.get("id")?.as_str()?.to_owned();
                Some(TaskRecord {
                    note_id: id.clone(),
                    task_id,
                    revision: string_at(&value, "revision"),
                    note_title: string_at(&meta, "title"),
                    text: string_at(task, "text"),
                    created_at: string_at(task, "createdAt"),
                    completed_at: task
                        .get("completedAt")
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned),
                    due_date: meta
                        .get("due_date")
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned),
                    modified_at: string_at(&value, "modifiedAt"),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut reference_targets = projection.page_links;
    let mut tags = string_array(value.get("tags"));
    for values in [&mut tags, &mut reference_targets] {
        values.sort();
        values.dedup();
    }
    let mut pins = string_array(value.get("pins"));
    pins.sort();
    pins.dedup();
    Ok(SearchDocument {
        id,
        index_stamp,
        revision: string_at(&value, "revision"),
        created_at: string_at(&value, "createdAt"),
        modified_at: string_at(&value, "modifiedAt"),
        title: string_at(&meta, "title"),
        title_sort: string_at(&meta, "title").to_lowercase(),
        preview: bounded_preview(&projection.text),
        preview_image: projection.preview_image,
        body: projection.text,
        item_type: string_at(&meta, "type"),
        state: string_at(&meta, "state"),
        tags,
        labels: projection.labels,
        reference_targets,
        snippet_citation_targets: projection.snippet_citations,
        snippet_embed_targets: projection.snippet_embeds,
        passages,
        tasks,
        pins,
        metadata: meta,
    })
}

pub(crate) fn all_item_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    // Canonical IDs need only the shard directories, not a recursive walk of
    // every item's assets and history. Check candidate files concurrently;
    // an externally deleted item.json may leave its directory behind.
    let candidates = list_project_item_ids(root)?;
    let paths = crate::parallel::files(|| {
        candidates
            .par_iter()
            .map(|id| {
                let path = item_path(root, id);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) => Ok(metadata.is_file().then_some(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(format!("{}: {error}", path.display())),
                }
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    Ok(paths.into_iter().flatten().collect())
}

pub(crate) fn item_id_from_path(path: &Path) -> std::result::Result<String, String> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .filter(|value| value.len() == 36)
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no item UUID parent", path.display()))
}

pub(crate) fn file_stamp(path: &Path) -> std::result::Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    Ok(format!("{}:{modified}", metadata.len()))
}

pub(crate) fn index_stamp(item_stamp: &str) -> String {
    format!("v15:{item_stamp}")
}

/// A validated canonical identifier without a separate heap allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ItemId([u8; 36]);

impl ItemId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if !crate::engine::project::item::is_item_id(value) {
            return None;
        }
        Some(Self(value.as_bytes().try_into().ok()?))
    }

    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("Validated ASCII item ID")
    }
}

/// Exact metadata comparison without allocating strings for every cached item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    size: u64,
    modified: Duration,
}

impl FileStamp {
    pub(crate) fn read(path: &Path) -> Result<Option<Self>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if !metadata.is_file() {
            return Ok(None);
        }
        Ok(Some(Self {
            size: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|error| error.to_string())?
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?,
        }))
    }

    pub(crate) fn parse_index(value: &str) -> Option<Self> {
        let (size, nanos) = value.strip_prefix("v15:")?.split_once(':')?;
        let nanos: u128 = nanos.parse().ok()?;
        Some(Self {
            size: size.parse().ok()?,
            modified: Duration::new(
                u64::try_from(nanos / 1_000_000_000).ok()?,
                u32::try_from(nanos % 1_000_000_000).ok()?,
            ),
        })
    }

    pub(crate) fn indexed(self) -> String {
        index_stamp(&format!("{}:{}", self.size, self.modified.as_nanos()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_metadata_preserves_identifier_case_and_nanosecond_precision() {
        let id = "ABCDEF01-2345-7000-ABCD-EF0123456789";
        assert_eq!(ItemId::parse(id).unwrap().as_str(), id);
        assert!(ItemId::parse("invalid").is_none());
        for value in [
            "v15:0:0",
            "v15:123456:1791627717754000001",
            "v15:18446744073709551615:18446744073709551615000000001",
        ] {
            assert_eq!(FileStamp::parse_index(value).unwrap().indexed(), value);
        }
        assert!(FileStamp::parse_index("v14:1:2").is_none());
        assert!(FileStamp::parse_index("v15:1:invalid").is_none());
    }

    #[test]
    fn native_index_reads_portable_fixture_through_core_projection() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "test-fixtures/pdf-project/items/01/01a0e2a0-0000-7000-8000-000000000001/item.json",
        );
        let item = read_document(&fixture, "fixture-stamp".to_owned()).unwrap();
        assert_eq!(item.id, "01a0e2a0-0000-7000-8000-000000000001");
        assert_ne!(item.body, "");
        assert_eq!(item.index_stamp, "fixture-stamp");
    }

    #[test]
    fn discovery_uses_canonical_files_and_ignores_history_assets_and_deleted_items() {
        let root = std::env::temp_dir().join(format!(
            "bluink-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let id = "01a0e2a0-0000-7000-8000-000000000001";
        let deleted_id = "01a0e2a0-0000-7000-8000-000000000002";
        let canonical = item_path(&root, id);
        for relative in [
            format!("items/01/{id}"),
            format!("items/01/{id}/history/{id}"),
            format!("items/01/{id}/assets/{id}"),
            format!("items/_system/{id}"),
            format!("items/01/{deleted_id}"),
        ] {
            std::fs::create_dir_all(root.join(relative)).unwrap();
        }
        for relative in [
            format!("items/01/{id}/item.json"),
            format!("items/01/{id}/history/{id}/item.json"),
            format!("items/01/{id}/assets/{id}/item.json"),
            format!("items/_system/{id}/item.json"),
        ] {
            std::fs::write(root.join(relative), "{}").unwrap();
        }
        assert_eq!(all_item_paths(&root).unwrap(), vec![canonical]);
        std::fs::remove_dir_all(root.join("items")).unwrap();
        assert!(all_item_paths(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
