// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PassageRecord {
    pub(crate) item_id: String,
    pub(crate) block_id: String,
    pub(crate) revision: String,
    pub(crate) note_title: String,
    pub(crate) labels: Vec<String>,
    pub(crate) text: String,
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::engine::project::types::OpaqueJson>")
    )]
    pub(crate) fragment: JsonValue,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TaskRecord {
    pub(crate) note_id: String,
    pub(crate) task_id: String,
    pub(crate) revision: String,
    pub(crate) note_title: String,
    pub(crate) text: String,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) due_date: Option<String>,
    pub(crate) modified_at: String,
}

pub(crate) fn empty_object() -> JsonValue {
    json!({})
}

#[derive(Debug, Clone)]
pub(crate) struct SortSpec {
    pub(crate) field: String,
    pub(crate) descending: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchHit {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) created_at: String,
    pub(crate) modified_at: String,
    pub(crate) score: f32,
    pub(crate) preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preview_image: Option<String>,
    pub(crate) metadata: JsonValue,
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TargetCount {
    pub(crate) tid: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResults {
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) count: usize,
    pub(crate) pinned_count: Option<usize>,
}

#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CalendarBucket {
    pub(crate) date: String,
    pub(crate) count: u64,
}

#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CalendarResults {
    pub(crate) buckets: Vec<CalendarBucket>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ResultPage<T> {
    pub(crate) items: Vec<T>,
    pub(crate) count: usize,
}
