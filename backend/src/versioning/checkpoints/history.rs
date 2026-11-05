// SPDX-License-Identifier: MPL-2.0
//! Ingest immutable history once; page reads use the covered SQL projection.
use git2::{Oid, Repository};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{database, open};
use crate::versioning::{
    Result, failure, history,
    model::{HistoryEntry, HistoryPage, HistoryRequest},
    repository,
};
use crate::{
    domain::project::identity::ItemId,
    store::{
        Store,
        git::history::{self as projection, Commit, Name, Order, Projection},
    },
};

const PREFIX: &str = "refs/tags/bluink/versions/";

#[cfg(test)]
mod benchmark;

fn owner(path: &Path) -> Result<Option<String>> {
    let value = path
        .to_str()
        .ok_or_else(|| failure("History path is not UTF-8"))?
        .replace('\\', "/");
    super::validate_path(&value)?;
    let parts: Vec<_> = value.split('/').collect();
    let id = match parts.as_slice() {
        ["items", _, filename] => filename.strip_suffix(".json"),
        ["assets", _, id, _] => Some(*id),
        _ => None,
    };
    id.map(|id| {
        ItemId::try_from(id)
            .map(|id| id.as_str().to_owned())
            .map_err(failure)
    })
    .transpose()
}

fn commit(repo: &Repository, oid: Oid) -> Result<Commit> {
    let commit = repo.find_commit(oid)?;
    let previous = if commit.parent_count() == 0 {
        None
    } else {
        Some(commit.parent(0)?.tree()?)
    };
    let current = commit.tree()?;
    let changes = repo.diff_tree_to_tree(previous.as_ref(), Some(&current), None)?;
    let mut items = BTreeSet::new();
    for delta in changes.deltas() {
        for path in [delta.old_file().path(), delta.new_file().path()]
            .into_iter()
            .flatten()
        {
            if let Some(id) = owner(path)? {
                items.insert(id);
            }
        }
    }
    Ok(Commit {
        oid: oid.to_string(),
        seconds: commit.time().seconds(),
        message: commit.summary()?.unwrap_or("Checkpoint").to_owned(),
        items: items.into_iter().collect(),
    })
}

/// The caller holds the same Git lease used for publication. Only a newly
/// reachable graph needs a revwalk; ordinary single-parent saves prepend one row.
pub(super) fn prepare(repo: &Repository, store: &Store) -> Result<Option<Projection>> {
    let head = repository::head(repo)?;
    let references = repo
        .references_glob(&format!("{PREFIX}*"))?
        .map(|reference| {
            let reference = reference?;
            Ok::<_, crate::versioning::Error>((
                reference.name()?.to_owned(),
                reference
                    .target()
                    .ok_or_else(|| failure("Named version must be a direct ref"))?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut hash = Sha256::new();
    for (name, oid) in &references {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(oid.as_bytes());
    }
    let digest = crate::versioning::lfs::digest_hex(&hash.finalize());
    let coverage = database(store.read(projection::coverage))?;
    let head_text = head.map(|oid| oid.to_string());
    if coverage
        .as_ref()
        .is_some_and(|coverage| coverage.0 == head_text && coverage.1 == digest)
    {
        return Ok(None);
    }
    let (project, cached) = database(store.read(|tx| {
        let project =
            tx.query_row::<String, _, _>("SELECT project_uuid FROM project_state", [], |row| {
                row.get(0)
            })?;
        let mut cached = Vec::with_capacity(references.len());
        for (reference, oid) in &references {
            cached.push(projection::cached_name(tx, reference, &oid.to_string())?);
        }
        Ok((project, cached))
    }))?;
    let mut names = Vec::with_capacity(references.len());
    for ((reference, oid), cached) in references.into_iter().zip(cached) {
        names.push(if let Some(cached) = cached {
            cached
        } else {
            Name {
                value: history::named_tag(repo, &reference, &repo.find_tag(oid)?, Some(&project))?,
                reference,
                tag: oid.to_string(),
            }
        });
    }
    let mut commits = BTreeMap::new();
    let targets: Vec<_> = names
        .iter()
        .map(|name| name.value.commit_id.clone())
        .collect();
    for oid in database(store.read(|tx| projection::unknown(tx, &targets)))? {
        let oid = Oid::from_str(oid)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = commits.entry(oid) {
            entry.insert(commit(repo, oid)?);
        }
    }
    let order = if coverage
        .as_ref()
        .is_some_and(|coverage| coverage.0 == head_text)
    {
        Order::Unchanged
    } else if let (Some(head), Some((Some(previous), _))) = (head, coverage.as_ref()) {
        let current = repo.find_commit(head)?;
        if current.parent_count() == 1 && current.parent_id(0)?.to_string() == *previous {
            commits.insert(head, commit(repo, head)?);
            Order::Prepend(head.to_string())
        } else {
            backfill(repo, store, Some(head), &mut commits)?
        }
    } else {
        backfill(repo, store, head, &mut commits)?
    };
    Ok(Some(Projection {
        head: head_text,
        refs_digest: digest,
        commits: commits.into_values().collect(),
        names,
        order,
    }))
}

fn backfill(
    repo: &Repository,
    store: &Store,
    head: Option<Oid>,
    commits: &mut BTreeMap<Oid, Commit>,
) -> Result<Order> {
    let mut order = Vec::new();
    if let Some(head) = head {
        let mut walk = repo.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        walk.push(head)?;
        for oid in walk {
            order.push(oid?.to_string());
        }
    }
    // One snapshot identifies every commit still to ingest.
    for oid in database(store.read(|tx| projection::unknown(tx, &order)))? {
        let oid = Oid::from_str(oid)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = commits.entry(oid) {
            entry.insert(commit(repo, oid)?);
        }
    }
    Ok(Order::Replace(order))
}

pub(crate) fn read(root: &Path, store: &Store, request: &HistoryRequest) -> Result<HistoryPage> {
    let repo = open(root)?;
    let (rows, more) = database(store.read(|tx| {
        let head = crate::store::git::expected_head(tx)?;
        projection::page(tx, head.as_deref(), request)
    }))?;
    let mut attachments = history::AttachmentPresence::default();
    let mut entries = Vec::new();
    for row in rows {
        let commit = repo.find_commit(Oid::from_str(&row.oid)?)?;
        let created_at = chrono::DateTime::from_timestamp(row.seconds, 0)
            .ok_or_else(|| failure("Invalid checkpoint time"))?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        entries.push(HistoryEntry {
            id: row.oid,
            message: row.message,
            created_at,
            named_versions: row.names,
            attachments_available: history::attachments_available(
                &repo,
                &commit.tree()?,
                request.item_id.as_deref(),
                &mut attachments,
            )?,
        });
    }
    let cursor = if more {
        entries.last().map(|entry| entry.id.clone())
    } else {
        None
    };
    Ok(HistoryPage { entries, cursor })
}
