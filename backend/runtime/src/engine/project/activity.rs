// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::project::config::read_project_config;
use crate::engine::project::io::write_json_atomic;
use crate::engine::project::item::{is_canonical_date, is_canonical_instant, is_item_id};

type Result<T> = std::result::Result<T, String>;

fn native_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

const ACTIVITY_FORMAT: &str = "bluink.activity";
const ACTIVITY_FORMAT_VERSION: u8 = 3;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityEvent {
    pub(crate) id: String,
    pub(crate) date: String,
    pub(crate) occurred_at: String,
    pub(crate) item_id: String,
    pub(crate) revision: String,
    pub(crate) kind: String,
    pub(crate) words_added: i64,
    pub(crate) words_deleted: i64,
    pub(crate) net_word_change: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct JournalEntry {
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<ActivityEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<String>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailyActivity {
    format: String,
    format_version: u8,
    pub(crate) date: String,
    pub(crate) items: BTreeMap<String, ItemActivity>,
    applied_event_ids: Vec<String>,
}

impl DailyActivity {
    fn new(date: String) -> Self {
        Self {
            format: ACTIVITY_FORMAT.to_owned(),
            format_version: ACTIVITY_FORMAT_VERSION,
            date,
            items: BTreeMap::new(),
            applied_event_ids: Vec::new(),
        }
    }
}

pub(crate) fn events_for_item_write(
    existing: Option<&Value>,
    next: &Value,
    date: &str,
) -> Result<Vec<ActivityEvent>> {
    if !is_canonical_date(date) {
        return Err("Activity date must be a canonical date.".to_owned());
    }
    if next["meta"]["type"] != "page" {
        return Ok(Vec::new());
    }
    let id = next["id"].as_str().ok_or("Item has no ID")?;
    let revision = next["revision"].as_str().ok_or("Item has no revision")?;
    let occurred_at = next["modifiedAt"]
        .as_str()
        .ok_or("Item has no modified time")?;
    let mut events = Vec::new();
    if existing.map(|item| &item["analytics"]["current"]["contentHash"])
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
    let previous_completed: HashSet<&str> = existing
        .and_then(|item| item["tasks"].as_array())
        .into_iter()
        .flatten()
        .filter(|task| task.get("completedAt").is_some())
        .filter_map(|task| task["id"].as_str())
        .collect();
    for task in next["tasks"].as_array().into_iter().flatten() {
        let Some(task_id) = task["id"].as_str() else {
            continue;
        };
        if task.get("completedAt").is_none() || previous_completed.contains(task_id) {
            continue;
        }
        events.push(ActivityEvent {
            id: format!("{id}:{revision}:{task_id}:completed"),
            date: date.to_owned(),
            occurred_at: occurred_at.to_owned(),
            item_id: id.to_owned(),
            revision: revision.to_owned(),
            kind: "task_completed".to_owned(),
            words_added: 0,
            words_deleted: 0,
            net_word_change: 0,
        });
    }
    Ok(events)
}

pub(crate) fn prepare_item_events(root: &Path, events: &[ActivityEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    with_activity_lock(root, || {
        for event in events {
            validate_event(event)?;
            append_journal(
                root,
                &event.date,
                &JournalEntry {
                    state: "pending".to_owned(),
                    event: Some(event.clone()),
                    event_id: None,
                },
            )?;
        }
        Ok(())
    })
}

pub(crate) fn finish_item_events(root: &Path, events: &[ActivityEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    with_activity_lock(root, || {
        for event in events {
            append_journal(
                root,
                &event.date,
                &JournalEntry {
                    state: "committed".to_owned(),
                    event: None,
                    event_id: Some(event.id.clone()),
                },
            )?;
        }
        reconcile_activity_root(root)?;
        Ok(())
    })
}

pub(crate) fn reconcile_activity(project_root: String) -> Result<String> {
    let root = PathBuf::from(project_root);
    let result = with_activity_lock(&root, || reconcile_activity_root(&root))?;
    serde_json::to_string(&result).map_err(native_error)
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

pub(crate) fn activity_days(
    root: &Path,
    start_date: &str,
    end_date: &str,
    selected: Option<&HashSet<String>>,
) -> Result<Vec<ActivityDay>> {
    let mut days = Vec::new();
    let entries = match fs::read_dir(root.join("analytics")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(days),
        Err(error) => return Err(native_error(error)),
    };
    for entry in entries {
        let path = entry.map_err(native_error)?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(date) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !is_date(date) || date < start_date || date > end_date {
            continue;
        }
        if let Some(day) = read_daily(&path, date)? {
            days.push(day_summary(&day, selected));
        }
    }
    days.sort_by(|left, right| left.date.cmp(&right.date));
    Ok(days)
}

fn day_summary(day: &DailyActivity, selected: Option<&HashSet<String>>) -> ActivityDay {
    let values = day
        .items
        .iter()
        .filter(|(id, _)| selected.is_none_or(|ids| ids.contains(*id)))
        .map(|(_, value)| value);
    let mut words_added = 0i64;
    let mut words_deleted = 0i64;
    let mut net_word_change = 0i64;
    let mut editing_events = 0u64;
    let mut tasks_completed = 0u64;
    let mut notes_touched = 0u64;
    for value in values {
        words_added += value.words_added;
        words_deleted += value.words_deleted;
        net_word_change += value.net_word_change;
        editing_events += value.editing_events;
        tasks_completed += value.tasks_completed;
        if value.editing_events > 0 {
            notes_touched += 1;
        }
    }
    ActivityDay {
        date: day.date.clone(),
        activity: ItemActivity {
            words_added,
            words_deleted,
            net_word_change,
            editing_events,
            tasks_completed,
        },
        notes_touched,
    }
}

fn reconcile_activity_root(root: &Path) -> Result<Vec<ActivityDay>> {
    let mut grouped = BTreeMap::<String, Vec<ActivityEvent>>::new();
    let mut committed = HashSet::<String>::new();
    for path in journal_paths(root)? {
        for entry in read_journal(&path)? {
            if entry.state == "committed" {
                if let Some(id) = entry.event_id {
                    committed.insert(id);
                }
            } else if entry.state == "pending"
                && let Some(event) = entry.event
            {
                grouped.entry(event.date.clone()).or_default().push(event);
            }
        }
    }
    let mut summaries = Vec::new();
    for (date, events) in grouped {
        let target = root.join("analytics").join(format!("{date}.json"));
        let mut day =
            read_daily(&target, &date)?.unwrap_or_else(|| DailyActivity::new(date.clone()));
        let mut applied = day
            .applied_event_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for event in events {
            if applied.contains(&event.id) {
                continue;
            }
            if !committed.contains(&event.id) && !revision_exists(root, &event) {
                continue;
            }
            let item = day.items.entry(event.item_id.clone()).or_default();
            match event.kind.as_str() {
                "content" => {
                    item.words_added += event.words_added;
                    item.words_deleted += event.words_deleted;
                    item.net_word_change += event.net_word_change;
                    item.editing_events += 1;
                }
                "task_completed" => item.tasks_completed += 1,
                _ => continue,
            }
            applied.insert(event.id);
        }
        let next_ids = applied.into_iter().collect::<Vec<_>>();
        let changed = day.applied_event_ids != next_ids;
        day.applied_event_ids = next_ids;
        if changed {
            write_json_atomically(&target, &day)?;
        }
        summaries.push(day_summary(&day, None));
    }
    compact_old_journals(root)?;
    Ok(summaries)
}

fn validate_event(event: &ActivityEvent) -> Result<()> {
    if event.id.is_empty()
        || !is_item_id(&event.item_id)
        || !is_item_id(&event.revision)
        || !is_canonical_date(&event.date)
        || !is_canonical_instant(&event.occurred_at)
    {
        return Err(native_error("Activity event is incomplete"));
    }
    if !matches!(event.kind.as_str(), "content" | "task_completed") {
        return Err(native_error("Activity event kind is unsupported"));
    }
    Ok(())
}

fn event_directory(root: &Path) -> PathBuf {
    root.join("analytics").join("events")
}

fn with_activity_lock<T>(root: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let directory = root.join(".cache");
    fs::create_dir_all(&directory).map_err(native_error)?;
    let path = directory.join("activity.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(native_error)?;
    lock.lock_exclusive().map_err(native_error)?;
    operation()
}

fn journal_path(root: &Path, date: &str) -> PathBuf {
    event_directory(root).join(format!("{date}.jsonl"))
}

fn append_journal(root: &Path, date: &str, entry: &JournalEntry) -> Result<()> {
    let directory = event_directory(root);
    fs::create_dir_all(&directory).map_err(native_error)?;
    let path = journal_path(root, date);
    match fs::read(&path) {
        Ok(source) if !source.is_empty() && !source.ends_with(b"\n") => {
            let complete_length = source
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(native_error)?
                .set_len(complete_length as u64)
                .map_err(native_error)?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(native_error(error)),
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(native_error)?;
    let line = serde_json::to_string(entry).map_err(native_error)?;
    file.write_all(line.as_bytes()).map_err(native_error)?;
    file.write_all(b"\n").map_err(native_error)?;
    file.sync_data().map_err(native_error)
}

fn journal_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let directory = event_directory(root);
    let entries = match fs::read_dir(directory) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(native_error(error)),
    };
    entries
        .map(|entry| entry.map(|entry| entry.path()).map_err(native_error))
        .filter(|result| {
            result.as_ref().map_or(true, |path| {
                path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            })
        })
        .collect()
}

fn read_journal(path: &Path) -> Result<Vec<JournalEntry>> {
    let source = fs::read_to_string(path).map_err(native_error)?;
    let complete = source
        .rsplit_once('\n')
        .map_or("", |(complete, _)| complete);
    complete
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str::<JournalEntry>(line)
                .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))
        })
        .collect()
}

fn read_daily(path: &Path, date: &str) -> Result<Option<DailyActivity>> {
    let source = match fs::read(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let value: DailyActivity =
        serde_json::from_slice(&source).map_err(|error| format!("{}: {error}", path.display()))?;
    if value.format != ACTIVITY_FORMAT
        || value.format_version != ACTIVITY_FORMAT_VERSION
        || value.date != date
    {
        return Err(format!(
            "{} is not a supported Bluink daily activity file.",
            path.display()
        ));
    }
    Ok(Some(value))
}

pub(crate) fn read_daily_activity(root: &Path, date: &str) -> Result<Option<DailyActivity>> {
    if !is_canonical_date(date) {
        return Err("Activity date must be a canonical date.".to_owned());
    }
    read_daily(&root.join("analytics").join(format!("{date}.json")), date)
}

fn write_json_atomically(path: &Path, value: &DailyActivity) -> Result<()> {
    let value = serde_json::to_value(value).map_err(native_error)?;
    write_json_atomic(path, &value)
}

fn revision_exists(root: &Path, event: &ActivityEvent) -> bool {
    let item = root
        .join("items")
        .join(&event.item_id[0..2])
        .join(&event.item_id)
        .join("item.json");
    if revision_in_file(&item, &event.revision) {
        return true;
    }
    let history = item.parent().map(|parent| parent.join("history"));
    let Some(history) = history else {
        return false;
    };
    let Ok(entries) = fs::read_dir(history) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| revision_in_file(&entry.path(), &event.revision))
}

fn revision_in_file(path: &Path, revision: &str) -> bool {
    fs::read(path)
        .ok()
        .and_then(|source| serde_json::from_slice::<Value>(&source).ok())
        .and_then(|value| {
            value
                .get("revision")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some(revision)
}

fn compact_old_journals(root: &Path) -> Result<()> {
    let config = read_project_config(root)?.ok_or("Project configuration is missing.")?;
    let retention = config["analytics"]["eventRetentionDays"]
        .as_u64()
        .ok_or("Invalid event retention setting")?;
    let now_days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs() / 86_400);
    for path in journal_paths(root)? {
        let Some(date) = path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.get(0..10))
        else {
            continue;
        };
        if !is_date(date) {
            continue;
        }
        let day_days = days_from_date(date).unwrap_or(now_days);
        if now_days.saturating_sub(day_days) > retention {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn is_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value
            .chars()
            .enumerate()
            .all(|(index, c)| index == 4 || index == 7 || c.is_ascii_digit())
}

fn days_from_date(value: &str) -> Option<u64> {
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let yoe = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468).max(0).cast_unsigned())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use serde_json::Value;

    use super::{ActivityEvent, prepare_item_events, read_daily_activity, reconcile_activity};
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_project_item;

    #[test]
    fn applies_a_prepared_event_once_after_a_durable_item_write() {
        let root =
            std::env::temp_dir().join(format!("bluink-core-activity-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        std::fs::create_dir_all(root.join("properties")).unwrap();
        let item: Value = serde_json::from_slice(
            &std::fs::read(fixture.join("items/01/01a0e2a0-0000-7000-8000-000000000001/item.json"))
                .unwrap(),
        )
        .unwrap();
        let id = item["id"].as_str().unwrap().to_owned();
        let revision = item["revision"].as_str().unwrap().to_owned();
        let event_id = format!("{id}:{revision}:recovered");
        let event = ActivityEvent {
            id: event_id.clone(),
            date: "2026-09-28".to_owned(),
            occurred_at: "2026-09-28T10:00:00.000Z".to_owned(),
            item_id: id,
            revision,
            kind: "task_completed".to_owned(),
            words_added: 0,
            words_deleted: 0,
            net_word_change: 0,
        };
        prepare_item_events(&root, std::slice::from_ref(&event)).unwrap();
        assert!(std::fs::read(root.join("analytics/2026-09-28.json")).is_err());
        write_project_item(&root, item, None, "2026-09-28").unwrap();
        let first: Value =
            serde_json::from_str(&reconcile_activity(root.to_string_lossy().into_owned()).unwrap())
                .unwrap();
        assert_eq!(first[0]["tasksCompleted"], 1);
        let second: Value =
            serde_json::from_str(&reconcile_activity(root.to_string_lossy().into_owned()).unwrap())
                .unwrap();
        assert_eq!(second[0]["tasksCompleted"], 1);
        let day: Value =
            serde_json::from_slice(&std::fs::read(root.join("analytics/2026-09-28.json")).unwrap())
                .unwrap();
        assert_eq!(
            read_daily_activity(&root, "2026-09-28")
                .unwrap()
                .map(|day| serde_json::to_value(day).unwrap()),
            Some(day.clone())
        );
        assert!(read_daily_activity(&root, "2026-09-27").unwrap().is_none());
        assert!(read_daily_activity(&root, "2026-02-30").is_err());
        assert_eq!(
            day["appliedEventIds"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|id| id.as_str() == Some(&event_id))
                .count(),
            1
        );
        let mut invalid = event.clone();
        invalid.item_id = "🔒".to_owned();
        invalid.date = "2026-02-30".to_owned();
        assert!(prepare_item_events(&root, &[invalid]).is_err());
        let journal = root.join("analytics/events/2026-09-28.jsonl");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap()
            .write_all(b"{\"partial")
            .unwrap();
        prepare_item_events(&root, &[event]).unwrap();
        let journal_source = std::fs::read_to_string(journal).unwrap();
        assert!(!journal_source.contains("partial"));
        std::fs::write(root.join("analytics/2026-09-28.json"), b"{broken").unwrap();
        assert!(
            reconcile_activity(root.to_string_lossy().into_owned())
                .unwrap_err()
                .contains("2026-09-28.json")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
