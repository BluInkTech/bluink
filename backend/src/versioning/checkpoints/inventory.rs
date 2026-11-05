// SPDX-License-Identifier: MPL-2.0
//! Fill missing commit deltas parent-first. Covered histories query SQLite;
//! current-tree checks reuse validated portable subtrees by Git object identity.
use super::database;
use crate::{
    store::{
        Store,
        git::inventory::{self as cache, Commit, Pointers},
    },
    versioning::{Result, failure, lfs::Pointer, repository, transport},
};
use git2::{Oid, Repository};
use std::collections::BTreeSet;

#[derive(Default, Debug)]
pub(in crate::versioning) struct Work {
    pub(in crate::versioning) commit_diffs: usize,
    pub(in crate::versioning) pointer_blobs: usize,
    pub(in crate::versioning) shard_fills: usize,
}

pub(super) fn add(pointers: &mut Pointers, pointer: &Pointer) -> Result<()> {
    let (oid, size) = pointer.identity();
    if pointers
        .insert(oid.to_owned(), size)
        .is_some_and(|existing| existing != size)
    {
        return Err(failure(
            "Git history declares inconsistent attachment sizes",
        ));
    }
    Ok(())
}
fn pointers(values: Pointers) -> Result<Vec<Pointer>> {
    values
        .into_iter()
        .map(|(oid, size)| Pointer::from_identity(&oid, size))
        .collect()
}
fn commit_ids(repo: &Repository, oids: &[Oid]) -> Result<Vec<String>> {
    oids.iter()
        .map(|oid| {
            Ok(repo
                .find_object(*oid, None)?
                .peel_to_commit()?
                .id()
                .to_string())
        })
        .collect()
}

fn changed(repo: &Repository, oid: Oid, work: &mut Work) -> Result<Commit> {
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    let parents: Vec<_> = commit.parent_ids().map(|id| id.to_string()).collect();
    let previous = if parents.is_empty() {
        None
    } else {
        Some(commit.parent(0)?.tree()?)
    };
    let diff = repo.diff_tree_to_tree(previous.as_ref(), Some(&tree), None)?;
    let mut found = Pointers::new();
    work.commit_diffs += 1;
    for delta in diff.deltas() {
        let file = delta.new_file();
        if file.id().is_zero() {
            continue;
        }
        let path = file
            .path()
            .and_then(|path| path.to_str())
            .ok_or_else(|| failure("Invalid Git inventory path"))?;
        let path = path.replace('\\', "/");
        let kind =
            repository::tracked(&path).ok_or_else(|| failure("Invalid portable inventory path"))?;
        if file.mode() != git2::FileMode::Blob {
            return Err(failure("An inventory path is not a regular file"));
        }
        if kind == repository::Kind::Binary || path.starts_with("items/") {
            work.pointer_blobs += 1;
            for pointer in transport::blob_pointers(repo, &path, file.id())? {
                add(&mut found, &pointer)?;
            }
        }
    }
    Ok(Commit {
        id: oid.to_string(),
        parents,
        pointers: found,
    })
}

/// An installed commit has a complete delta even if a not-yet-imported parent
/// prevents transitive coverage. Closing that gap never rereads its file blobs.
/// One snapshot plans the parent-first order; installs commit in bounded batches.
fn cover(repo: &Repository, store: &Store, heads: &[String], work: &mut Work) -> Result<()> {
    enum Visit {
        Enter(String),
        Finish(String, bool),
    }
    enum Install {
        Delta(Commit),
        Parents(String),
    }
    let parents = |id: &str| -> Result<Vec<String>> {
        Ok(repo
            .find_commit(Oid::from_str(id)?)?
            .parent_ids()
            .map(|id| id.to_string())
            .collect())
    };
    // Each entry is a commit and whether its delta is already installed.
    let plan = database(store.read(|tx| {
        let mut visits = heads.iter().cloned().map(Visit::Enter).collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut plan = Vec::new();
        while let Some(visit) = visits.pop() {
            match visit {
                Visit::Enter(id) => {
                    if !visited.insert(id.clone()) {
                        continue;
                    }
                    let (parents, cached_delta) = match cache::status(tx, &id)? {
                        Some((true, _)) => continue,
                        Some((false, parents)) => (parents, true),
                        None => (
                            parents(&id)
                                .map_err(|e| crate::store::Error::Invalid(e.to_string()))?,
                            false,
                        ),
                    };
                    visits.push(Visit::Finish(id, cached_delta));
                    visits.extend(parents.into_iter().map(Visit::Enter));
                }
                Visit::Finish(id, cached_delta) => plan.push((id, cached_delta)),
            }
        }
        Ok(plan)
    }))?;
    for batch in plan.chunks(256) {
        let mut installs = Vec::with_capacity(batch.len());
        for (id, cached_delta) in batch {
            installs.push(if *cached_delta {
                Install::Parents(id.clone())
            } else {
                Install::Delta(changed(repo, Oid::from_str(id)?, work)?)
            });
        }
        database(store.write(move |tx| {
            for install in installs {
                match install {
                    Install::Delta(commit) => cache::install(tx, &commit)?,
                    Install::Parents(id) => cache::cover_parents(tx, &id)?,
                }
            }
            Ok(())
        }))?;
    }
    Ok(())
}

pub(in crate::versioning) fn history(
    repo: &Repository,
    store: &Store,
    heads: &[Oid],
    known: &[Oid],
    work: &mut Work,
) -> Result<Vec<Pointer>> {
    let heads = commit_ids(repo, heads)?;
    let known = commit_ids(repo, known)?;
    let read = |tx: &rusqlite::Transaction<'_>| cache::reachable(tx, &heads, &known);
    if let Some(found) = database(store.read(read))? {
        return pointers(found);
    }
    cover(repo, store, &heads, work)?;
    cover(repo, store, &known, work)?;
    pointers(
        database(store.read(read))?
            .ok_or_else(|| failure("Attachment inventory coverage is incomplete"))?,
    )
}

pub(in crate::versioning) fn from_snapshot(
    repo: &Repository,
    db: &rusqlite::Connection,
    heads: &[Oid],
) -> Result<Option<Vec<Pointer>>> {
    database(cache::reachable(db, &commit_ids(repo, heads)?, &[]))?
        .map(pointers)
        .transpose()
}

/// Fill the durable inventory before copying a live database. Restoring the
/// closed backup can then recover its precise pointer set from its own rows.
pub(crate) fn cover_backup(root: &std::path::Path, store: &Store) -> Result<()> {
    use crate::store::io::{self, LockMode};
    let repo = super::open(root)?;
    let _lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| failure(format!("{error:#}")))?;
    let mut heads = Vec::new();
    for reference in repo.references()? {
        let reference = reference?;
        if let Ok(commit) = reference.peel_to_commit() {
            heads.push(commit.id().to_string());
        }
    }
    let pins = database(store.read(|tx| {
        let mut query = tx.prepare("SELECT object_id FROM object_pin WHERE object_kind='git'")?;
        Ok(query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }))?;
    for pin in pins {
        let object = repo.find_object(Oid::from_str(&pin)?, None)?;
        if let Ok(commit) = object.peel_to_commit() {
            heads.push(commit.id().to_string());
        }
    }
    cover(&repo, store, &heads, &mut Work::default())
}

pub(in crate::versioning) fn current(
    repo: &Repository,
    store: &Store,
    heads: &[Oid],
    work: &mut Work,
) -> Result<Vec<Pointer>> {
    let mut found = Pointers::new();
    for id in commit_ids(repo, heads)? {
        let tree = repo.find_commit(Oid::from_str(&id)?)?.tree()?;
        for top in ["items", "assets", "themes"] {
            let Some(entry) = tree.get_name(top) else {
                continue;
            };
            for shard in &repo.find_tree(entry.id())? {
                let oid = shard.id();
                let prefix = format!("{top}/{}/", shard.name()?);
                let key = oid.to_string();
                let cached = database(store.read(|tx| cache::tree(tx, &key, &prefix)))?;
                let values = if let Some(cached) = cached {
                    cached
                } else {
                    work.shard_fills += 1;
                    let mut values = Pointers::new();
                    for pointer in transport::shard_pointers(repo, &prefix, oid)? {
                        add(&mut values, &pointer)?;
                    }
                    let stored = values.clone();
                    database(
                        store.write(move |tx| cache::install_tree(tx, &key, &prefix, &stored)),
                    )?;
                    values
                };
                for pointer in pointers(values)? {
                    add(&mut found, &pointer)?;
                }
            }
        }
    }
    pointers(found)
}
