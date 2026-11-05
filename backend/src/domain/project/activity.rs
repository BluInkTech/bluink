// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::item::is_canonical_date;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityEvent {
    pub(crate) id: String,
    pub(crate) date: String,
    pub(crate) occurred_at: String,
    pub(crate) item_id: String,
    /// A task event contributes to its note while durability belongs to the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_item_id: Option<String>,
    pub(crate) revision: String,
    pub(crate) kind: String,
    pub(crate) words_added: i64,
    pub(crate) words_deleted: i64,
    pub(crate) net_word_change: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ItemActivity {
    pub(crate) words_added: i64,
    pub(crate) words_deleted: i64,
    pub(crate) net_word_change: i64,
    pub(crate) editing_events: u64,
    pub(crate) tasks_completed: u64,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityDay {
    pub(crate) date: String,
    #[serde(flatten)]
    pub(crate) activity: ItemActivity,
    pub(crate) notes_touched: u64,
}

pub(crate) fn events_for_item_write(
    existing: Option<&Value>,
    next: &Value,
    date: &str,
) -> Result<Vec<ActivityEvent>, String> {
    if !is_canonical_date(date) {
        return Err("Activity date must be a canonical date.".to_owned());
    }
    if !matches!(next["meta"]["type"].as_str(), Some("page" | "task")) {
        return Ok(Vec::new());
    }
    let id = next["id"].as_str().ok_or("Item has no ID")?;
    let revision = next["revision"].as_str().ok_or("Item has no revision")?;
    let occurred_at = next["modifiedAt"]
        .as_str()
        .ok_or("Item has no modified time")?;
    let mut events = Vec::new();
    if next["meta"]["type"] == "page"
        && existing.map(|item| &item["analytics"]["current"]["contentHash"])
            != Some(&next["analytics"]["current"]["contentHash"])
    {
        let previous_words = existing
            .and_then(|item| item["analytics"]["current"]["wordCount"].as_i64())
            .unwrap_or(0);
        let next_words = next["analytics"]["current"]["wordCount"]
            .as_i64()
            .ok_or("Item has no word count")?;
        let word_delta = next_words - previous_words;
        events.push(ActivityEvent {
            source_item_id: None,
            id: format!("{id}:{revision}"),
            date: date.to_owned(),
            occurred_at: occurred_at.to_owned(),
            item_id: id.to_owned(),
            revision: revision.to_owned(),
            kind: "content".to_owned(),
            words_added: word_delta.max(0),
            words_deleted: (-word_delta).max(0),
            net_word_change: word_delta,
        });
    }
    if next["meta"]["type"] == "task"
        && next["content"].get("completedAt").is_some()
        && existing.is_none_or(|previous| previous["content"].get("completedAt").is_none())
    {
        events.push(ActivityEvent {
            id: format!("{id}:{revision}:completed"),
            date: date.to_owned(),
            occurred_at: occurred_at.to_owned(),
            item_id: next["content"]["noteId"]
                .as_str()
                .ok_or("Task has no owning note")?
                .to_owned(),
            source_item_id: Some(id.to_owned()),
            revision: revision.to_owned(),
            kind: "task_completed".to_owned(),
            words_added: 0,
            words_deleted: 0,
            net_word_change: 0,
        });
    }
    Ok(events)
}
