//! Shared item read options, views and canonical-to-view conversion.
use crate::domain::project::types::Metadata;
use crate::store::Result;
use crate::structured_query::document::QueryDocument;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadOptions {
    pub(crate) content: Option<bool>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadItemRequest {
    pub(crate) id: String,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadItemsRequest {
    pub(crate) ids: Option<Vec<String>>,
    pub(crate) query: Option<QueryDocument>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QueryItemsRequest {
    pub(crate) query: QueryDocument,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ReadOptions>", optional))]
    pub(crate) options: ReadOptions,
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
    pub(crate) pin_scope_id: Option<String>,
    pub(crate) prioritize_pins: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ResolveWorkspaceNotesRequest {
    pub(crate) workspace_id: String,
    pub(crate) ids: Vec<String>,
    pub(crate) limit: Option<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
pub(crate) struct PublicItem {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) created_at: String,
    pub(crate) modified_at: String,
    pub(crate) meta: Metadata,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
    )]
    pub(crate) content: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) pins: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) preview_image: Option<String>,
    /// A result card's active tasks; absent for full reads and pages without tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) tasks: Option<TaskCounts>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TaskCounts {
    pub(crate) open: u32,
    pub(crate) total: u32,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryItemsResult {
    pub(crate) items: Vec<PublicItem>,
    pub(crate) count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) pinned_count: Option<usize>,
}

pub(crate) fn canonical_item(mut value: Value, with_content: bool) -> Result<PublicItem> {
    let fields = value.as_object_mut().ok_or_else(|| {
        crate::store::Error::Internal(anyhow::anyhow!("Stored item is not an object"))
    })?;
    let revision = fields.remove("revision").ok_or_else(|| {
        crate::store::Error::Internal(anyhow::anyhow!("Stored item has no revision"))
    })?;
    fields.insert("version".to_owned(), revision);
    if !with_content {
        fields.insert("content".to_owned(), Value::Object(serde_json::Map::new()));
    }
    serde_json::from_value(value).map_err(|error| crate::store::Error::Internal(error.into()))
}
