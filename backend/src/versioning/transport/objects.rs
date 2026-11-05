// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::{
    Result, failure,
    lfs::{self, Pointer},
    repository,
};
use super::{Auth, Remote};
use git2::{Oid, Repository};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    sync::{Arc, LazyLock, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Upload,
    Download,
}

impl Direction {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

fn pointers(repo: &Repository, heads: &[Oid], history: bool) -> Result<Vec<Pointer>> {
    inventory(repo, heads, history, &[])
}

pub(in crate::versioning) fn local_inventory(
    repo: &Repository,
    heads: &[Oid],
) -> Result<Vec<Pointer>> {
    inventory(repo, heads, true, &[])
}

pub(super) fn inventory(
    repo: &Repository,
    heads: &[Oid],
    history: bool,
    known: &[Oid],
) -> Result<Vec<Pointer>> {
    let commit_id = |oid| Ok::<_, git2::Error>(repo.find_object(oid, None)?.peel_to_commit()?.id());
    let mut inventory = Inventory::default();
    if history {
        let mut walk = repo.revwalk()?;
        for head in heads {
            walk.push(commit_id(*head)?)?;
        }
        for head in known {
            walk.hide(commit_id(*head)?)?;
        }
        for commit in walk {
            inventory.tree(repo, repo.find_commit(commit?)?.tree_id(), "", 0)?;
        }
    } else {
        for head in heads {
            inventory.tree(repo, repo.find_commit(commit_id(*head)?)?.tree_id(), "", 0)?;
        }
    }
    Ok(inventory.pointers.into_values().collect())
}

#[derive(Default)]
struct Inventory {
    // Reused Merkle subtrees need no traversal on later commits. Prefix remains
    // part of the key because the tracked-path policy is location-dependent.
    trees: BTreeSet<(String, Oid)>,
    blobs: BTreeSet<(bool, Oid)>,
    pointers: BTreeMap<String, Pointer>,
}

// Git identifies a subtree by the immutable hash of its complete contents.
// Most item buckets contain no attachment decisions at all: reuse that proof
// between requests instead of opening and parsing a large book on every sync.
type Shards = BTreeMap<(String, Oid), Arc<[Pointer]>>;
static ITEM_SHARDS: LazyLock<Mutex<Shards>> = LazyLock::new(Mutex::default);

fn item_shard(repo: &Repository, oid: Oid, prefix: &str, depth: usize) -> Result<Arc<[Pointer]>> {
    let key = (prefix.to_owned(), oid);
    let cache = || {
        ITEM_SHARDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let cached = cache().get(&key).cloned();
    if let Some(found) = cached {
        return Ok(found);
    }
    let mut shard = Inventory::default();
    shard.entries(repo, oid, prefix, depth)?;
    let pointers: Arc<[Pointer]> = shard.pointers.into_values().collect();
    if pointers.len() <= 32 {
        let mut shards = cache();
        if shards.len() >= 2048 {
            shards.clear();
        }
        shards.insert(key, Arc::clone(&pointers));
    }
    Ok(pointers)
}

impl Inventory {
    fn insert(&mut self, pointer: Pointer) -> Result<()> {
        let (id, size) = pointer.fingerprint();
        if self
            .pointers
            .get(&id)
            .is_some_and(|old| old.fingerprint().1 != size)
        {
            return Err(failure(
                "Attachment history declares inconsistent object sizes",
            ));
        }
        self.pointers.insert(id, pointer);
        Ok(())
    }

    fn tree(&mut self, repo: &Repository, oid: Oid, prefix: &str, depth: usize) -> Result<()> {
        if depth > 16 {
            return Err(failure("Attachment history tree is too deeply nested"));
        }
        if !self.trees.insert((prefix.to_owned(), oid)) {
            return Ok(());
        }
        if depth == 2 && prefix.starts_with("items/") {
            for pointer in item_shard(repo, oid, prefix, depth)?.iter() {
                self.insert(pointer.clone())?;
            }
            return Ok(());
        }
        self.entries(repo, oid, prefix, depth)
    }

    fn entries(&mut self, repo: &Repository, oid: Oid, prefix: &str, depth: usize) -> Result<()> {
        for entry in &repo.find_tree(oid)? {
            let path = format!("{prefix}{}", entry.name()?);
            if entry.kind() == Some(git2::ObjectType::Tree) {
                self.tree(repo, entry.id(), &format!("{path}/"), depth + 1)?;
                continue;
            }
            let kind = repository::tracked(&path)
                .ok_or_else(|| failure("Attachment history contains an unsupported path"))?;
            if entry.filemode() != 0o100_644 {
                return Err(failure("Attachment history contains a non-regular file"));
            }
            if kind == repository::Kind::Binary && self.blobs.insert((true, entry.id())) {
                self.insert(Pointer::parse(repo.find_blob(entry.id())?.content())?)?;
            } else if path.starts_with("items/") && self.blobs.insert((false, entry.id())) {
                self.decision(repo.find_blob(entry.id())?.content())?;
            }
        }
        Ok(())
    }

    fn decision(&mut self, bytes: &[u8]) -> Result<()> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            meta: Metadata,
        }
        #[derive(serde::Deserialize)]
        struct Metadata {
            #[serde(rename = "type")]
            kind: String,
        }
        if serde_json::from_slice::<Envelope>(bytes)?.meta.kind != "conflict" {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        self.decision_value(&value)
    }

    fn decision_value(&mut self, value: &serde_json::Value) -> Result<()> {
        use crate::domain::merge::{decisions, model::Fragment};
        let record = decisions::read(value)?;
        if let Some(file) = record.proposal.file_decision()
            && repository::tracked(&file.path) == Some(repository::Kind::Binary)
        {
            for fragment in [
                &file.base,
                &file.alternatives[0].value,
                &file.alternatives[1].value,
            ] {
                if let Fragment::Present(value) = fragment {
                    let text = value.as_str().ok_or_else(|| {
                        failure("Attachment decision has invalid pointer content")
                    })?;
                    self.insert(Pointer::parse(text.as_bytes())?)?;
                }
            }
        }
        Ok(())
    }
}

pub(in crate::versioning) fn decision_pointers(value: &serde_json::Value) -> Result<Vec<Pointer>> {
    if value["meta"]["type"] != "conflict" {
        return Ok(Vec::new());
    }
    let mut inventory = Inventory::default();
    inventory.decision_value(value)?;
    Ok(inventory.pointers.into_values().collect())
}

pub(in crate::versioning) fn blob_pointers(
    repo: &Repository,
    path: &str,
    oid: Oid,
) -> Result<Vec<Pointer>> {
    let mut inventory = Inventory::default();
    let kind = repository::tracked(path).ok_or_else(|| failure("Unsupported inventory path"))?;
    if kind == repository::Kind::Binary {
        inventory.insert(Pointer::parse(repo.find_blob(oid)?.content())?)?;
    } else if path.starts_with("items/") {
        inventory.decision(repo.find_blob(oid)?.content())?;
    }
    Ok(inventory.pointers.into_values().collect())
}

pub(in crate::versioning) fn shard_pointers(
    repo: &Repository,
    prefix: &str,
    oid: Oid,
) -> Result<Vec<Pointer>> {
    let mut inventory = Inventory::default();
    inventory.entries(repo, oid, prefix, 2)?;
    Ok(inventory.pointers.into_values().collect())
}

pub(in crate::versioning) fn usage_for_pointers(
    repo: &Repository,
    pointers: &[Pointer],
) -> Result<(u64, u64)> {
    let mut reachable = 0_u64;
    let mut missing = 0_u64;
    for pointer in pointers {
        let size = pointer.fingerprint().1;
        reachable = reachable
            .checked_add(size)
            .ok_or_else(|| failure("Attachment size overflow"))?;
        if pointer.available_path(repo.path())?.is_none() {
            missing += size;
        }
    }
    Ok((reachable, missing))
}

pub(crate) fn transfer_lfs(
    repo: &Repository,
    remote: &Remote,
    auth: &Auth,
    commits: &[Oid],
    direction: Direction,
    history: bool,
) -> Result<()> {
    auth.check_cancelled()?;
    let pointers = pointers(repo, commits, history)?;
    transfer_pointers(repo, remote, auth, &pointers, direction)
}

pub(in crate::versioning) fn transfer_pointers(
    repo: &Repository,
    remote: &Remote,
    auth: &Auth,
    pointers: &[Pointer],
    direction: Direction,
) -> Result<()> {
    auth.check_cancelled()?;
    let mut needed = Vec::new();
    for pointer in pointers {
        auth.check_cancelled()?;
        // Upload adapters first ask the actual destination whether bytes are
        // required. A lightweight clone can reference an object already there.
        if direction == Direction::Upload || pointer.available_path(repo.path())?.is_none() {
            needed.push(pointer.clone());
        }
    }
    if needed.is_empty() {
        return Ok(());
    }
    match remote {
        Remote::Local(_) => {
            let destination = remote.local_repository()?;
            let (from, to) = if direction == Direction::Upload {
                (repo.path(), destination.path())
            } else {
                (destination.path(), repo.path())
            };
            for pointer in needed {
                auth.check_cancelled()?;
                if pointer.available_path(to)?.is_some() {
                    pointer.verified_path(to)?;
                    continue;
                }
                // Receiving hashes the bytes as it copies them.
                lfs::receive(to, &pointer, &mut File::open(pointer.stored_path(from)?)?)?;
            }
            Ok(())
        }
        Remote::Https(url) => super::http::transfer(repo, url, auth, &needed, direction),
        Remote::Ssh(_) => super::ssh::transfer(repo, remote, auth, &needed, direction),
    }
}
