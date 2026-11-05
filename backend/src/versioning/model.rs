// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryStatus {
    pub(crate) head: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) last_checkpoint_at: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) local_only: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct NamedVersion {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) item_id: Option<String>,
    pub(crate) commit_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryEntry {
    pub(crate) id: String,
    pub(crate) message: String,
    pub(crate) created_at: String,
    pub(crate) named_versions: Vec<NamedVersion>,
    pub(crate) attachments_available: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryPage {
    pub(crate) entries: Vec<HistoryEntry>,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryRequest {
    #[cfg_attr(test, ts(optional))]
    pub(crate) item_id: Option<String>,
    #[cfg_attr(test, ts(optional))]
    pub(crate) before: Option<String>,
    #[cfg_attr(test, ts(optional))]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct NamedVersionRequest {
    pub(crate) request_id: String,
    pub(crate) title: String,
    #[cfg_attr(test, ts(optional))]
    pub(crate) item_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryPreviewRequest {
    pub(crate) commit_id: String,
    #[cfg_attr(test, ts(optional))]
    pub(crate) item_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryItemRequest {
    pub(crate) commit_id: String,
    pub(crate) item_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryClientActivityRequest {
    pub(crate) client_id: String,
    pub(crate) visible: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryClientActivity {
    pub(crate) accepted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryRestoreRequest {
    pub(crate) commit_id: String,
    #[cfg_attr(test, ts(optional))]
    pub(crate) item_id: Option<String>,
    pub(crate) expected_head: String,
    pub(crate) expected_generation: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum HistoryChange {
    Added,
    Changed,
    Removed,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryItemChange {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) change: HistoryChange,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryPreview {
    pub(crate) commit_id: String,
    pub(crate) item_id: Option<String>,
    pub(crate) expected_head: String,
    pub(crate) expected_generation: String,
    pub(crate) changed_items: Vec<HistoryItemChange>,
    pub(crate) changed_assets: usize,
    pub(crate) changed_settings: bool,
    pub(crate) changed_themes: usize,
    pub(crate) archived_tasks: Vec<String>,
}
