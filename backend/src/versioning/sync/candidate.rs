// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Build a candidate entirely from immutable Git objects before publication.
use crate::domain::merge::{
    self,
    decisions::{self, Proposal},
    model::{Context, Source},
    validation::{Snapshot, Snapshots},
};
use crate::domain::project::{self, identity::ItemId};
use crate::versioning::{Result, failure};
use git2::{Oid, Repository};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) fn read(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    id: &str,
) -> Result<Option<Value>> {
    let path = ItemId::try_from(id)?
        .path(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    files
        .get(&path)
        .map(|oid| Ok(serde_json::from_slice(repo.find_blob(*oid)?.content())?))
        .transpose()
}

fn ids(files: &BTreeMap<String, Oid>) -> Result<BTreeSet<String>> {
    files
        .keys()
        .filter(|path| path.starts_with("items/"))
        .map(|path| {
            let id = Path::new(path)
                .file_stem()
                .and_then(|x| x.to_str())
                .ok_or_else(|| failure("Invalid item path"))?;
            ItemId::try_from(id)?;
            Ok(id.to_owned())
        })
        .collect()
}

pub(super) fn captured<'a>(
    repo: &'a Repository,
    files: &'a BTreeMap<String, Oid>,
) -> Result<impl Snapshot + 'a> {
    Ok(merge::capture::Captured::new(ids(files)?, |id: &str| {
        read(repo, files, id).map_err(|e| e.to_string())
    })?)
}

fn source(repo: &Repository, oid: Oid) -> Result<Source> {
    let commit = repo.find_commit(oid)?;
    let device = device_name(&commit).to_owned();
    Ok(Source::Commit {
        id: oid.to_string(),
        committer_time: commit.time().seconds(),
        device,
    })
}

pub(super) fn device_name<'a>(commit: &'a git2::Commit<'_>) -> &'a str {
    commit
        .message()
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix("Bluink-Device-Name: "))
        .filter(|name| !name.is_empty() && name.len() <= 160 && !name.chars().any(char::is_control))
        .unwrap_or("another device")
}

pub(super) fn put(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    value: &Value,
) -> Result<()> {
    let id = value["id"]
        .as_str()
        .ok_or_else(|| failure("Item has no identity"))?;
    project::item::validate_item(value, id)?;
    let path = ItemId::try_from(id)?
        .path(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/");
    files.insert(path, repo.blob(&project::canonical_json::to_vec(value)?)?);
    Ok(())
}

pub(super) fn merge(
    repo: &Repository,
    (base, b): (Oid, &BTreeMap<String, Oid>),
    (left, l): (Oid, &BTreeMap<String, Oid>),
    (right, r): (Oid, &BTreeMap<String, Oid>),
) -> Result<BTreeMap<String, Oid>> {
    let mut files = l.clone();
    let config = project::config::parse_project_config(
        repo.find_blob(
            *l.get("bluink.json")
                .ok_or_else(|| failure("Missing configuration"))?,
        )?
        .content(),
    )?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| failure("Missing timezone"))?;
    let mut changed = BTreeSet::new();
    let mut proposals = Vec::new();
    let context = Context {
        target: config["id"]
            .as_str()
            .ok_or_else(|| failure("Missing project identity"))?
            .into(),
        base_epoch: base.to_string(),
        left: source(repo, left)?,
        right: source(repo, right)?,
    };
    merge_paths(
        repo,
        &context,
        [b, l, r],
        &mut files,
        timezone,
        &mut changed,
        &mut proposals,
    )?;
    let config = project::config::parse_project_config(
        repo.find_blob(
            *files
                .get("bluink.json")
                .ok_or_else(|| failure("Project configuration cannot be deleted"))?,
        )?
        .content(),
    )?;
    proposals.extend(super::theme_dependencies::repair(
        repo,
        &context,
        [b, l, r],
        &mut files,
    )?);
    let item_proposals = replay_files(repo, &mut files, proposals, timezone)?;
    repair(
        repo,
        &context,
        &config,
        b,
        l,
        r,
        &mut files,
        &changed,
        item_proposals,
    )?;
    Ok(files)
}

fn merge_paths(
    repo: &Repository,
    context: &Context,
    sources: [&BTreeMap<String, Oid>; 3],
    files: &mut BTreeMap<String, Oid>,
    timezone: &str,
    changed: &mut BTreeSet<String>,
    proposals: &mut Vec<Proposal>,
) -> Result<()> {
    let [b, l, r] = sources;
    let paths: BTreeSet<_> = b.keys().chain(l.keys()).chain(r.keys()).cloned().collect();
    for path in paths {
        let (bo, lo, ro) = (b.get(&path), l.get(&path), r.get(&path));
        if lo == ro && lo == bo {
            continue;
        }
        if path.starts_with("items/") {
            let id = Path::new(&path)
                .file_stem()
                .and_then(|x| x.to_str())
                .ok_or_else(|| failure("Invalid item path"))?;
            changed.insert(id.to_owned());
            // One side unchanged: keep the other side's exact blob. Deletions
            // still merge, because conflict records must survive as tombstones.
            let one_sided = if lo == ro || ro == bo {
                lo
            } else if lo == bo {
                ro
            } else {
                None
            };
            if let Some(oid) = one_sided {
                files.insert(path, *oid);
                continue;
            }
            let (bv, lv, rv) = (read(repo, b, id)?, read(repo, l, id)?, read(repo, r, id)?);
            let merged = merge::item::merge_item(
                &Context {
                    target: id.into(),
                    ..(*context).clone()
                },
                bv.as_ref(),
                lv.as_ref(),
                rv.as_ref(),
            )?;
            proposals.extend(
                merged
                    .decisions
                    .into_iter()
                    .map(|p| Proposal::Item(Box::new(p))),
            );
            proposals.extend(merged.followups);
            match crate::store::commands::with_authoring_values(|values| {
                merge::item::materialize(
                    merged.candidate,
                    lv.as_ref().or(rv.as_ref()),
                    timezone,
                    values,
                )
            })? {
                Some(value) => put(repo, files, &value)?,
                None => {
                    files.remove(&path);
                }
            }
        } else {
            let selected = if lo == ro || ro == bo {
                lo.copied()
            } else if lo == bo {
                ro.copied()
            } else {
                let (selected, proposal) = super::packages::merge(
                    repo,
                    context,
                    &path,
                    bo.copied(),
                    lo.copied(),
                    ro.copied(),
                )?;
                proposals.extend(proposal);
                selected
            };
            if let Some(oid) = selected {
                files.insert(path, oid);
            } else {
                files.remove(&path);
            }
        }
    }
    Ok(())
}

fn replay_files(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    proposals: Vec<Proposal>,
    timezone: &str,
) -> Result<Vec<Proposal>> {
    let mut item_proposals = Vec::new();
    for proposal in proposals {
        if proposal.file_decision().is_some() {
            let existing = read(repo, files, proposal.id())?;
            let replay = existing.is_some() || matches!(&proposal, Proposal::Resolution(_));
            let record = crate::store::commands::with_authoring_values(|values| {
                decisions::prepare(proposal, existing.as_ref(), timezone, values)
            })?;
            let stored = decisions::read(&record)?;
            if replay {
                super::decisions::apply_choice(
                    repo,
                    files,
                    stored
                        .proposal
                        .file_decision()
                        .ok_or_else(|| failure("Missing package decision"))?,
                    &stored.proposal.selected_file(
                        stored
                            .resolution
                            .as_ref()
                            .map_or(decisions::Selection::Provisional, |r| r.selection),
                    )?,
                )?;
            }
            if let Some(resolution) = &stored.resolution
                && stored.proposal.effective(resolution.selection)?.1 == decisions::Selection::Both
            {
                super::assets::keep_both(
                    repo,
                    files,
                    &stored.proposal,
                    resolution.selection,
                    timezone,
                )?;
            }
            if let Some(resolution) = &stored.resolution {
                super::decisions::apply_intents(
                    repo,
                    files,
                    &stored.proposal,
                    resolution,
                    timezone,
                )?;
            }
            put(repo, files, &record)?;
        } else {
            item_proposals.push(proposal);
        }
    }
    Ok(item_proposals)
}

#[expect(
    clippy::too_many_arguments,
    reason = "One immutable base/branch/candidate set defines the merge transaction."
)]
fn repair(
    repo: &Repository,
    context: &Context,
    config: &Value,
    base: &BTreeMap<String, Oid>,
    left: &BTreeMap<String, Oid>,
    right: &BTreeMap<String, Oid>,
    files: &mut BTreeMap<String, Oid>,
    changed: &BTreeSet<String>,
    mut proposals: Vec<Proposal>,
) -> Result<()> {
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| failure("Missing timezone"))?;
    let b = captured(repo, base)?;
    let l = captured(repo, left)?;
    let r = captured(repo, right)?;
    let plan = {
        let current = captured(repo, files)?;
        merge::groups::repair(
            context,
            config,
            changed,
            Snapshots {
                base: &b,
                left: &l,
                right: &r,
                candidate: &current,
            },
        )?
    };
    for (id, candidate) in plan.items {
        let previous = read(repo, files, &id)?;
        if let Some(value) = crate::store::commands::with_authoring_values(|values| {
            merge::item::materialize(candidate, previous.as_ref(), timezone, values)
        })? {
            put(repo, files, &value)?;
        }
    }
    proposals.extend(
        plan.groups
            .into_iter()
            .map(|p| Proposal::Group(Box::new(p))),
    );
    replay_items(repo, files, proposals, config, timezone)
}

fn replay_items(
    repo: &Repository,
    files: &mut BTreeMap<String, Oid>,
    proposals: Vec<Proposal>,
    config: &Value,
    timezone: &str,
) -> Result<()> {
    let mut deleted = BTreeSet::new();
    let replacements = {
        let current = captured(repo, files)?;
        let mut overlay = merge::groups::Overlay::new(&current);
        let mut replacements = BTreeMap::new();
        for proposal in proposals {
            let existing = overlay.read(proposal.id())?;
            let replay = existing.is_some() || matches!(&proposal, Proposal::Resolution(_));
            let decision = crate::store::commands::with_authoring_values(|values| {
                decisions::prepare(proposal, existing.as_ref(), timezone, values)
            })?;
            // Replay only matching regenerated proposals, never every inherited
            // tombstone: later authored edits must remain untouched.
            if replay {
                let record = decisions::read(&decision)?;
                let selection = record
                    .resolution
                    .as_ref()
                    .map_or(decisions::Selection::Provisional, |r| r.selection);
                let whole = record.proposal.selected_item(selection)?;
                let mut updates = if let Some((item, merge::model::Fragment::Missing)) = whole
                    && item.location.0.is_empty()
                {
                    overlay.remove(&item.target)?;
                    deleted.insert(item.target.clone());
                    BTreeMap::new()
                } else {
                    crate::store::commands::with_authoring_values(|values| {
                        decisions::replay(&decision, &overlay, config, values)
                    })?
                };
                if let Some(resolution) = &record.resolution {
                    decisions::resolve::apply_intents(
                        &record.proposal,
                        resolution,
                        &overlay,
                        &mut updates,
                    )
                    .map_err(|e| failure(format!("Invalid resolution intent: {e:?}")))?;
                }
                for value in updates.into_values() {
                    overlay.put(value.clone())?;
                    replacements.insert(
                        value["id"]
                            .as_str()
                            .ok_or_else(|| failure("Missing target identity"))?
                            .to_owned(),
                        value,
                    );
                }
            }
            overlay.put(decision.clone())?;
            replacements.insert(
                decision["id"]
                    .as_str()
                    .ok_or_else(|| failure("Missing decision identity"))?
                    .to_owned(),
                decision,
            );
        }
        replacements
    };
    for id in deleted {
        files.remove(
            &ItemId::try_from(id.as_str())?
                .path(Path::new(""))
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    for value in replacements.into_values() {
        let id = value["id"]
            .as_str()
            .ok_or_else(|| failure("Missing replacement identity"))?;
        let previous = read(repo, files, id)?;
        let value = if previous.as_ref().is_some_and(|previous| previous != &value) {
            crate::store::commands::with_authoring_values(|values| {
                merge::item::materialize(
                    merge::item::ItemCandidate::Changed {
                        fields: merge::item::semantic(&value)?,
                        created_at: value["createdAt"]
                            .as_str()
                            .ok_or("Missing creation time")?
                            .into(),
                    },
                    previous.as_ref(),
                    timezone,
                    values,
                )
            })?
            .ok_or_else(|| failure("Missing replacement"))?
        } else {
            value
        };
        put(repo, files, &value)?;
    }
    Ok(())
}
