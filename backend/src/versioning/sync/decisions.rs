// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{Prepared, candidate, state};
use crate::domain::merge::{
    decisions::{self, Resolution, Selection, file::FileDecision, resolve::Expected},
    model::Fragment,
};
use crate::domain::project;
use crate::store::io::{self, LockMode};
use crate::versioning::{History, Result, failure, repository};
use git2::{Oid, Repository};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct FilePreview {
    pub(crate) decision: Value,
    pub(crate) path: String,
    pub(crate) current: Fragment,
    pub(crate) expected: Expected,
    pub(crate) allowed: Vec<Selection>,
}

fn preview(repo: &Repository, head: Oid, id: &str) -> Result<FilePreview> {
    let files = repository::files(repo, head)?;
    let decision = candidate::read(repo, &files, id)?
        .ok_or_else(|| failure("Decision no longer exists").classified("stale_decision"))?;
    let record = decisions::read(&decision)?;
    if record.resolution.is_some() {
        return Err(failure("Decision is already resolved").classified("stale_decision"));
    }
    let file = record
        .proposal
        .file_decision()
        .ok_or_else(|| failure("Expected a package-file decision"))?;
    let source = source(repo, files.get(&file.path).copied())?;
    let current = file
        .current(source.as_deref())
        .map_err(|e| failure(e).classified("stale_decision"))?;
    let expected = Expected {
        decision_revision: decision["revision"]
            .as_str()
            .ok_or_else(|| failure("Missing decision revision"))?
            .into(),
        targets: BTreeMap::from([
            ("@head".into(), head.to_string()),
            (
                file.path.clone(),
                files
                    .get(&file.path)
                    .map_or_else(|| "missing".into(), ToString::to_string),
            ),
        ]),
    };
    Ok(FilePreview {
        allowed: [
            Selection::Provisional,
            Selection::First,
            Selection::Second,
            Selection::Both,
        ]
        .into_iter()
        .filter(|choice| {
            record.proposal.selected_file(*choice).is_ok_and(|value| {
                file.apply(source.as_deref(), &value).is_ok_and(|next| {
                    next.is_some()
                        || !matches!(
                            file.path.as_str(),
                            "bluink.json" | ".gitignore" | ".gitattributes"
                        )
                })
            })
        })
        .collect(),
        decision,
        path: file.path.clone(),
        current,
        expected,
    })
}

pub(super) fn fragment(repo: &Repository, oid: Option<Oid>) -> Result<Fragment> {
    Ok(source(repo, oid)?.map_or(Fragment::Missing, |s| Fragment::Present(Value::String(s))))
}

fn source(repo: &Repository, oid: Option<Oid>) -> Result<Option<String>> {
    oid.map(|oid| {
        String::from_utf8(repo.find_blob(oid)?.content().to_vec())
            .map_err(|_| failure("Portable package files must be UTF-8 or LFS pointers"))
    })
    .transpose()
}

pub(super) fn apply_choice(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    file: &FileDecision,
    selected: &Fragment,
) -> Result<()> {
    let current = source(repo, files.get(&file.path).copied())?;
    let next = file
        .apply(current.as_deref(), selected)?
        .map_or(Fragment::Missing, |text| {
            Fragment::Present(Value::String(text))
        });
    apply(repo, files, &file.path, &next)
}

pub(super) fn apply(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    path: &str,
    selected: &Fragment,
) -> Result<()> {
    if repository::tracked(path).is_none() || path.starts_with("items/") {
        return Err(failure("Invalid package decision path"));
    }
    match selected {
        Fragment::Missing => {
            files.remove(path);
        }
        Fragment::Present(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| failure("Invalid file choice"))?;
            if repository::tracked(path) == Some(repository::Kind::Binary) {
                super::super::lfs::Pointer::parse(text.as_bytes())?;
            } else {
                super::super::checkpoint::validate_file(path, text.as_bytes())?;
            }
            files.insert(path.into(), repo.blob(text.as_bytes())?);
        }
    }
    Ok(())
}

impl History {
    pub(crate) fn preview_file(&self, id: &str) -> Result<FilePreview> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        self.checkpoint_locked(&repo)?;
        preview(
            &repo,
            repository::head(&repo)?.ok_or_else(|| failure("No checkpoint"))?,
            id,
        )
    }

    pub(crate) fn prepare_file_choice(
        &self,
        id: &str,
        expected: &Expected,
        selection: Selection,
        device: &str,
    ) -> Result<(Prepared, Value, Vec<Value>)> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        self.checkpoint_locked(&repo)?;
        let head = repository::head(&repo)?.ok_or_else(|| failure("No checkpoint"))?;
        let preview = preview(&repo, head, id)?;
        if preview.expected != *expected {
            return Err(failure(
                "Decision or package changed. Refresh the preview before choosing.",
            )
            .classified("stale_decision"));
        }
        let mut record = decisions::read(&preview.decision)?;
        let file = record
            .proposal
            .file_decision()
            .ok_or_else(|| failure("Expected a file decision"))?;
        if preview.current != record.proposal.selected_file(Selection::Provisional)? {
            return Err(failure(
                "The file changed since this decision. Its current content was preserved.",
            )
            .classified("stale_decision"));
        }
        let selected = record.proposal.selected_file(selection)?;
        let before = repository::files(&repo, head)?;
        let mut files = before.clone();
        apply_choice(&repo, &mut files, file, &selected)?;
        record.resolution = Some(Resolution {
            selection,
            device: device.into(),
            resolved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        let config = project::config::parse_project_config(
            repo.find_blob(
                *files
                    .get("bluink.json")
                    .ok_or_else(|| failure("Project configuration cannot be removed"))?,
            )?
            .content(),
        )?;
        let timezone = config["analytics"]["timezone"]
            .as_str()
            .ok_or_else(|| failure("Missing timezone"))?;
        if record.proposal.effective(selection)?.1 == Selection::Both {
            super::assets::keep_both(&repo, &mut files, &record.proposal, selection, timezone)?;
        }
        apply_intents(
            &repo,
            &mut files,
            &record.proposal,
            record
                .resolution
                .as_ref()
                .ok_or_else(|| failure("Missing resolution"))?,
            timezone,
        )?;
        let decision = crate::store::commands::assemble_item(
            &json!({"id":id,"meta":preview.decision["meta"],"content":record}),
            Some(&preview.decision),
            timezone,
        )?;
        candidate::put(&repo, &mut files, &decision)?;
        super::super::restore::validate_candidate(&repo, &files, &before, false)?;
        let changes = super::tree_changes(&before, &files);
        let items = changed_targets(&repo, &changes)?;
        let tree = repository::changed_tree(&repo, Some(head), &changes)?;
        let commit = self.commit(&repo, Some(head), tree, "Resolve package decision")?;
        let generation = self.generation()?;
        repo.reference(super::PENDING, commit, true, "Prepared decision")?;
        Ok((
            Prepared {
                previous: head,
                candidate: commit,
                generation,
                origin: state::origin(&repo)?,
                remote_head: None,
                tags: BTreeMap::new(),
            },
            decision,
            items,
        ))
    }
}

fn changed_targets(
    repo: &Repository,
    changes: &BTreeMap<String, Option<Oid>>,
) -> Result<Vec<Value>> {
    let mut targets = Vec::new();
    for (path, oid) in changes {
        if path.starts_with("items/")
            && let Some(oid) = oid
        {
            let value: Value = serde_json::from_slice(repo.find_blob(*oid)?.content())?;
            if value["meta"]["type"] != "conflict" {
                targets.push(value);
            }
        }
    }
    Ok(targets)
}

pub(super) fn apply_intents(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    proposal: &decisions::Proposal,
    resolution: &Resolution,
    timezone: &str,
) -> Result<()> {
    if !matches!(proposal, decisions::Proposal::Resolution(_)) {
        return Ok(());
    }
    let updates = {
        let snapshot = candidate::captured(repo, files)?;
        let mut updates = BTreeMap::new();
        decisions::resolve::apply_intents(proposal, resolution, &snapshot, &mut updates)
            .map_err(|e| failure(format!("Invalid resolution intent: {e:?}")))?;
        updates
    };
    for (id, value) in updates {
        let previous = candidate::read(repo, files, &id)?;
        let value = crate::store::commands::assemble_item(&value, previous.as_ref(), timezone)?;
        candidate::put(repo, files, &value)?;
    }
    Ok(())
}
