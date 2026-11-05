// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    History, Result, failure,
    model::{
        HistoryChange, HistoryItemChange, HistoryPreview, HistoryPreviewRequest,
        HistoryRestoreRequest,
    },
    repository::{self, Kind},
};
use crate::domain::project::{self, identity::ItemId};
use crate::store::io::{self, LockMode};
use git2::{Oid, Repository};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

mod scoped;
#[cfg(test)]
mod tests;

struct Candidate {
    head: Oid,
    generation: i64,
    changes: BTreeMap<String, Option<Oid>>,
    preview: HistoryPreview,
}

impl History {
    pub(crate) fn preview_restore(
        &self,
        request: &HistoryPreviewRequest,
    ) -> Result<HistoryPreview> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = super::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        self.checkpoint_locked(&repo)?;
        Ok(self.prepare_restore(&repo, request)?.preview)
    }

    fn prepare_restore(
        &self,
        repo: &Repository,
        request: &HistoryPreviewRequest,
    ) -> Result<Candidate> {
        let selected = super::history::reachable(repo, &request.commit_id)?;
        if request.item_id.is_none() {
            self.ensure_history_attachments(repo, selected)?;
        }
        let (head, generation) = self.restore_base(repo)?;
        let (current, target, archived_tasks) = if let Some(id) = &request.item_id {
            scoped::destination(self, repo, head, selected, generation, id)?
        } else {
            let current = repository::files(repo, head)?;
            let target = repository::files(repo, selected)?;
            validate_target(repo, &target, &current)?;
            (current, target, Vec::new())
        };
        let mut changes = BTreeMap::new();
        let mut changed_items = Vec::new();
        let mut changed_assets = 0;
        let mut changed_themes = BTreeSet::new();
        let mut changed_settings = false;
        for path in current
            .keys()
            .chain(target.keys().filter(|path| !current.contains_key(*path)))
        {
            if current.get(path) == target.get(path) {
                continue;
            }
            let next = target.get(path).copied();
            if path.starts_with("items/") {
                let oid = next
                    .or_else(|| current.get(path).copied())
                    .ok_or_else(|| failure("Missing changed item"))?;
                let item: Value = serde_json::from_slice(repo.find_blob(oid)?.content())?;
                changed_items.push(HistoryItemChange {
                    id: item["id"]
                        .as_str()
                        .ok_or_else(|| failure("Item has no identity"))?
                        .to_owned(),
                    title: item["meta"]["title"]
                        .as_str()
                        .or_else(|| item["content"]["key"].as_str())
                        .unwrap_or("Untitled")
                        .to_owned(),
                    change: if next.is_none() {
                        HistoryChange::Removed
                    } else if current.contains_key(path) {
                        HistoryChange::Changed
                    } else {
                        HistoryChange::Added
                    },
                });
            } else if path.starts_with("assets/") {
                changed_assets += 1;
            } else if path.starts_with("themes/") {
                changed_themes.insert(path.split('/').nth(1).unwrap_or_default().to_owned());
            } else {
                changed_settings = true;
            }
            changes.insert(path.clone(), next);
        }
        for (path, oid) in &changes {
            if repository::tracked(path) == Some(Kind::Binary)
                && let Some(oid) = oid
            {
                self.ensure_history_asset(
                    repo,
                    &super::lfs::Pointer::parse(repo.find_blob(*oid)?.content())?,
                )?;
            }
        }
        let preview = HistoryPreview {
            commit_id: request.commit_id.clone(),
            item_id: request.item_id.clone(),
            expected_head: head.to_string(),
            expected_generation: generation.to_string(),
            changed_items,
            changed_assets,
            changed_settings,
            changed_themes: changed_themes.len(),
            archived_tasks,
        };
        Ok(Candidate {
            head,
            generation,
            changes,
            preview,
        })
    }

    pub(crate) fn apply_restore(&self, request: &HistoryRestoreRequest) -> Result<()> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = super::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        self.checkpoint_locked(&repo)?;
        let mut candidate = self.prepare_restore(
            &repo,
            &HistoryPreviewRequest {
                commit_id: request.commit_id.clone(),
                item_id: request.item_id.clone(),
            },
        )?;
        if request.expected_head != candidate.head.to_string()
            || request.expected_generation != candidate.preview.expected_generation
        {
            return Err(failure(
                "The project changed since this preview. Refresh the restore preview before continuing.",
            ));
        }
        // Restore is a new edit: a client holding an old revision must never
        // become current again solely because that historical content returns.
        for (path, oid) in &mut candidate.changes {
            if path.starts_with("items/")
                && let Some(previous) = oid
            {
                let mut item: Value = serde_json::from_slice(repo.find_blob(*previous)?.content())?;
                item["revision"] = json!(uuid::Uuid::now_v7().to_string());
                item["modifiedAt"] =
                    json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
                *oid = Some(repo.blob(&project::canonical_json::to_vec(&item)?)?);
            }
        }
        let tree = repository::changed_tree(&repo, Some(candidate.head), &candidate.changes)?;
        let commit = self.commit(
            &repo,
            Some(candidate.head),
            tree,
            &format!(
                "Restore version\n\nBluink-Restored-From: {}",
                request.commit_id
            ),
        )?;
        let delta = super::checkpoints::delta::prepare(&self.root, &repo, candidate.head, commit)?;
        super::checkpoints::delta::apply(
            &repo,
            self.canonical()?.as_ref(),
            delta,
            super::checkpoints::delta::Publication {
                previous: candidate.head,
                candidate: commit,
                generation: candidate.generation,
                tags: BTreeMap::new(),
                cancelled: self.cancellation()?,
            },
        )
        .map(|_| ())
    }
    fn restore_base(&self, repo: &Repository) -> Result<(Oid, i64)> {
        let canonical = self.canonical()?;
        let (expected, generation, pending) = canonical
            .read(|tx| {
                Ok((
                    crate::store::git::expected_head(tx)?,
                    tx.query_row::<i64, _, _>("SELECT generation FROM project_state", [], |row| {
                        row.get(0)
                    })?,
                    tx.query_row::<bool, _, _>(
                        "SELECT EXISTS(SELECT 1 FROM dirty_path)",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .map_err(|error| failure(error.to_string()))?;
        let head = repository::head(repo)?.ok_or_else(|| failure("Project has no checkpoints"))?;
        if pending || expected.as_deref() != Some(head.to_string().as_str()) {
            return Err(failure(
                "Project changed during preparation. Refresh the preview.",
            ));
        }
        Ok((head, generation))
    }
    pub(super) fn apply_sync_commit(
        &self,
        repo: &Repository,
        previous: Oid,
        candidate: Oid,
        generation: i64,
        tags: &BTreeMap<String, Oid>,
    ) -> Result<()> {
        let delta = super::checkpoints::delta::prepare(&self.root, repo, previous, candidate)?;
        if delta.needs_maintenance() {
            self.sync_phase(&format!(
                "Applying a large sync change ({} paths). Queued saves may need to retry.",
                delta.changed_paths
            ))?;
        }
        super::checkpoints::delta::apply(
            repo,
            self.canonical()?.as_ref(),
            delta,
            super::checkpoints::delta::Publication {
                previous,
                candidate,
                generation,
                tags: tags.clone(),
                cancelled: self.cancellation()?,
            },
        )
        .map(|_| ())
    }
}
fn validate_target(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    current: &BTreeMap<String, Oid>,
) -> Result<()> {
    validate_candidate(repo, files, current, true)
}

fn project_config(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    missing: &str,
) -> Result<Value> {
    let oid = files.get("bluink.json").ok_or_else(|| failure(missing))?;
    Ok(project::config::parse_project_config(
        repo.find_blob(*oid)?.content(),
    )?)
}

fn json(repo: &Repository, oid: Oid) -> Result<Value> {
    Ok(serde_json::from_slice(repo.find_blob(oid)?.content())?)
}

fn item_id(path: &str) -> Result<&str> {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| failure("Invalid item path"))
}

/// `current` is an accepted state. Besides the shared checks of
/// `validate_changes`, the candidate must keep the project identity and every
/// current decision.
pub(super) fn validate_candidate(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    current: &BTreeMap<String, Oid>,
    restoring: bool,
) -> Result<()> {
    let config = project_config(repo, files, "Checkpoint has no project configuration")?;
    let previous = project_config(
        repo,
        current,
        "Current history has no project configuration",
    )?;
    if config["id"] != previous["id"] {
        return Err(failure("Checkpoint belongs to another project"));
    }
    // Restoring older prose must not erase a resolution or reopen its decision.
    // Until restore has an explicit decision policy, preserve current decision
    // content exactly; checking only target records would miss removed files.
    for (path, oid) in current {
        if !path.starts_with("items/") || files.get(path) == Some(oid) {
            continue;
        }
        let previous = json(repo, *oid)?;
        if previous["meta"]["type"] != "conflict" {
            continue;
        }
        let next = files.get(path).map(|oid| json(repo, *oid)).transpose()?;
        if next.as_ref().is_none_or(|item| {
            item["meta"]["type"] != "conflict"
                || (restoring && item["content"] != previous["content"])
                || (!previous["content"]["resolution"].is_null()
                    && item["content"]["resolution"].is_null())
        }) {
            return Err(failure(
                "History cannot erase a decision or reopen a resolved decision. Restore also preserves existing decision content.",
            ));
        }
    }
    validate_changes(repo, files, current)
}

/// Validate a tree without an accepted base, such as a fresh clone.
pub(super) fn validate_complete(repo: &Repository, files: &BTreeMap<String, Oid>) -> Result<()> {
    let config = project_config(repo, files, "Checkpoint has no project configuration")?;
    validate_all(repo, &config, files)
}

fn validate_all(repo: &Repository, config: &Value, files: &BTreeMap<String, Oid>) -> Result<()> {
    let mut items = BTreeMap::new();
    for (path, oid) in files {
        if path.starts_with("items/") {
            items.insert(item_id(path)?, *oid);
        }
    }
    project::candidate::validate(config, items.keys().copied(), |id| {
        let blob = repo
            .find_blob(items[id])
            .map_err(|error| error.to_string())?;
        serde_json::from_slice(blob.content()).map_err(|error| error.to_string())
    })?;
    super::sync::validation::packages(repo, files)?;
    Ok(())
}

/// Whether a change can invalidate an unchanged item that depends on it: a
/// removal, a changed type or state, or any property declaration change.
fn affects_dependents(before: Option<&Value>, after: Option<&Value>) -> bool {
    let property = |value: Option<&Value>| value.is_some_and(|v| v["meta"]["type"] == "property");
    match (before, after) {
        (_, None) => true,
        (None, Some(_)) => property(after),
        (Some(old), Some(new)) => {
            old["meta"]["type"] != new["meta"]["type"]
                || old["meta"]["state"] != new["meta"]["state"]
                || (property(before) && old["content"] != new["content"])
        }
    }
}

/// `base` passed complete validation when it was accepted, so an unchanged
/// item stays valid unless one of its dependencies changed. Decode only changed
/// items and their direct dependency targets; a change that can affect
/// unchanged dependents falls back to validating the complete candidate.
pub(super) fn validate_changes(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    base: &BTreeMap<String, Oid>,
) -> Result<()> {
    let config = project_config(repo, files, "Checkpoint has no project configuration")?;
    let mut changed = Vec::new();
    for path in files
        .keys()
        .chain(base.keys().filter(|path| !files.contains_key(*path)))
        .filter(|path| path.starts_with("items/") && files.get(*path) != base.get(*path))
    {
        let before = base.get(path).map(|oid| json(repo, *oid)).transpose()?;
        let after = files.get(path).map(|oid| json(repo, *oid)).transpose()?;
        if affects_dependents(before.as_ref(), after.as_ref()) {
            return validate_all(repo, &config, files);
        }
        if let Some(value) = after {
            changed.push((item_id(path)?, value));
        }
    }
    let mut catalogue = project::candidate::Catalogue::default();
    let mut observed = BTreeSet::new();
    let mut targets = BTreeSet::new();
    if let Some(id) = config["defaultWorkspaceId"].as_str() {
        targets.insert(id.to_owned());
    }
    for (id, value) in &changed {
        catalogue.observe(id, value)?;
        observed.insert((*id).to_owned());
        targets.extend(
            project::candidate::dependencies(value)?
                .iter()
                .map(project::candidate::Dependency::target_id),
        );
    }
    for id in targets.difference(&observed) {
        // An invalid reference is reported by the relationship check below.
        let Ok(identity) = ItemId::try_from(id.as_str()) else {
            continue;
        };
        let path = identity
            .path(Path::new(""))
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(oid) = files.get(&path) {
            catalogue.observe(id, &json(repo, *oid)?)?;
        }
    }
    if let Some(issue) = catalogue.default_workspace(&config)? {
        return Err(failure(issue.to_string()));
    }
    for (_, value) in &changed {
        if let Some(issue) = catalogue.check(value)?.first() {
            return Err(failure(issue.to_string()));
        }
    }
    super::sync::validation::changed_packages(repo, files, base)
}
