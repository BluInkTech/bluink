// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::document::projection::{bounded_preview, project_document};
use crate::domain::project::envelope::ItemEnvelope;
use crate::domain::project::types::ItemType;

use serde_json::Value as JsonValue;

use crate::records::{PassageRecord, TaskRecord};
use crate::store::projections::passages;

/// Internal projection assembled from a validated canonical item.
#[derive(Debug)]
pub(crate) struct SearchDocument {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) created_at: String,
    pub(crate) title: String,
    pub(crate) preview: String,
    pub(crate) preview_image: Option<String>,
    pub(crate) body: String,
    pub(crate) item_type: String,
    pub(crate) state: String,
    pub(crate) labels: Vec<String>,
    pub(crate) reference_targets: Vec<String>,
    /// Internal page links in first-occurrence order, for export discovery.
    pub(crate) page_links: Vec<String>,
    /// A page's export facts as JSON; absent for other item types.
    pub(crate) export_facts: Option<String>,
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
        modified_at: envelope.modified_at.to_owned(),
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
        // The projection keeps owner, completion and due day; queries read
        // properties from the task's metadata.
        properties: serde_json::Map::new(),
    })
}

/// Projection accepts validated canonical values independently of their storage.
pub(crate) fn project_item(value: &JsonValue, id: &str) -> Result<SearchDocument, String> {
    let envelope = ItemEnvelope::read(value, id)?;
    let meta = JsonValue::Object(envelope.metadata.clone());
    // Canonical parsing already validates content. Projection only reads it;
    // cloning the entire document doubles the largest per-item allocation.
    let content = envelope.content;
    let mut projection = project_document(crate::domain::document::document_root(content));
    if envelope.kind == ItemType::Task {
        projection.text = string_at(content, "text");
    }
    // Archived pages keep their passages; readers filter by the item's state,
    // so activation only changes the header.
    let passages = if envelope.kind == ItemType::Page {
        passages::project(id, envelope.revision, &string_at(&meta, "title"), content)
    } else {
        Vec::new()
    };
    let task = project_task(id, &envelope);
    let snippet = if envelope.kind == ItemType::Snippet {
        Some(crate::domain::project::snippets::record(value)?)
    } else {
        None
    };
    // Pages and templates record what export needs once, when they are saved.
    let export_facts = matches!(envelope.kind, ItemType::Page | ItemType::Template)
        .then(|| {
            serde_json::to_string(&crate::domain::publication::preflight::document_facts(
                crate::domain::document::document_root(content),
            ))
        })
        .transpose()
        .map_err(|error| error.to_string())?;
    let page_links = projection.page_links;
    let mut reference_targets = page_links.clone();
    reference_targets.sort();
    reference_targets.dedup();
    let mut pins = string_array(value.get("pins"));
    pins.sort();
    pins.dedup();
    Ok(SearchDocument {
        id: id.to_owned(),
        revision: envelope.revision.to_owned(),
        created_at: envelope.created_at.to_owned(),
        title: string_at(&meta, "title"),
        preview: bounded_preview(&projection.text),
        preview_image: projection.preview_image,
        body: projection.text,
        item_type: envelope.kind.as_str().to_owned(),
        state: envelope.state.as_str().to_owned(),
        labels: projection.labels,
        reference_targets,
        page_links,
        export_facts,
        snippet_citation_targets: projection.snippet_citations,
        snippet_embed_targets: projection.snippet_embeds,
        passages,
        task,
        snippet,
        pins,
        metadata: meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn native_index_reads_portable_fixture_through_core_projection() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-fixtures/pdf-project/items/be/01a0e2a0-0000-7000-8000-000000000001.json");
        let value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let item = project_item(&value, "01a0e2a0-0000-7000-8000-000000000001").unwrap();
        assert_eq!(item.id, "01a0e2a0-0000-7000-8000-000000000001");
        assert_ne!(item.body, "");
        assert_eq!(item.page_links, ["01a0e2a0-0000-7000-8000-000000000002"]);
        let facts: crate::domain::publication::preflight::PageFacts =
            serde_json::from_str(item.export_facts.as_deref().unwrap()).unwrap();
        assert_eq!(facts.headings.len(), 1);
        assert_eq!(facts.anchor_links.len(), 1);
        assert!(
            facts
                .snippets
                .iter()
                .all(|link| link.target == "source_one")
        );
        assert_eq!(facts.issues, Vec::new());
    }
}
