// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{BYTE_LIMIT, Result, failure, persist, persist_pack};
use git2::{Oid, Repository};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const LOOSE_THRESHOLD: usize = 1_024;
const PASS_LIMIT: usize = 512;
/// Every checkpoint and fetch adds a pack; lookups probe each pack's index.
const PACK_THRESHOLD: usize = 16;
/// Packs are merged in memory, so one pass merges at most this many pack bytes.
/// Larger packs stay as they are and later small packs merge beside them.
const MERGE_BYTES: u64 = 64 * 1024 * 1024;

/// Call with the same process/worker exclusion used for checkpoints. This only
/// replaces verified loose objects with packed copies: no reachability pruning,
/// reflog expiry, LFS traversal, or reference updates occur.
pub(in crate::versioning) fn maintain(
    repo: &Repository,
    cancelled: impl Fn() -> bool,
) -> Result<usize> {
    let objects = repo.path().join("objects");
    let Some(candidates) = candidates(repo, &objects, &cancelled)? else {
        return Ok(0);
    };
    let ids: Vec<_> = candidates.iter().map(|(id, _)| *id).collect();
    if !persist(repo, &ids, &cancelled)? {
        return Ok(0);
    }
    let mut removed = 0;
    for (_, path) in candidates {
        if cancelled() {
            break;
        }
        remove_loose(&path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            super::sync_directory(parent)?;
        }
        removed += 1;
    }
    repo.odb()?.refresh()?;
    Ok(removed)
}

fn candidates(
    repo: &Repository,
    directory: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Vec<(Oid, PathBuf)>>> {
    let mut count = 0;
    let mut bytes = 0_usize;
    let mut selected = Vec::new();
    let mut full = false;
    let odb = repo.odb()?;
    for bucket in fs::read_dir(directory)? {
        if cancelled() {
            return Ok(None);
        }
        let bucket = bucket?;
        let name = bucket.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !hexadecimal(name, 2) || !bucket.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(bucket.path())? {
            if cancelled() {
                return Ok(None);
            }
            let entry = entry?;
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            if !hexadecimal(filename, 38) || !entry.file_type()?.is_file() {
                continue;
            }
            count += 1;
            if !full {
                let id = Oid::from_str(&format!("{name}{filename}"))?;
                let (size, _) = odb.read_header(id)?;
                if selected.is_empty() || bytes.saturating_add(size) <= BYTE_LIMIT {
                    selected.push((id, entry.path()));
                    bytes = bytes.saturating_add(size);
                } else {
                    full = true;
                }
                full |= selected.len() >= PASS_LIMIT || bytes >= BYTE_LIMIT;
            }
            // More entries cannot change the threshold decision or this pass.
            if count > LOOSE_THRESHOLD {
                return Ok(Some(selected));
            }
        }
    }
    Ok(None)
}

/// Call with the same exclusion as `maintain`, and with no other handle on this
/// repository open in the process: Windows cannot delete a mapped pack. Merges
/// the smallest packs into one verified pack, then removes them. Every object
/// they hold is kept, reachable or not, so pending operations, captures and
/// backups never lose one. Returns the number of packs removed.
pub(in crate::versioning) fn consolidate(
    repository: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<usize> {
    let directory = repository.join("objects/pack");
    let mut packs = packs(&directory)?;
    if packs.len() < PACK_THRESHOLD {
        return Ok(0);
    }
    packs.sort();
    let mut selected = Vec::new();
    let mut bytes = 0_u64;
    for (size, id) in packs {
        if bytes.saturating_add(size) > MERGE_BYTES {
            break;
        }
        bytes += size;
        selected.push(id);
    }
    if selected.len() < 2 {
        return Ok(0);
    }
    let mut objects = BTreeSet::new();
    for id in &selected {
        if cancelled() {
            return Ok(0);
        }
        objects.extend(indexed(&directory.join(format!("pack-{id}.idx")))?);
    }
    let objects: Vec<_> = objects.into_iter().collect();
    let merged = {
        let repo = Repository::open_bare(repository)?;
        persist_pack(&repo, &objects, &cancelled)?
    };
    let Some(merged) = merged else {
        return Ok(0);
    };
    let mut removed = 0;
    for id in selected.iter().filter(|id| **id != merged) {
        // Without its index a pack is invisible, so the index goes first.
        let result: Result<()> = ["idx", "pack", "rev"].iter().try_for_each(|extension| {
            let path = directory.join(format!("pack-{id}.{extension}"));
            if path.try_exists()? {
                remove_object_file(&path)?;
            }
            Ok(())
        });
        match result {
            Ok(()) => removed += 1,
            // A reader elsewhere still maps it. Its objects are also in the
            // merged pack, so it is only a duplicate until a later pass.
            Err(error) => tracing::debug!(operation = "history.maintenance", pack = %id,
                error = %error, "A merged pack is still open; it will be removed later"),
        }
    }
    #[cfg(unix)]
    super::sync_directory(&directory)?;
    Ok(removed)
}

/// Packs with an index and no `.keep` marker, with their sizes.
fn packs(directory: &Path) -> Result<Vec<(u64, Oid)>> {
    let mut packs = Vec::new();
    if !directory.is_dir() {
        return Ok(packs);
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("pack-"))
            .and_then(|name| name.strip_suffix(".pack"))
            .filter(|id| hexadecimal(id, 40))
        else {
            continue;
        };
        if !entry.file_type()?.is_file()
            || !directory.join(format!("pack-{id}.idx")).is_file()
            || directory.join(format!("pack-{id}.keep")).exists()
        {
            continue;
        }
        packs.push((entry.metadata()?.len(), Oid::from_str(id)?));
    }
    Ok(packs)
}

/// The object IDs listed in a version 2 pack index.
fn indexed(path: &Path) -> Result<Vec<Oid>> {
    const FANOUT: usize = 8;
    const TABLE: usize = FANOUT + 256 * 4;
    let index = fs::read(path)?;
    if index.get(..8) != Some(b"\xfftOc\0\0\0\x02".as_slice()) {
        return Err(failure("Unsupported Git pack index"));
    }
    let count = index
        .get(TABLE - 4..TABLE)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| failure("Truncated Git pack index"))?;
    let count = usize::try_from(count).map_err(|_| failure("Oversized Git pack index"))?;
    let names = count
        .checked_mul(20)
        .and_then(|length| index.get(TABLE..TABLE.checked_add(length)?))
        .ok_or_else(|| failure("Truncated Git pack index"))?;
    names
        .as_chunks::<20>()
        .0
        .iter()
        .map(|name| Oid::from_bytes(name).map_err(Into::into))
        .collect()
}

fn hexadecimal(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn remove_loose(path: &Path) -> Result<()> {
    remove_object_file(path)
}

/// Git writes object files read-only; Windows needs that attribute cleared.
fn remove_object_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(failure("Git object file is not a regular file"));
    }
    #[cfg(windows)]
    {
        let original = metadata.permissions();
        let mut writable = original.clone();
        #[allow(
            clippy::permissions_set_readonly_false,
            reason = "Windows-only read-only attribute; Unix never changes object permissions for removal."
        )]
        writable.set_readonly(false);
        fs::set_permissions(path, writable)?;
        if let Err(error) = fs::remove_file(path) {
            fs::set_permissions(path, original)?;
            return Err(error.into());
        }
    }
    #[cfg(unix)]
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(repo: &Repository, count: usize) -> Vec<(Oid, PathBuf)> {
        (0..count)
            .map(|index| {
                let id = repo.blob(format!("object {index}").as_bytes()).unwrap();
                let text = id.to_string();
                (
                    id,
                    repo.path()
                        .join("objects")
                        .join(&text[..2])
                        .join(&text[2..]),
                )
            })
            .collect()
    }

    #[test]
    fn bounded_pass_preserves_all_objects_ancestry_and_named_refs() {
        let (_temporary, repo) = super::super::tests::repository();
        let objects = seed(&repo, LOOSE_THRESHOLD + 1);
        let mut builder = repo.treebuilder(None).unwrap();
        builder
            .insert("page.json", objects[0].0, 0o100_644)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@localhost").unwrap();
        let first = repo
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "First",
                &tree,
                &[],
            )
            .unwrap();
        let parent = repo.find_commit(first).unwrap();
        let second = repo
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "Second",
                &tree,
                &[&parent],
            )
            .unwrap();
        repo.tag(
            "named",
            parent.as_object(),
            &signature,
            "Named version",
            false,
        )
        .unwrap();
        repo.reference("refs/bluink/recovery/test", first, false, "Recovery")
            .unwrap();
        assert_eq!(maintain(&repo, || false).unwrap(), PASS_LIMIT);
        let disk = Repository::open_bare(repo.path()).unwrap();
        for (id, _) in objects {
            assert!(disk.find_blob(id).is_ok());
        }
        assert_eq!(
            disk.find_commit(second).unwrap().parent_id(0).unwrap(),
            first
        );
        assert_eq!(
            disk.revparse_single("refs/tags/named")
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .id(),
            first
        );
        assert_eq!(
            disk.refname_to_id("refs/bluink/recovery/test").unwrap(),
            first
        );
        assert_eq!(maintain(&repo, || false).unwrap(), 0);
    }

    #[test]
    fn cancellation_before_and_during_packing_preserves_loose_files() {
        let (_temporary, repo) = super::super::tests::repository();
        let objects = seed(&repo, LOOSE_THRESHOLD + 1);
        assert_eq!(maintain(&repo, || true).unwrap(), 0);
        let pack = repo.path().join("objects/pack");
        assert_eq!(
            maintain(&repo, || fs::read_dir(&pack).unwrap().any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "pack")))
            .unwrap(),
            0
        );
        assert!(objects.iter().all(|(_, path)| path.is_file()));
    }

    #[test]
    fn pack_failure_preserves_loose_objects_and_never_visits_lfs() {
        let (_temporary, repo) = super::super::tests::repository();
        let objects = seed(&repo, LOOSE_THRESHOLD + 1);
        let lfs = repo.path().join("lfs/objects/00");
        fs::create_dir_all(&lfs).unwrap();
        fs::write(lfs.join("keep"), b"asset").unwrap();
        let pack = repo.path().join("objects/pack");
        fs::remove_dir(&pack).unwrap();
        fs::write(&pack, b"blocked").unwrap();
        assert!(maintain(&repo, || false).is_err());
        assert!(objects.iter().all(|(_, path)| path.is_file()));
        assert_eq!(fs::read(lfs.join("keep")).unwrap(), b"asset");
    }

    /// One pack per object, with no loose copy left behind.
    fn packed(repo: &Repository, count: usize, prefix: &str) -> Vec<Oid> {
        seed_named(repo, count, prefix)
            .into_iter()
            .map(|(id, path)| {
                assert!(persist(repo, &[id], &|| false).unwrap());
                remove_loose(&path).unwrap();
                id
            })
            .collect()
    }

    fn seed_named(repo: &Repository, count: usize, prefix: &str) -> Vec<(Oid, PathBuf)> {
        (0..count)
            .map(|index| {
                let id = repo.blob(format!("{prefix} {index}").as_bytes()).unwrap();
                let text = id.to_string();
                let path = repo
                    .path()
                    .join("objects")
                    .join(&text[..2])
                    .join(&text[2..]);
                (id, path)
            })
            .collect()
    }

    fn pack_count(path: &Path) -> usize {
        packs(&path.join("objects/pack")).unwrap().len()
    }

    #[test]
    fn small_packs_merge_into_one_keeping_every_object_and_kept_packs() {
        let (_temporary, repo) = super::super::tests::repository();
        let objects = packed(&repo, PACK_THRESHOLD + 4, "checkpoint");
        let kept = packed(&repo, 1, "kept")[0];
        let directory = repo.path().join("objects/pack");
        let marked = packs(&directory)
            .unwrap()
            .into_iter()
            .map(|(_, id)| id)
            .find(|id| indexed(&directory.join(format!("pack-{id}.idx"))).unwrap() == [kept])
            .unwrap();
        fs::write(directory.join(format!("pack-{marked}.keep")), b"").unwrap();
        let path = repo.path().to_owned();
        drop(repo);
        assert_eq!(consolidate(&path, || false).unwrap(), PACK_THRESHOLD + 4);
        // The merged pack, plus the kept pack the merge left alone.
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "pack"))
                .count(),
            2
        );
        assert!(directory.join(format!("pack-{marked}.pack")).is_file());
        let disk = Repository::open_bare(&path).unwrap();
        for id in objects.iter().chain([&kept]) {
            assert!(disk.find_blob(*id).is_ok());
        }
        drop(disk);
        // Two packs are below the threshold: nothing more to merge.
        assert_eq!(consolidate(&path, || false).unwrap(), 0);
    }

    #[test]
    fn few_packs_or_cancellation_leave_packs_untouched() {
        let (_temporary, repo) = super::super::tests::repository();
        packed(&repo, PACK_THRESHOLD - 1, "few");
        let path = repo.path().to_owned();
        drop(repo);
        assert_eq!(consolidate(&path, || false).unwrap(), 0);
        assert_eq!(pack_count(&path), PACK_THRESHOLD - 1);
        let repo = Repository::open_bare(&path).unwrap();
        packed(&repo, 1, "more");
        drop(repo);
        assert_eq!(consolidate(&path, || true).unwrap(), 0);
        assert_eq!(pack_count(&path), PACK_THRESHOLD);
    }
}
