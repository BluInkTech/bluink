// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use crate::engine::document::projection::{bounded_preview, project_document};
use crate::engine::project::envelope::ItemEnvelope;
use crate::engine::project::item::{ItemReadError, read_item};
use crate::engine::project::types::{ItemState, ItemType};

use serde_json::Value as JsonValue;

#[cfg(test)]
use crate::engine::project::identity::ItemId;
use crate::model::{PassageRecord, TaskRecord};
use crate::passages;

/// Internal projection assembled from a validated canonical item.
#[derive(Debug)]
pub(crate) struct SearchDocument {
    pub(crate) id: String,
    pub(crate) index_stamp: String,
    pub(crate) revision: String,
    pub(crate) created_at: String,
    pub(crate) modified_at: String,
    pub(crate) title: String,
    pub(crate) title_sort: String,
    pub(crate) preview: String,
    pub(crate) preview_image: Option<String>,
    pub(crate) body: String,
    pub(crate) item_type: String,
    pub(crate) state: String,
    pub(crate) tags: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) reference_targets: Vec<String>,
    pub(crate) snippet_citation_targets: Vec<String>,
    pub(crate) snippet_embed_targets: Vec<String>,
    pub(crate) passages: Vec<PassageRecord>,
    pub(crate) task: Option<TaskRecord>,
    pub(crate) snippet: Option<JsonValue>,
    pub(crate) pins: Vec<String>,
    pub(crate) metadata: JsonValue,
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

pub(crate) fn project_task(id: &str, envelope: &ItemEnvelope<'_>) -> Option<TaskRecord> {
    if envelope.kind != ItemType::Task {
        return None;
    }
    let content = envelope.content;
    Some(TaskRecord {
        note_id: string_at(content, "noteId"),
        task_id: id.to_owned(),
        revision: envelope.revision.to_owned(),
        note_title: String::new(),
        text: string_at(content, "text"),
        created_at: envelope.created_at.to_owned(),
        completed_at: content
            .get("completedAt")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        due_date: envelope
            .metadata
            .get("due_date")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        modified_at: envelope.modified_at.to_owned(),
    })
}

pub(crate) fn read_document(
    path: &Path,
    index_stamp: String,
) -> Result<SearchDocument, ItemReadError> {
    let invalid = |message| ItemReadError::InvalidData {
        path: path.to_owned(),
        message,
    };
    let id = item_id_from_path(path).map_err(invalid)?;
    let value = read_item(path, &id)?;
    let envelope = ItemEnvelope::read(&value, &id).map_err(invalid)?;
    let meta = JsonValue::Object(envelope.metadata.clone());
    // Canonical parsing already validates content. Projection only reads it;
    // cloning the entire document doubles the largest per-item allocation.
    let content = envelope.content;
    let mut projection = project_document(content);
    if envelope.kind == ItemType::Task {
        projection.text = string_at(content, "text");
    }
    let passages = if envelope.kind == ItemType::Page && envelope.state == ItemState::Active {
        passages::project(&id, envelope.revision, &string_at(&meta, "title"), content)
    } else {
        Vec::new()
    };
    let task = project_task(&id, &envelope);
    let snippet = if envelope.kind == ItemType::Snippet {
        Some(crate::engine::project::snippets::record(&value).map_err(invalid)?)
    } else {
        None
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
        revision: envelope.revision.to_owned(),
        created_at: envelope.created_at.to_owned(),
        modified_at: envelope.modified_at.to_owned(),
        title: string_at(&meta, "title"),
        title_sort: string_at(&meta, "title").to_lowercase(),
        preview: bounded_preview(&projection.text),
        preview_image: projection.preview_image,
        body: projection.text,
        item_type: envelope.kind.as_str().to_owned(),
        state: envelope.state.as_str().to_owned(),
        tags,
        labels: projection.labels,
        reference_targets,
        snippet_citation_targets: projection.snippet_citations,
        snippet_embed_targets: projection.snippet_embeds,
        passages,
        task,
        snippet,
        pins,
        metadata: meta,
    })
}

pub(crate) fn item_id_from_path(path: &Path) -> std::result::Result<String, String> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .filter(|value| value.len() == 36)
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no item UUID parent", path.display()))
}

const STAMP_PREFIX: &str = "v15:";
pub(crate) fn index_stamp(item_stamp: &str) -> String {
    format!("{STAMP_PREFIX}{item_stamp}")
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
            return Err(format!("{} is not a regular item file", path.display()));
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
        let (size, nanos) = value.strip_prefix(STAMP_PREFIX)?.split_once(':')?;
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
    fn file_stamp_distinguishes_missing_files_from_invalid_file_types() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("item.json");
        assert_eq!(FileStamp::read(&path).unwrap(), None);
        fs::create_dir(&path).unwrap();
        let error = FileStamp::read(&path).unwrap_err();
        assert!(error.contains("not a regular item file"));
        assert!(error.contains(&path.display().to_string()));
        root.close().unwrap();
    }

    #[test]
    fn compact_metadata_preserves_nanosecond_precision() {
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
    fn malformed_canonical_projection_retains_its_read_cause() {
        use std::error::Error;
        let root = tempfile::tempdir().unwrap();
        let id = ItemId::try_from("01a0e2a0-0000-7000-8000-000000000001").unwrap();
        std::fs::create_dir_all(id.directory(root.path())).unwrap();
        let path = id.path(root.path());
        std::fs::write(&path, b"{broken").unwrap();
        let error = read_document(&path, "stamp".into()).unwrap_err();
        assert!(matches!(&error, ItemReadError::Json { path: failed, .. } if failed == &path));
        assert!(error.source().unwrap().is::<serde_json::Error>());
        root.close().unwrap();
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
}
