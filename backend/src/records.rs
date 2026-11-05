// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

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
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
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
    /// The task's own property values, such as a context or a start date.
    #[cfg_attr(
        test,
        ts(as = "std::collections::BTreeMap<String, crate::domain::project::types::OpaqueJson>")
    )]
    pub(crate) properties: serde_json::Map<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TargetCount {
    pub(crate) tid: String,
    pub(crate) count: usize,
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

// Snippet records shared by the RPC and canonical read adapters.
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
    paragraph: crate::domain::project::types::ReferenceParagraph,
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
    pub(crate) record: SnippetRecord,
    pub(crate) usage_count: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct SnippetResults {
    pub(crate) items: Vec<SnippetHit>,
    pub(crate) count: usize,
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

pub(crate) mod items;

pub(crate) mod drafts;
