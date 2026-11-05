// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded Git object packing. A verified durable pack precedes any ref update.

mod maintenance;
pub(super) use maintenance::{consolidate, maintain};

use super::{Result, failure};
use git2::{ObjectFormat, Odb, Oid, Repository};
use std::{collections::BTreeSet, fs, io::Write, path::Path};

const OBJECT_LIMIT: usize = 1_000;
const BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// Use a fresh checkpoint repository handle and record every tree/commit written
/// through it. `finish` verifies all objects and detaches the memory backend before
/// the caller may publish refs. A failed writer must not be reused.
pub(super) struct ObjectWriter<'repo, 'odb> {
    repo: &'repo Repository,
    odb: &'odb Odb<'repo>,
    disk: Repository,
    memory: git2::Mempack<'odb>,
    pending: BTreeSet<Oid>,
    bytes: usize,
    failed: bool,
}

impl<'repo, 'odb> ObjectWriter<'repo, 'odb> {
    pub(super) fn new(repo: &'repo Repository, odb: &'odb Odb<'repo>) -> Result<Self> {
        require_sha1(repo)?;
        Ok(Self {
            repo,
            odb,
            disk: Repository::open_bare(repo.path())?,
            memory: odb.add_new_mempack_backend(1_000)?,
            pending: BTreeSet::new(),
            bytes: 0,
            failed: false,
        })
    }

    pub(super) fn blob(&mut self, bytes: &[u8]) -> Result<Oid> {
        self.ready()?;
        let id = Oid::hash_object(git2::ObjectType::Blob, bytes)?;
        if self.disk.odb()?.exists(id) {
            return Ok(id);
        }
        if self.bytes.saturating_add(bytes.len()) > BYTE_LIMIT {
            self.flush()?;
        }
        let result = self.repo.blob(bytes).map_err(Into::into);
        let id = self.checked(result)?;
        self.record(id)
    }

    /// Immediately register every `TreeBuilder::write` and `commit(None, ...)`
    /// result. A single object exceeding the byte budget is flushed alone.
    pub(super) fn record(&mut self, id: Oid) -> Result<Oid> {
        self.ready()?;
        if self.disk.odb()?.exists(id) {
            return Ok(id);
        }
        let result = self.odb.read_header(id).map_err(Into::into);
        let (size, _) = self.checked(result)?;
        if self.pending.insert(id) {
            self.bytes = self.bytes.saturating_add(size);
        }
        if self.pending.len() >= OBJECT_LIMIT || self.bytes >= BYTE_LIMIT {
            self.flush()?;
        }
        Ok(id)
    }

    pub(super) fn finish(mut self) -> Result<()> {
        self.flush()?;
        // Mempack is owned by the ODB, not its Rust wrapper. Without replacing
        // this ODB, subsequent writes through repo would silently stay in RAM.
        let disk = Repository::open_bare(self.repo.path())?;
        self.repo.set_odb(&disk.odb()?)?;
        Ok(())
    }

    fn ready(&self) -> Result<()> {
        if self.failed {
            Err(failure(
                "Object packing failed; retry with a new repository handle",
            ))
        } else {
            Ok(())
        }
    }

    const fn checked<T>(&mut self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn flush(&mut self) -> Result<()> {
        self.ready()?;
        if self.pending.is_empty() {
            return Ok(());
        }
        let ids: Vec<_> = self.pending.iter().copied().collect();
        let result = persist(self.repo, &ids, &|| false);
        if !self.checked(result)? {
            return Err(failure("Unexpected object packing cancellation"));
        }
        // The independent pack-only reader above must succeed before memory can
        // be discarded. Neither an existing loose object nor the cache can mask
        // a missing packed object in that verification.
        let result = self.memory.reset().map_err(Into::into);
        self.checked(result)?;
        let result = self.odb.refresh().map_err(Into::into);
        self.checked(result)?;
        let result = self
            .disk
            .odb()
            .and_then(|disk| disk.refresh())
            .map_err(Into::into);
        self.checked(result)?;
        self.pending.clear();
        self.bytes = 0;
        Ok(())
    }
}

/// Copy every object of `from` into a new repository as packs, never as one
/// loose file per object. Existing packs stream through the destination
/// indexer, which recomputes every object ID and checks the pack checksum;
/// remaining objects are packed in bounded batches. Completeness is verified
/// against a fresh destination handle before any caller publishes refs.
pub(super) fn copy_objects(from: &Repository, to: &Repository) -> Result<usize> {
    require_sha1(from)?;
    require_sha1(to)?;
    let source = from.odb()?;
    // An object can be both loose and packed.
    let mut ids = BTreeSet::new();
    source.foreach(|oid| {
        ids.insert(*oid);
        true
    })?;
    let target = to.odb()?;
    let packs = from.path().join("objects/pack");
    if packs.is_dir() {
        for entry in fs::read_dir(&packs)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "pack")
            {
                let mut writer = target.packwriter()?;
                std::io::copy(&mut fs::File::open(&path)?, &mut writer)?;
                writer.commit()?;
            }
        }
    }
    target.refresh()?;
    let mut batch = Vec::new();
    let mut bytes = 0usize;
    for id in ids.iter().copied().filter(|id| !target.exists(*id)) {
        bytes = bytes.saturating_add(source.read_header(id)?.0);
        batch.push(id);
        if batch.len() >= OBJECT_LIMIT || bytes >= BYTE_LIMIT {
            copy_batch(from, &target, &batch)?;
            batch.clear();
            bytes = 0;
        }
    }
    copy_batch(from, &target, &batch)?;
    let directory = to.path().join("objects/pack");
    if directory.is_dir() {
        for entry in fs::read_dir(&directory)? {
            sync_file(&entry?.path())?;
        }
        #[cfg(unix)]
        sync_directory(&directory)?;
    }
    let copied = Repository::open_bare(to.path())?;
    let copied = copied.odb()?;
    if ids.iter().any(|id| !copied.exists(*id)) {
        return Err(failure("Copied Git history is incomplete"));
    }
    Ok(ids.len())
}

fn copy_batch(from: &Repository, target: &Odb<'_>, ids: &[Oid]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut builder = from.packbuilder()?;
    for id in ids {
        builder.insert_object(*id, None)?;
    }
    let mut buffer = git2::Buf::new();
    builder.write_buf(&mut buffer)?;
    drop(builder);
    let mut writer = target.packwriter()?;
    writer.write_all(&buffer)?;
    writer.commit()?;
    Ok(())
}

fn require_sha1(repo: &Repository) -> Result<()> {
    if repo.object_format() != ObjectFormat::Sha1 {
        return Err(failure("Project history requires SHA-1 Git object IDs"));
    }
    Ok(())
}

/// Returns true only when this exact pack has persisted, readable copies of all
/// requested objects. No refs, loose objects, or in-memory objects are removed.
fn persist(repo: &Repository, ids: &[Oid], cancelled: &impl Fn() -> bool) -> Result<bool> {
    Ok(persist_pack(repo, ids, cancelled)?.is_some())
}

/// Like `persist`, naming the verified pack; `None` means cancelled.
fn persist_pack(
    repo: &Repository,
    ids: &[Oid],
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Oid>> {
    require_sha1(repo)?;
    if cancelled() {
        return Ok(None);
    }
    let mut builder = repo.packbuilder()?;
    builder.set_threads(4);
    builder.set_progress_callback(|_, _, _| !cancelled())?;
    for id in ids {
        if cancelled() {
            return Ok(None);
        }
        // Mempack::dump only walks commits: it omits blob-only batches.
        builder.insert_object(*id, None)?;
    }
    let mut buffer = git2::Buf::new();
    if let Err(error) = builder.write_buf(&mut buffer) {
        return if cancelled() {
            Ok(None)
        } else {
            Err(error.into())
        };
    }
    drop(builder);
    if cancelled() {
        return Ok(None);
    }
    let bytes: &[u8] = &buffer;
    let (_, checksum) = bytes
        .split_last_chunk::<20>()
        .ok_or_else(|| failure("Incomplete Git pack"))?;
    let pack_id = Oid::from_bytes(checksum)?;
    let odb = repo.odb()?;
    {
        let mut writer = odb.packwriter()?;
        writer.progress(|_| !cancelled());
        let written = writer
            .write_all(bytes)
            .map_err(super::Error::from)
            .and_then(|()| writer.commit().map(|_| ()).map_err(Into::into));
        if let Err(error) = written {
            return if cancelled() { Ok(None) } else { Err(error) };
        }
    }
    let directory = repo.path().join("objects/pack");
    for extension in ["pack", "idx"] {
        sync_file(&directory.join(format!("pack-{pack_id}.{extension}")))?;
    }
    #[cfg(unix)]
    sync_directory(&directory)?;
    if cancelled() {
        return Ok(None);
    }
    Ok(verify_pack(&directory, pack_id, ids, cancelled)?.then_some(pack_id))
}

fn sync_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(failure("Git pack is not a regular file"));
    }
    let original = metadata.permissions();
    let mut writable = original.clone();
    #[cfg(windows)]
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "Windows-only attribute change; Unix adds only owner write permission below."
    )]
    writable.set_readonly(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        writable.set_mode(original.mode() | 0o200);
    }
    fs::set_permissions(path, writable)?;
    let flushed = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all());
    // Always restore the exact original permissions, including on flush failure.
    let restored = fs::set_permissions(path, original);
    if let Err(error) = restored {
        return Err(failure(format!(
            "Could not restore Git pack permissions: {error}"
        )));
    }
    flushed?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn verify_pack(
    directory: &Path,
    pack_id: Oid,
    ids: &[Oid],
    cancelled: &impl Fn() -> bool,
) -> Result<bool> {
    let temporary = tempfile::Builder::new()
        .prefix("bluink-pack-check-")
        .tempdir_in(directory)?;
    let pack = temporary.path().join("pack");
    fs::create_dir(&pack)?;
    for extension in ["pack", "idx"] {
        let name = format!("pack-{pack_id}.{extension}");
        let source = directory.join(&name);
        let target = pack.join(name);
        // Do not hard-link read-only files: Windows scratch cleanup may clear
        // their attribute, which would also modify the original pack's inode.
        fs::copy(&source, &target)?;
    }
    {
        let isolated = Odb::new()?;
        isolated.add_disk_alternate(
            temporary
                .path()
                .to_str()
                .ok_or_else(|| failure("Git object directory must be UTF-8"))?,
        )?;
        for id in ids {
            if cancelled() {
                return Ok(false);
            }
            let object = isolated.read(*id)?;
            if Oid::hash_object(object.kind(), object.data())? != *id {
                return Err(failure("Persisted Git object failed its checksum"));
            }
        }
    }
    temporary.close()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn repository() -> (tempfile::TempDir, Repository) {
        let temporary = tempfile::tempdir().unwrap();
        let mut options = git2::RepositoryInitOptions::new();
        options
            .bare(true)
            .initial_head("main")
            .external_template(false);
        let repo = Repository::init_opts(temporary.path(), &options).unwrap();
        (temporary, repo)
    }

    #[test]
    fn blobs_trees_and_commits_survive_without_the_memory_backend() {
        let (_temporary, repo) = repository();
        let odb = repo.odb().unwrap();
        let mut writer = ObjectWriter::new(&repo, &odb).unwrap();
        let blob = writer.blob(b"text").unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("page.json", blob, 0o100_644).unwrap();
        let tree = writer.record(builder.write().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@localhost").unwrap();
        let commit = writer
            .record(
                repo.commit(
                    None,
                    &signature,
                    &signature,
                    "Checkpoint",
                    &repo.find_tree(tree).unwrap(),
                    &[],
                )
                .unwrap(),
            )
            .unwrap();
        assert!(repo.head().is_err());
        writer.finish().unwrap();
        let disk = Repository::open_bare(repo.path()).unwrap();
        assert_eq!(disk.find_commit(commit).unwrap().tree_id(), tree);
        assert_eq!(
            disk.find_tree(tree)
                .unwrap()
                .get_name("page.json")
                .unwrap()
                .id(),
            blob
        );
        assert_eq!(disk.find_blob(blob).unwrap().content(), b"text");
        // A later ordinary write must not be trapped in the finished mempack.
        let later = repo.blob(b"later").unwrap();
        assert_eq!(
            Repository::open_bare(repo.path())
                .unwrap()
                .find_blob(later)
                .unwrap()
                .content(),
            b"later"
        );
        assert!(repo.head().is_err());
    }

    #[test]
    fn object_and_byte_limits_flush_before_finish() {
        let (_temporary, repo) = repository();
        let odb = repo.odb().unwrap();
        let mut writer = ObjectWriter::new(&repo, &odb).unwrap();
        let mut first = None;
        for index in 0..OBJECT_LIMIT {
            let id = writer.blob(index.to_string().as_bytes()).unwrap();
            first.get_or_insert(id);
        }
        assert!(writer.pending.is_empty());
        assert!(
            Repository::open_bare(repo.path())
                .unwrap()
                .find_blob(first.unwrap())
                .is_ok()
        );
        let large = writer.blob(&vec![b'x'; BYTE_LIMIT + 1]).unwrap();
        assert!(writer.pending.is_empty());
        assert_eq!(
            Repository::open_bare(repo.path())
                .unwrap()
                .find_blob(large)
                .unwrap()
                .size(),
            BYTE_LIMIT + 1
        );
        writer.finish().unwrap();
    }

    #[test]
    fn failed_pack_preserves_memory_and_rejects_further_writes() {
        let (_temporary, repo) = repository();
        let odb = repo.odb().unwrap();
        let mut writer = ObjectWriter::new(&repo, &odb).unwrap();
        let id = writer.blob(b"unpublished").unwrap();
        let directory = repo.path().join("objects/pack");
        fs::remove_dir(&directory).unwrap();
        fs::write(&directory, b"block pack creation").unwrap();
        assert!(writer.flush().is_err());
        assert_eq!(odb.read(id).unwrap().data(), b"unpublished");
        assert!(writer.pending.contains(&id));
        assert!(writer.blob(b"another").is_err());
        assert!(repo.head().is_err());
    }

    #[test]
    fn independent_verification_cannot_fall_back_to_loose_objects() {
        let (_temporary, repo) = repository();
        let packed = repo.blob(b"packed").unwrap();
        let absent = repo.blob(b"only loose").unwrap();
        assert!(persist(&repo, &[packed], &|| false).unwrap());
        let directory = repo.path().join("objects/pack");
        let filename = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension == "pack")
            })
            .unwrap();
        let id = Oid::from_str(
            filename
                .strip_prefix("pack-")
                .unwrap()
                .strip_suffix(".pack")
                .unwrap(),
        )
        .unwrap();
        assert!(verify_pack(&directory, id, &[absent], &|| false).is_err());
        assert!(
            Repository::open_bare(repo.path())
                .unwrap()
                .find_blob(absent)
                .is_ok()
        );
    }

    #[test]
    fn copies_packed_and_loose_objects_into_packs_only() {
        let (_source_directory, source) = repository();
        let packed = source.blob(b"packed").unwrap();
        assert!(persist(&source, &[packed], &|| false).unwrap());
        let loose = source.blob(b"loose").unwrap();
        let (_target_directory, target) = repository();
        assert_eq!(copy_objects(&source, &target).unwrap(), 2);
        let copied = Repository::open_bare(target.path()).unwrap();
        assert_eq!(copied.find_blob(packed).unwrap().content(), b"packed");
        assert_eq!(copied.find_blob(loose).unwrap().content(), b"loose");
        let loose_directories = fs::read_dir(target.path().join("objects"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.len() == 2)
            .count();
        assert_eq!(loose_directories, 0);
    }

    #[test]
    fn flush_restores_read_only_permissions() {
        let (temporary, _repo) = repository();
        let path = temporary.path().join("test-pack");
        fs::write(&path, b"test").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        sync_file(&path).unwrap();
        assert!(fs::metadata(path).unwrap().permissions().readonly());
    }

    #[test]
    fn existing_disk_objects_do_not_create_duplicate_packs() {
        let (_temporary, repo) = repository();
        let existing = repo.blob(b"already saved").unwrap();
        let odb = repo.odb().unwrap();
        let mut writer = ObjectWriter::new(&repo, &odb).unwrap();
        assert_eq!(writer.blob(b"already saved").unwrap(), existing);
        writer.record(existing).unwrap();
        assert!(writer.pending.is_empty());
        let new = writer.blob(b"new").unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.blob(b"new").unwrap(), new);
        writer.record(new).unwrap();
        assert!(writer.pending.is_empty());
        writer.finish().unwrap();
        let packs = fs::read_dir(repo.path().join("objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "pack")
            })
            .count();
        assert_eq!(packs, 1);
    }
}
