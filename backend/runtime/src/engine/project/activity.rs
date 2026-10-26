// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::engine::project::io::{LockMode, lock_file};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::project::config::read_project_config;
use crate::engine::project::identity::{ItemId, is_item_id};
use crate::engine::project::io::write_json_atomic;
use crate::engine::project::item::{is_canonical_date, is_canonical_instant};

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
    /// A task event contributes to its note while durability belongs to the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_item_id: Option<String>,
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
        let mut grouped = BTreeMap::<String, Vec<ActivityEvent>>::new();
        for event in events {
            grouped
                .entry(event.date.clone())
                .or_default()
                .push(event.clone());
        }
        // These commits are already durable. Recovery scans belong to startup
        // and external reconciliation, not every normal note save.
        apply_activity_events(root, grouped, |_| true)?;
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
    for date in daily_dates(root, start_date, end_date)? {
        if let Some(day) = read_daily_activity(root, &date)? {
            days.push(day_summary(&day, selected));
        }
    }
    Ok(days)
}

/// Enumerate existing daily records once, including sparse or very wide ranges.
pub(crate) fn daily_dates(root: &Path, start_date: &str, end_date: &str) -> Result<Vec<String>> {
    let mut dates = Vec::new();
    let entries = match fs::read_dir(root.join("analytics")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(dates),
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
        dates.push(date.to_owned());
    }
    dates.sort_unstable();
    Ok(dates)
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
    let summaries = reconcile_journals(root, journal_paths(root)?)?;
    compact_old_journals(root)?;
    Ok(summaries)
}

/// Native watcher events identify the journal date; unrelated days need no reads.
pub(crate) fn reconcile_activity_dates(root: &Path, dates: &HashSet<String>) -> Result<()> {
    with_activity_lock(root, || {
        let mut paths = Vec::new();
        for date in dates {
            if !is_canonical_date(date) {
                return Err("Activity date must be a canonical date.".to_owned());
            }
            let path = journal_path(root, date);
            if path.try_exists().map_err(native_error)? {
                paths.push(path);
            }
        }
        reconcile_journals(root, paths)?;
        Ok(())
    })
}

fn reconcile_journals(root: &Path, paths: Vec<PathBuf>) -> Result<Vec<ActivityDay>> {
    let mut grouped = BTreeMap::<String, Vec<ActivityEvent>>::new();
    let mut committed = HashSet::<String>::new();
    for path in paths {
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
    apply_activity_events(root, grouped, |event| {
        committed.contains(&event.id) || revision_exists(root, event)
    })
}

fn apply_activity_events(
    root: &Path,
    grouped: BTreeMap<String, Vec<ActivityEvent>>,
    confirmed: impl Fn(&ActivityEvent) -> bool,
) -> Result<Vec<ActivityDay>> {
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
            if !confirmed(&event) {
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
            write_json_atomic(&target, &day).map_err(|error| error.to_string())?;
        }
        summaries.push(day_summary(&day, None));
    }
    Ok(summaries)
}

fn validate_event(event: &ActivityEvent) -> Result<()> {
    if event.id.is_empty()
        || !is_item_id(&event.item_id)
        || event
            .source_item_id
            .as_deref()
            .is_some_and(|id| !is_item_id(id))
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
    let _lock = lock_file(&root.join(".cache/activity.lock"), LockMode::Exclusive)
        .map_err(|error| error.to_string())?;
    operation()
}

fn journal_path(root: &Path, date: &str) -> PathBuf {
    event_directory(root).join(format!("{date}.jsonl"))
}

fn append_journal(root: &Path, date: &str, entry: &JournalEntry) -> Result<()> {
    let directory = event_directory(root);
    fs::create_dir_all(&directory).map_err(native_error)?;
    let path = journal_path(root, date);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(native_error)?;
    truncate_partial_line(&mut file)?;
    // The activity lock serializes appenders. Read/write access also permits
    // SetEndOfFile for torn-tail recovery on Windows (append-only does not).
    file.seek(SeekFrom::End(0)).map_err(native_error)?;
    let line = serde_json::to_string(entry).map_err(native_error)?;
    file.write_all(line.as_bytes()).map_err(native_error)?;
    file.write_all(b"\n").map_err(native_error)?;
    file.sync_data().map_err(native_error)
}

/// A complete journal needs only a one-byte tail check. After a torn write,
/// walk backwards in bounded chunks to retain the last complete record.
fn truncate_partial_line(file: &mut fs::File) -> Result<()> {
    let length = file.metadata().map_err(native_error)?.len();
    if length == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(length - 1))
        .map_err(native_error)?;
    let mut last = [0];
    file.read_exact(&mut last).map_err(native_error)?;
    if last[0] == b'\n' {
        return Ok(());
    }
    let mut end = length;
    let mut buffer = [0; 4096];
    while end > 0 {
        let start = end.saturating_sub(4096);
        let count = usize::try_from(end - start).map_err(native_error)?;
        file.seek(SeekFrom::Start(start)).map_err(native_error)?;
        file.read_exact(&mut buffer[..count])
            .map_err(native_error)?;
        if let Some(position) = buffer[..count].iter().rposition(|byte| *byte == b'\n') {
            return file
                .set_len(start + position as u64 + 1)
                .map_err(native_error);
        }
        end = start;
    }
    file.set_len(0).map_err(native_error)
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

fn revision_exists(root: &Path, event: &ActivityEvent) -> bool {
    let Ok(identity) = ItemId::try_from(event.source_item_id.as_deref().unwrap_or(&event.item_id))
    else {
        return false;
    };
    let item = identity.path(root);
    if revision_in_file(&item, &event.revision) {
        return true;
    }
    let history = identity.directory(root).join("history");
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
    crate::engine::project::item::is_canonical_date(value)
}

fn days_from_date(value: &str) -> Option<u64> {
    let day = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some(
        day.signed_duration_since(epoch)
            .num_days()
            .max(0)
            .cast_unsigned(),
    )
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
    fn saving_and_watching_one_day_do_not_read_unrelated_journals() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(root.path(), "UTC").unwrap();
        let date = chrono::Utc::now().date_naive().to_string();
        let event = ActivityEvent {
            source_item_id: None,
            id: "one-edit".to_owned(),
            date: date.clone(),
            occurred_at: format!("{date}T10:00:00.000Z"),
            item_id: uuid::Uuid::now_v7().to_string(),
            revision: uuid::Uuid::now_v7().to_string(),
            kind: "content".to_owned(),
            words_added: 3,
            words_deleted: 0,
            net_word_change: 3,
        };
        let unrelated = root.path().join("analytics/events/2000-01-01.jsonl");
        std::fs::write(&unrelated, "broken unrelated journal\n").unwrap();
        prepare_item_events(root.path(), std::slice::from_ref(&event)).unwrap();
        super::finish_item_events(root.path(), std::slice::from_ref(&event)).unwrap();
        // Retrying completion and watcher reconciliation must not double-count.
        super::finish_item_events(root.path(), std::slice::from_ref(&event)).unwrap();
        let dates = std::collections::HashSet::from([date.clone()]);
        super::reconcile_activity_dates(root.path(), &dates).unwrap();
        let day = read_daily_activity(root.path(), &date).unwrap().unwrap();
        assert_eq!(day.items[&event.item_id].words_added, 3);
        assert_eq!(day.items[&event.item_id].editing_events, 1);
        // Full recovery still reports corrupt historical journals.
        assert!(reconcile_activity(root.path().to_string_lossy().into_owned()).is_err());
        std::fs::remove_file(unrelated).unwrap();
        let summaries: Value = serde_json::from_str(
            &reconcile_activity(root.path().to_string_lossy().into_owned()).unwrap(),
        )
        .unwrap();
        assert_eq!(summaries[0]["wordsAdded"], 3);
        root.close().unwrap();
    }

    #[test]
    fn journal_tail_repair_preserves_complete_records_across_chunk_boundaries() {
        for (complete, partial) in [("", 0), ("", 9000), ("record\n", 0), ("record\n", 9000)] {
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(complete.as_bytes()).unwrap();
            file.write_all(&vec![b'x'; partial]).unwrap();
            super::truncate_partial_line(&mut file).unwrap();
            assert_eq!(file.metadata().unwrap().len(), complete.len() as u64);
            super::truncate_partial_line(&mut file).unwrap();
            assert_eq!(file.metadata().unwrap().len(), complete.len() as u64);
        }
    }

    #[test]
    fn recovery_finds_current_and_historical_revisions_for_uppercase_ids() {
        let root = tempfile::tempdir().unwrap();
        let id = "ABCDEF01-2345-7000-ABCD-EF0123456789";
        let directory = root.path().join("items").join("ab").join(id);
        let history = directory.join("history");
        std::fs::create_dir_all(&history).unwrap();
        std::fs::write(directory.join("item.json"), br#"{"revision":"current"}"#).unwrap();
        std::fs::write(history.join("previous.json"), br#"{"revision":"previous"}"#).unwrap();
        let mut event = ActivityEvent {
            source_item_id: None,
            id: "recovery".into(),
            date: "2026-10-20".into(),
            occurred_at: "2026-10-20T10:00:00Z".into(),
            item_id: id.into(),
            revision: "current".into(),
            kind: "task_completed".into(),
            words_added: 0,
            words_deleted: 0,
            net_word_change: 0,
        };
        assert!(super::revision_exists(root.path(), &event));
        "previous".clone_into(&mut event.revision);
        assert!(super::revision_exists(root.path(), &event));
        event.source_item_id = Some(id.into());
        event.item_id = "11111111-1111-7111-8111-111111111111".into();
        assert!(super::revision_exists(root.path(), &event));
        event.source_item_id = None;
        id.clone_into(&mut event.item_id);
        "absent".clone_into(&mut event.revision);
        assert!(!super::revision_exists(root.path(), &event));
        "../escape".clone_into(&mut event.item_id);
        assert!(!super::revision_exists(root.path(), &event));
        root.close().unwrap();
    }

    #[test]
    fn applies_a_prepared_event_once_after_a_durable_item_write() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let item: Value = serde_json::from_slice(
            &std::fs::read(fixture.join("items/01/01a0e2a0-0000-7000-8000-000000000001/item.json"))
                .unwrap(),
        )
        .unwrap();
        let id = item["id"].as_str().unwrap().to_owned();
        let revision = item["revision"].as_str().unwrap().to_owned();
        let event_id = format!("{id}:{revision}:recovered");
        // Keep recovery events inside retention regardless of when tests run.
        let today = chrono::Utc::now().date_naive();
        let previous_date = today.pred_opt().unwrap().to_string();
        let date = today.to_string();
        let event = ActivityEvent {
            source_item_id: None,
            id: event_id.clone(),
            date: date.clone(),
            occurred_at: format!("{date}T10:00:00.000Z"),
            item_id: id,
            revision,
            kind: "task_completed".to_owned(),
            words_added: 0,
            words_deleted: 0,
            net_word_change: 0,
        };
        prepare_item_events(&root, std::slice::from_ref(&event)).unwrap();
        assert!(std::fs::read(root.join(format!("analytics/{date}.json"))).is_err());
        write_project_item(&root, &item, None, &date).unwrap();
        let first: Value =
            serde_json::from_str(&reconcile_activity(root.to_string_lossy().into_owned()).unwrap())
                .unwrap();
        assert_eq!(first[0]["tasksCompleted"], 1);
        let second: Value =
            serde_json::from_str(&reconcile_activity(root.to_string_lossy().into_owned()).unwrap())
                .unwrap();
        assert_eq!(second[0]["tasksCompleted"], 1);
        let day: Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("analytics/{date}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_daily_activity(&root, &date)
                .unwrap()
                .map(|day| serde_json::to_value(day).unwrap()),
            Some(day.clone())
        );
        assert!(
            read_daily_activity(&root, &previous_date)
                .unwrap()
                .is_none()
        );
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
        let journal = root.join(format!("analytics/events/{date}.jsonl"));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap()
            .write_all(b"{\"partial")
            .unwrap();
        prepare_item_events(&root, &[event]).unwrap();
        let journal_source = std::fs::read_to_string(journal).unwrap();
        assert!(!journal_source.contains("partial"));
        std::fs::write(root.join(format!("analytics/{date}.json")), b"{broken").unwrap();
        assert!(
            reconcile_activity(root.to_string_lossy().into_owned())
                .unwrap_err()
                .contains(&format!("{date}.json"))
        );
        temporary.close().unwrap();
    }
}
