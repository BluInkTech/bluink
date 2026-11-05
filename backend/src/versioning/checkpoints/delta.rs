// SPDX-License-Identifier: MPL-2.0
//! An incoming immutable tree is accepted with its complete SQL change and ref intent.
use crate::{
    store::{self, git, mutations::Delete, portable::PreparedDelta},
    versioning::{Result, failure, lfs, repository},
};
use git2::{Oid, Repository};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Seek, Write};
use std::sync::{Arc, atomic::AtomicBool};

#[cfg(test)]
mod benchmark;

pub(in crate::versioning) struct Publication {
    pub(in crate::versioning) previous: Oid,
    pub(in crate::versioning) candidate: Oid,
    pub(in crate::versioning) generation: i64,
    pub(in crate::versioning) tags: BTreeMap<String, Oid>,
    pub(in crate::versioning) cancelled: Arc<AtomicBool>,
}

pub(in crate::versioning) struct Prepared {
    delta: PreparedDelta,
    pub(in crate::versioning) changed_paths: usize,
    encoded_bytes: usize,
}

impl Prepared {
    pub(in crate::versioning) const fn needs_maintenance(&self) -> bool {
        self.changed_paths > 512 || self.encoded_bytes > 16 * 1024 * 1024
    }
}

fn json(repo: &Repository, oid: Option<Oid>) -> Result<Option<Value>> {
    oid.map(|oid| Ok(serde_json::from_slice(repo.find_blob(oid)?.content())?))
        .transpose()
}

pub(in crate::versioning) fn prepare(
    root: &std::path::Path,
    repo: &Repository,
    previous: Oid,
    candidate: Oid,
) -> Result<Prepared> {
    let old = repo.find_commit(previous)?.tree()?;
    let new = repo.find_commit(candidate)?.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&old), Some(&new), None)?;
    let changed_paths = diff.deltas().len();
    let odb = repo.odb()?;
    let mut encoded_bytes = 0usize;
    let mut items = Vec::new();
    let mut deletes = Vec::new();
    let mut records = Vec::new();
    let mut assets = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(|path| path.to_str())
            .ok_or_else(|| failure("Invalid incoming path"))?
            .replace('\\', "/");
        super::validate_path(&path)?;
        if matches!(path.as_str(), ".gitignore" | ".gitattributes") {
            continue;
        }
        let before = old
            .get_path(std::path::Path::new(&path))
            .ok()
            .map(|entry| entry.id());
        let after = new
            .get_path(std::path::Path::new(&path))
            .ok()
            .map(|entry| entry.id());
        if let Some(oid) = after {
            encoded_bytes = encoded_bytes.saturating_add(odb.read_header(oid)?.0);
        }
        if path.starts_with("items/") {
            let previous = json(repo, before)?;
            let expected = previous
                .as_ref()
                .map(|value| {
                    value["revision"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| failure("Incoming base has no revision"))
                })
                .transpose()?;
            if let Some(value) = json(repo, after)? {
                items.push((value, expected));
            } else if let (Some(value), Some(expected)) = (previous, expected) {
                deletes.push(Delete {
                    id: value["id"]
                        .as_str()
                        .ok_or_else(|| failure("Deleted item has no identity"))?
                        .to_owned(),
                    expected,
                });
            }
        } else if super::validate_record_path(&path).is_ok() {
            records.push((path, json(repo, after)?));
        } else {
            let asset = after
                .map(|oid| -> Result<_> {
                    let blob = repo.find_blob(oid)?;
                    if repository::tracked(&path) == Some(repository::Kind::Binary) {
                        let pointer = lfs::Pointer::parse(blob.content())?;
                        let (oid, size) = pointer.identity();
                        super::verify_asset(root, oid, size)
                    } else {
                        let mut file = tempfile::tempfile()?;
                        file.write_all(blob.content())?;
                        file.rewind()?;
                        super::install_asset(root, &mut file)
                    }
                })
                .transpose()?;
            assets.push((path, asset));
        }
    }
    store::theme_packages::validate_delta(root, &records, &assets)
        .map_err(|error| failure(error.to_string()))?;
    let delta = PreparedDelta::new(items, deletes, records, assets)
        .map_err(|error| failure(error.to_string()))?;
    Ok(Prepared {
        delta,
        changed_paths,
        encoded_bytes,
    })
}

/// Caller holds the Git lease. SQLite acknowledges the complete canonical
/// change before CAS; startup can finish this exact ref intent after a crash.
pub(in crate::versioning) fn apply(
    repo: &Repository,
    store: &store::Store,
    prepared: Prepared,
    publication: Publication,
) -> Result<store::WriteTiming> {
    let Publication {
        previous,
        candidate,
        generation,
        tags,
        cancelled,
    } = publication;
    super::recover(repo, store)?;
    let head = previous.to_string();
    let next = candidate.to_string();
    let maintenance = prepared.needs_maintenance();
    let tags: BTreeMap<_, _> = tags
        .into_iter()
        .map(|(name, oid)| (name, oid.to_string()))
        .collect();
    let install = move |tx: &rusqlite::Transaction<'_>| {
        git::begin_transition(tx, &head)?;
        prepared.delta.apply(tx, generation)?;
        git::prepare_transition(tx, &head, &next, &tags)?;
        Ok(())
    };
    let ((), timing) = store
        .timed_write(maintenance.then_some(cancelled), install)
        .map_err(|error| {
            let changed = matches!(error, store::Error::RevisionConflict);
            let error = failure(error.to_string());
            // A save after preparation: the caller can checkpoint and merge again.
            if changed {
                error.classified("local_changed")
            } else {
                error
            }
        })?;
    super::recover(repo, store)?;
    Ok(timing)
}
