// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Revision-aware editor intent. Storage identities never cross this boundary.
use crate::domain::project::types::{Metadata, OpaqueJson};
use crate::records::items::PublicItem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct MergeDraftRequest {
    pub(crate) id: String,
    pub(crate) base_revision: String,
    pub(crate) submission_id: String,
    pub(crate) device: String,
    pub(crate) draft: Box<EditorDraft>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum EditorDraft {
    Replace {
        item: Box<DraftItem>,
    },
    DocumentPatch {
        meta: Box<Metadata>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        tags: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        pins: Option<Vec<String>>,
        /// Complete resulting content fields other than `document`.
        content: BTreeMap<String, OpaqueJson>,
        document: DocumentPatch,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DraftItem {
    pub(crate) meta: Metadata,
    pub(crate) content: BTreeMap<String, OpaqueJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) pins: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DocumentPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) attrs: Option<BTreeMap<String, OpaqueJson>>,
    pub(crate) order: Vec<String>,
    /// Only new or changed complete top-level subtrees, keyed by editor block ID.
    pub(crate) blocks: BTreeMap<String, OpaqueJson>,
}

impl EditorDraft {
    /// Expand against the exact retained base, never the latest version.
    pub(crate) fn expand(&self, base: &Value) -> Result<Value, String> {
        match self {
            Self::Replace { item } => serde_json::to_value(item).map_err(|error| error.to_string()),
            Self::DocumentPatch {
                meta,
                tags,
                pins,
                content,
                document,
            } => {
                if content.contains_key("document") {
                    return Err("Document patch content cannot contain document".into());
                }
                let root = &base["content"]["document"];
                if root["type"] != "doc" {
                    return Err("Draft base has no editable document".into());
                }
                let source = root["content"]
                    .as_array()
                    .ok_or("Draft base has no blocks")?;
                let mut originals = BTreeMap::new();
                for block in source {
                    let id = block["attrs"]["id"]
                        .as_str()
                        .ok_or("Base block has no ID")?;
                    if originals.insert(id, block).is_some() {
                        return Err("Duplicate base block ID".into());
                    }
                }
                let mut seen = std::collections::BTreeSet::new();
                let mut blocks = Vec::with_capacity(document.order.len());
                for id in &document.order {
                    if id.is_empty() || !seen.insert(id.as_str()) {
                        return Err("Duplicate or empty document block ID".into());
                    }
                    let block = document
                        .blocks
                        .get(id)
                        .map(|value| &value.0)
                        .or_else(|| originals.get(id.as_str()).copied())
                        .ok_or("Document patch references an unknown block")?;
                    if block["attrs"]["id"].as_str() != Some(id.as_str()) {
                        return Err("Document patch block ID does not match its key".into());
                    }
                    blocks.push(block.clone());
                }
                if document.blocks.keys().any(|id| !seen.contains(id.as_str())) {
                    return Err("Document patch contains an unused block".into());
                }
                let mut next_root = json!({"type":"doc", "content":blocks});
                if let Some(attrs) = &document.attrs {
                    next_root["attrs"] =
                        serde_json::to_value(attrs).map_err(|error| error.to_string())?;
                }
                let mut next_content =
                    serde_json::to_value(content).map_err(|error| error.to_string())?;
                next_content["document"] = next_root;
                let mut item = json!({"meta":meta, "content":next_content});
                if let Some(tags) = tags {
                    item["tags"] = json!(tags);
                }
                if let Some(pins) = pins {
                    item["pins"] = json!(pins);
                }
                Ok(item)
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum MergeDraftResult {
    Ack {
        version: String,
        modified_at: String,
        conflict_ids: Vec<String>,
        submission_id: String,
    },
    Reconciled {
        item: Box<PublicItem>,
        conflict_ids: Vec<String>,
        submission_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        json!({"content":{"document":{"type":"doc","attrs":{"old":true},"content":[
            {"type":"paragraph","attrs":{"id":"a"},"content":[{"type":"text","text":"A"}]},
            {"type":"paragraph","attrs":{"id":"b"},"content":[{"type":"text","text":"B"}]}
        ]}}})
    }

    fn patch(document: &Value) -> EditorDraft {
        serde_json::from_value(
            json!({"kind":"documentPatch","meta":{"type":"page"},"content":{},"document":document}),
        )
        .unwrap()
    }

    #[test]
    fn reorder_delete_insert_and_root_attributes_use_exact_base() {
        let draft = patch(
            &json!({"order":["b","new"],"attrs":{"language":"en"},"blocks":{
                "new":{"type":"paragraph","attrs":{"id":"new"},"content":[{"type":"text","text":"New"}]}
            }}),
        );
        let result = draft.expand(&base()).unwrap();
        let root = &result["content"]["document"];
        assert_eq!(root["attrs"], json!({"language":"en"}));
        assert_eq!(
            root["content"][0],
            base()["content"]["document"]["content"][1]
        );
        assert_eq!(root["content"].as_array().unwrap().len(), 2);
        assert_eq!(root["content"][1]["attrs"]["id"], "new");
    }

    #[test]
    fn malformed_block_references_and_duplicate_ids_are_rejected() {
        for document in [
            json!({"order":["a","a"],"blocks":{}}),
            json!({"order":["missing"],"blocks":{}}),
            json!({"order":["a"],"blocks":{"unused":{"attrs":{"id":"unused"}}}}),
            json!({"order":["a"],"blocks":{"a":{"attrs":{"id":"different"}}}}),
        ] {
            assert!(patch(&document).expand(&base()).is_err());
        }
    }

    #[test]
    fn omitted_root_attributes_are_removed_and_content_cannot_override_document() {
        let draft = patch(&json!({"order":["a"],"blocks":{}}));
        assert!(
            draft.expand(&base()).unwrap()["content"]["document"]
                .get("attrs")
                .is_none()
        );
        let invalid: EditorDraft = serde_json::from_value(json!({"kind":"documentPatch","meta":{},"content":{"document":{}},"document":{"order":[],"blocks":{}}})).unwrap();
        assert!(invalid.expand(&base()).is_err());
    }
}
