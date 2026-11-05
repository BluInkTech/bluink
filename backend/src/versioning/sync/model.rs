// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct SyncStatus {
    pub(crate) origin: Option<String>,
    pub(crate) state: String,
    pub(crate) phase: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) head: Option<String>,
    pub(crate) last_synced_head: Option<String>,
    pub(crate) decisions: usize,
    pub(crate) recovery_refs: Vec<RecoveryRef>,
    pub(crate) remote_head: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct RecoveryRef {
    pub(crate) name: String,
    pub(crate) commit: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ConfigureSync {
    pub(crate) origin: Option<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub(crate) create: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct CloneProject {
    pub(crate) origin: String,
    pub(crate) destination: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ClonedProject {
    pub(crate) path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HistoryDownload {
    pub(crate) reachable_bytes: u64,
    pub(crate) missing_bytes: u64,
}
