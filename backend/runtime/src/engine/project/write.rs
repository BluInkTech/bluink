// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Instant;
use std::time::{Duration, SystemTime};

use crate::engine::project::io::{LockMode, lock_file};
use chrono::{DateTime, Local, Timelike};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::engine::project::activity::{
    events_for_item_write, finish_item_events, prepare_item_events,
};
use crate::engine::project::assemble::assemble_item;
use crate::engine::project::config::read_project_config;
use crate::engine::project::identity::{ItemId, is_item_id};
use crate::engine::project::io::write_json_atomic;
use crate::engine::project::item::{read_project_item, validate_item};
use crate::engine::project::properties::with_validated_metadata;

#[derive(Debug)]
pub(crate) enum MutationError {
    Validation(String),
    NotFound(String),
    RevisionConflict(String),
    Internal(String),
    Serialization(serde_json::Error),
    Storage(super::storage_error::StorageError),
    Read(super::item::ItemReadError),
}

impl From<super::storage_error::StorageError> for MutationError {
    fn from(error: super::storage_error::StorageError) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message)
            | Self::NotFound(message)
            | Self::RevisionConflict(message)
            | Self::Internal(message) => formatter.write_str(message),
            Self::Serialization(error) => std::fmt::Display::fmt(error, formatter),
            Self::Storage(error) => std::fmt::Display::fmt(error, formatter),
            Self::Read(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for MutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Read(error) => Some(error),
            _ => None,
        }
    }
}

/// Read, change, assemble, and commit one existing item under the same OS lock.
/// Domain checks in `change` remain distinct from storage failures at the NAPI boundary.
#[tracing::instrument(level = "debug", skip_all, fields(has_expected_revision = expected_revision.is_some()))]
pub(crate) fn mutate_existing_item(
    root: &Path,
    id: &str,
    expected_revision: Option<&str>,
    activity_date: &str,
    change: impl FnOnce(&Value) -> Result<Value, MutationError>,
) -> Result<Value, MutationError> {
    let _mutation = super::transaction::lock(root, LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    if !is_item_id(id) {
        return Err(MutationError::Validation(format!(
            "Invalid Bluink item id: {id}"
        )));
    }
    let config = read_project_config(root)
        .map_err(MutationError::Internal)?
        .ok_or_else(|| MutationError::Internal("Project configuration is missing.".to_owned()))?;
    let _lock = lock_item(root, id).map_err(MutationError::Storage)?;
    let existing = read_project_item(root, id)
        .map_err(MutationError::Read)?
        .ok_or_else(|| MutationError::NotFound(format!("Item {id} not found")))?;
    if let Some(revision) = expected_revision
        && existing["revision"].as_str() != Some(revision)
    {
        return Err(MutationError::RevisionConflict(
            "Version conflict: the record was modified by another writer".to_owned(),
        ));
    }
    let input = change(&existing)?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Project has no timezone".to_owned()))?;
    let next =
        assemble_item(&input, Some(&existing), timezone).map_err(MutationError::Validation)?;
    let outcome = commit_locked(
        root,
        &next,
        Some(&existing),
        existing["revision"].as_str(),
        activity_date,
        &config,
    )?;
    Ok(json!({"item": next, "activityWarning": outcome.warning, "relatedItems": outcome.related}))
}

/// Commit a prepared item with a revision precondition. The per-item OS lock
/// covers stale checks, history, and the atomic canonical replacement.
#[cfg(test)]
pub(crate) fn write_project_item(
    root: &Path,
    next: &Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> Result<Option<String>, String> {
    let _mutation =
        super::transaction::lock(root, LockMode::Exclusive).map_err(|error| error.to_string())?;
    super::transaction::recover(root)?;
    let id = next.get("id").and_then(Value::as_str).unwrap_or("");
    if !is_item_id(id) {
        return Err(format!("Invalid Bluink item id: {id}"));
    }
    let config = read_project_config(root)?.ok_or("Project configuration is missing.")?;
    let _lock = lock_item(root, id).map_err(|error| error.to_string())?;
    let existing = read_project_item(root, id).map_err(|error| error.to_string())?;
    commit_locked(
        root,
        next,
        existing.as_ref(),
        expected_revision,
        activity_date,
        &config,
    )
    .map(|outcome| outcome.warning)
    .map_err(|error| error.to_string())
}

/// Assemble and commit a version-one item while holding its OS lock. Node
/// sends intent and an expected revision; Rust owns the canonical envelope.
#[tracing::instrument(level = "debug", skip_all, fields(has_expected_revision = expected_revision.is_some()))]
#[cfg(test)]
pub(crate) fn write_item_input(
    root: &Path,
    input: Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> Result<Value, String> {
    write_item_input_typed(root, input, expected_revision, activity_date)
        .map_err(|error| error.to_string())
}

pub(crate) fn write_item_input_typed(
    root: &Path,
    mut input: Value,
    expected_revision: Option<&str>,
    activity_date: &str,
) -> Result<Value, MutationError> {
    let _mutation = super::transaction::lock(root, LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    let id = match input.get("id") {
        Some(Value::String(id)) => id.clone(),
        Some(_) => {
            return Err(MutationError::Validation(
                "Invalid Bluink item id.".to_owned(),
            ));
        }
        None => Uuid::now_v7().to_string(),
    };
    if !is_item_id(&id) {
        return Err(MutationError::Validation(format!(
            "Invalid Bluink item id: {id}"
        )));
    }
    let input_map = input.as_object_mut().ok_or_else(|| {
        MutationError::Validation("Item write input must be an object.".to_owned())
    })?;
    input_map.insert("id".to_owned(), json!(id));
    let config = read_project_config(root)
        .map_err(MutationError::Internal)?
        .ok_or_else(|| MutationError::Internal("Project configuration is missing.".to_owned()))?;
    let _lock = lock_item(root, &id).map_err(MutationError::Storage)?;
    let existing = read_project_item(root, &id).map_err(MutationError::Read)?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Project has no timezone".to_owned()))?;
    let next =
        assemble_item(&input, existing.as_ref(), timezone).map_err(MutationError::Validation)?;
    let outcome = commit_locked(
        root,
        &next,
        existing.as_ref(),
        expected_revision,
        activity_date,
        &config,
    )?;
    Ok(json!({"item": next, "activityWarning": outcome.warning, "relatedItems": outcome.related}))
}

pub(crate) fn lock_item(
    root: &Path,
    id: &str,
) -> Result<std::fs::File, super::storage_error::StorageError> {
    lock_file(
        &root.join(".cache/item-locks").join(format!("{id}.lock")),
        LockMode::Exclusive,
    )
}

pub(crate) fn lock_relationships(
    root: &Path,
) -> Result<std::fs::File, super::storage_error::StorageError> {
    lock_file(&root.join(".cache/relationships.lock"), LockMode::Exclusive)
}
struct CommitOutcome {
    warning: Option<String>,
    related: Vec<Value>,
}

fn commit_locked(
    root: &Path,
    next: &Value,
    existing: Option<&Value>,
    expected_revision: Option<&str>,
    activity_date: &str,
    config: &Value,
) -> Result<CommitOutcome, MutationError> {
    let started = Instant::now();
    let id = next
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| MutationError::Validation("Item has no ID".to_owned()))?;
    validate_item(next, id).map_err(MutationError::Validation)?;
    if matches!(next["meta"]["type"].as_str(), Some("page" | "template")) {
        let document = crate::engine::document::document_root(&next["content"]);
        crate::engine::document::links::validate_http_links(document, None)
            .map_err(MutationError::Validation)?;
    }
    let actual_revision = existing.and_then(|item| item["revision"].as_str());
    if actual_revision != expected_revision {
        return Err(MutationError::RevisionConflict(format!(
            "Item {id} changed since it was read. Reload it before saving."
        )));
    }
    if let Some(existing) = existing {
        if next["meta"]["type"] != existing["meta"]["type"] {
            return Err(MutationError::Validation(
                "An item type cannot change after creation.".to_owned(),
            ));
        }
        if next["createdAt"] != existing["createdAt"] {
            return Err(MutationError::Validation(
                "An item's creation time cannot change.".to_owned(),
            ));
        }
        if next["revision"] == existing["revision"] {
            return Err(MutationError::Validation(
                "An item write needs a new revision.".to_owned(),
            ));
        }
        if existing["meta"]["type"] == "goal" && next["content"] != existing["content"] {
            return Err(MutationError::Validation(
                "Writing goal dates and targets cannot change. End this goal and create another."
                    .to_owned(),
            ));
        }
    }
    // Keep relationship and pin-scope validation serialized with the canonical write.
    let _relationships = lock_relationships(root).map_err(MutationError::Storage)?;
    super::task::validate_item_change(root, next, existing)?;
    super::snippets::validate_transition(root, existing, next)?;
    super::named_items::validate_mutation(root, existing, next)?;
    if next["meta"]["type"] == "workspace" && next["meta"]["state"] != "active" {
        let fresh = read_project_config(root)
            .map_err(MutationError::Internal)?
            .ok_or_else(|| MutationError::Internal("Project configuration is missing.".into()))?;
        if fresh["defaultWorkspaceId"].as_str() == Some(id) {
            return Err(MutationError::Validation("Choose another default workspace before removing this one. The last active workspace cannot be removed.".into()));
        }
    }
    crate::engine::publication::theme_store::validate_publication_reference(root, next)
        .map_err(MutationError::Validation)?;
    if matches!(next["meta"]["type"].as_str(), Some("page" | "template")) {
        crate::engine::project::snippets::validate_page_snippet_references(
            root,
            crate::engine::publication::source::document(next),
        )
        .map_err(MutationError::Validation)?;
    }
    crate::engine::project::goal::validate_goal_activation(root, existing, next)?;
    validate_new_pin_scopes(root, existing, next).map_err(MutationError::Validation)?;
    let activity =
        events_for_item_write(existing, next, activity_date).map_err(MutationError::Internal)?;
    let meta = next["meta"]
        .as_object()
        .ok_or_else(|| MutationError::Validation("Item metadata must be an object.".to_owned()))?;
    let identity = ItemId::try_from(id).map_err(MutationError::Validation)?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Project has no timezone".into()))?;
    let related = super::task::archive_children(root, next, timezone)?;
    let outcome = with_validated_metadata(root, meta, || {
        prepare_item_events(root, &activity).map_err(MutationError::Internal)?;
        let item_dir = identity.directory(root);
        if let Some(existing) = &existing
            && config["history"]["enabled"] == true
            && existing["analytics"]["current"]["contentHash"]
                != next["analytics"]["current"]["contentHash"]
        {
            record_history(&item_dir, existing, config)?;
        }
        persist_item_group(root, next, &related)?;
        // The pending journal entry is already durable. If projection fails,
        // startup reconciliation can still apply it from the saved revision.
        Ok(CommitOutcome {
            warning: finish_item_events(root, &activity).err(),
            related,
        })
    })?;
    tracing::debug!(
        operation = "commit_item",
        item_type = next["meta"]["type"].as_str().unwrap_or_default(),
        related_items = outcome.related.len(),
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(outcome)
}

/// Save the one canonical record or recoverable cascade before publication.
fn persist_item_group(root: &Path, next: &Value, related: &[Value]) -> Result<(), MutationError> {
    if !related.is_empty() {
        let mut group = vec![next.clone()];
        group.extend_from_slice(related);
        return super::transaction::write_group(root, group).map_err(MutationError::Internal);
    }
    persist_single_item(root, next)
}

/// Shared canonical replacement and observer notification for single items.
/// Callers hold the project mutation gate and the item's lock.
pub(crate) fn persist_single_item(root: &Path, next: &Value) -> Result<(), MutationError> {
    let id = next["id"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Item has no ID".into()))?;
    let revision = next["revision"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Item has no revision".into()))?;
    let path = ItemId::try_from(id)
        .map_err(MutationError::Validation)?
        .path(root);
    write_json_atomic(&path, next)?;
    super::usage::pending(root, id, revision).map_err(MutationError::Internal)?;
    if super::named_items::is_named(next["meta"]["type"].as_str().unwrap_or_default()) {
        super::named_items::observe(root, id).map_err(MutationError::Internal)?;
    }
    Ok(())
}

fn record_history(item_dir: &Path, existing: &Value, config: &Value) -> Result<(), MutationError> {
    let history_dir = item_dir.join("history");
    let interval = config["history"]["interval"].as_str().ok_or_else(|| {
        MutationError::Internal("Validated project configuration has no history interval.".into())
    })?;
    let bucket = history_bucket(Local::now(), interval);
    write_json_atomic(&history_dir.join(format!("{bucket}.item.json")), existing)
        .map_err(MutationError::Storage)?;
    let retention_days = config["history"]["retention"]["hourlyDays"]
        .as_u64()
        .ok_or_else(|| {
            MutationError::Internal(
                "Validated project configuration has no history retention.".into(),
            )
        })?;
    prune_history(&history_dir, retention_days).map_err(MutationError::Internal)?;
    Ok(())
}

fn validate_new_pin_scopes(
    root: &Path,
    existing: Option<&Value>,
    next: &Value,
) -> Result<(), String> {
    let previous: HashSet<&str> = existing
        .and_then(|item| item.get("pins"))
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|pins| pins.iter().filter_map(Value::as_str))
        .collect();
    for scope_id in next
        .get("pins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if previous.contains(scope_id) {
            continue;
        }
        let scope = read_project_item(root, scope_id).map_err(|error| error.to_string())?;
        if !scope.is_some_and(|item| {
            item["meta"]["type"] == "workspace" && item["meta"]["state"] == "active"
        }) {
            return Err("Pins can only target an active workspace.".to_owned());
        }
    }
    Ok(())
}

fn history_bucket(now: DateTime<Local>, interval: &str) -> String {
    if interval == "1d" {
        return now.format("%Y-%m-%d").to_string();
    }
    let hour = if interval == "4h" {
        now.hour() / 4 * 4
    } else {
        now.hour()
    };
    format!("{}-{hour:02}", now.format("%Y-%m-%d"))
}

fn prune_history(directory: &Path, retention_days: u64) -> Result<(), String> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days.saturating_mul(86_400)))
        .ok_or("Invalid history retention")?;
    for entry in
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".item.json"))
            || !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
        {
            continue;
        }
        if entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(|error| error.to_string())?
            < cutoff
        {
            fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{Local, TimeZone};

    use super::{history_bucket, write_item_input, write_project_item};
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::item::read_project_item;

    #[test]
    fn assembles_and_commits_input_under_the_revision_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let input = serde_json::json!({
            "id": id,
            "meta": {"type": "page", "title": "First"},
            "content": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Two words"}]}]},
            "tags": ["astronomy", "astronomy"]
        });
        let first = write_item_input(&root, input.clone(), None, "2026-09-28").unwrap();
        assert_eq!(first["item"]["analytics"]["current"]["wordCount"], 2);
        assert_eq!(first["item"]["tags"], serde_json::json!(["astronomy"]));
        assert!(first["item"]["content"]["content"][0]["attrs"]["id"].is_string());
        assert_eq!(
            read_project_item(&root, &id).unwrap().unwrap(),
            first["item"]
        );
        let revision = first["item"]["revision"].as_str().unwrap();
        let mut changed = input;
        changed["meta"]["title"] = serde_json::json!("Second");
        let second =
            write_item_input(&root, changed.clone(), Some(revision), "2026-09-28").unwrap();
        assert_eq!(second["item"]["createdAt"], first["item"]["createdAt"]);
        assert_eq!(second["item"]["meta"]["title"], "Second");
        let patch = serde_json::json!({"id": id, "meta": {}, "tags": ["physics"]});
        let patched = write_item_input(
            &root,
            patch,
            second["item"]["revision"].as_str(),
            "2026-09-28",
        )
        .unwrap();
        assert_eq!(patched["item"]["content"], second["item"]["content"]);
        assert_eq!(patched["item"]["meta"], second["item"]["meta"]);
        assert_eq!(patched["item"]["tags"], serde_json::json!(["physics"]));
        assert!(
            write_item_input(&root, changed, Some(revision), "2026-09-28")
                .unwrap_err()
                .contains("changed since it was read")
        );
        temporary.close().unwrap();
    }

    #[test]
    fn http_link_syntax_is_checked_before_save_without_contacting_servers() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let make_input = |href: &str| {
            serde_json::json!({
                "id":id,"meta":{"type":"page","title":"Private network links"},
                "content":{"type":"doc","content":[{"type":"paragraph","content":[{
                    "type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{
                        "type":"text","text":"Corporate reference","marks":[{"type":"link","attrs":{"href":href}}]
                    }]}}}
                ]}]}
            })
        };
        let first = write_item_input(
            &root,
            make_input("https://intranet.corp.internal/manual"),
            None,
            "2026-09-30",
        )
        .unwrap();
        let second = write_item_input(
            &root,
            make_input("http://127.0.0.1:9/offline"),
            first["item"]["revision"].as_str(),
            "2026-09-30",
        )
        .unwrap();
        let error = write_item_input(
            &root,
            make_input("https://doi:10.21926/example"),
            second["item"]["revision"].as_str(),
            "2026-09-30",
        )
        .unwrap_err();
        assert!(error.contains("URL"), "{error}");
        assert_eq!(
            read_project_item(&root, &id).unwrap().unwrap(),
            second["item"]
        );
        // Prepared/imported writes use the same transaction validation.
        let mut prepared = second["item"].clone();
        prepared["revision"] = serde_json::json!(uuid::Uuid::now_v7().to_string());
        prepared["content"]["content"][0]["content"][0]["attrs"]["paragraph"]["content"][0]["marks"]
            [0]["attrs"]["href"] = serde_json::json!("https://[broken");
        assert!(
            write_project_item(
                &root,
                &prepared,
                second["item"]["revision"].as_str(),
                "2026-09-30"
            )
            .unwrap_err()
            .contains("URL")
        );
        assert_eq!(
            read_project_item(&root, &id).unwrap().unwrap(),
            second["item"]
        );
        temporary.close().unwrap();
    }

    #[test]
    fn rejects_independently_authored_page_references() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let input = serde_json::json!({"id":id,"meta":{"type":"page","title":"Page"},
            "content":{"type":"doc","content":[{"type":"paragraph"}]}});
        let first = write_item_input(&root, input.clone(), None, "2026-09-28").unwrap();
        let mut self_reference = input;
        self_reference["references"] = serde_json::json!([id]);
        assert!(
            write_item_input(
                &root,
                self_reference,
                first["item"]["revision"].as_str(),
                "2026-09-28"
            )
            .unwrap_err()
            .contains("authored in the document")
        );
        temporary.close().unwrap();
    }

    #[test]
    fn rejects_stale_writes_and_snapshots_changed_content() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project/bluink.json"),
            )
            .unwrap(),
        )
        .unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "test-fixtures/pdf-project/items/01/01a0e2a0-0000-7000-8000-000000000001/item.json",
        );
        let mut item: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let id = item["id"].as_str().unwrap().to_owned();
        let first_revision = item["revision"].as_str().unwrap().to_owned();
        write_project_item(&root, &item, None, "2026-09-28").unwrap();
        assert!(write_project_item(&root, &item, None, "2026-09-28").is_err());
        item["revision"] = serde_json::json!("01a0e2a7-d131-792d-a0bd-26a49033486e");
        item["analytics"]["current"]["contentHash"] = serde_json::json!("changed");
        write_project_item(&root, &item, Some(&first_revision), "2026-09-28").unwrap();
        assert_eq!(
            read_project_item(&root, &id).unwrap().unwrap()["revision"],
            item["revision"]
        );
        assert!(write_project_item(&root, &item, Some(&first_revision), "2026-09-28").is_err());
        let history = root.join("items").join("01").join(&id).join("history");
        assert_eq!(std::fs::read_dir(history).unwrap().count(), 1);
        temporary.close().unwrap();
    }

    #[test]
    fn uses_local_hourly_and_daily_history_buckets() {
        let date = Local
            .with_ymd_and_hms(2026, 9, 16, 14, 30, 0)
            .single()
            .unwrap();
        assert_eq!(history_bucket(date, "1h"), "2026-09-16-14");
        assert_eq!(history_bucket(date, "4h"), "2026-09-16-12");
        assert_eq!(history_bucket(date, "1d"), "2026-09-16");
    }

    #[test]
    fn checks_property_declarations_before_an_item_is_committed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        crate::engine::project::properties::create_property(
            &root,
            serde_json::json!({"key":"workflow","valueType":"select","options":["draft"]}),
        )
        .unwrap();
        let mut item: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.join("items/01/01a0e2a0-0000-7000-8000-000000000001/item.json"))
                .unwrap(),
        )
        .unwrap();
        item["meta"]["workflow"] = serde_json::json!("review");
        assert!(
            write_project_item(&root, &item, None, "2026-09-28")
                .unwrap_err()
                .contains("declared option")
        );
        let id = item["id"].as_str().unwrap();
        assert!(read_project_item(&root, id).unwrap().is_none());
        item["meta"]["workflow"] = serde_json::json!("draft");
        write_project_item(&root, &item, None, "2026-09-28").unwrap();
        temporary.close().unwrap();
    }

    #[test]
    fn checks_new_pin_targets_in_the_item_transaction() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let mut item: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.join("items/01/01a0e2a0-0000-7000-8000-000000000001/item.json"))
                .unwrap(),
        )
        .unwrap();
        let missing_scope = "01a0e2a7-d131-792d-a0bd-26a49033486e";
        item["pins"] = serde_json::json!([missing_scope]);
        assert!(
            write_project_item(&root, &item, None, "2026-09-28")
                .unwrap_err()
                .contains("active workspace")
        );
        let mut scope = item.clone();
        let scope_id = "01a0e2a7-d131-792d-a0bd-26a49033486f";
        scope["id"] = serde_json::json!(scope_id);
        scope["meta"]["type"] = serde_json::json!("workspace");
        scope["content"] = crate::engine::project::workspace::initial_content();
        scope.as_object_mut().unwrap().remove("pins");
        write_project_item(&root, &scope, None, "2026-09-28").unwrap();
        item["pins"] = serde_json::json!([scope_id]);
        write_project_item(&root, &item, None, "2026-09-28").unwrap();
        temporary.close().unwrap();
    }
}
