// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! SQLite captures become immutable Git objects in a bare repository. Ref CAS
//! precedes a durable acknowledgement; no worktree or Git index participates.

use std::{collections::BTreeMap, fs, path::Path};

use git2::{Oid, Repository, RepositoryInitOptions};
use uuid::Uuid;

use crate::{
    domain::project::canonical_json,
    store::{self, git},
};

use super::{Result, failure, lfs, repository};

pub(crate) const DIRECTORY: &str = "history.git";
mod backup;
pub(crate) use super::lfs::digest_hex;
pub(super) mod delta;
pub(crate) mod history;
pub(in crate::versioning) mod inventory;
pub(crate) use inventory::cover_backup;
pub(crate) mod portable;
pub(crate) use backup::{HistoryCopy, copy_pinned_history};

pub(crate) struct VerifiedAsset(lfs::Pointer);

impl VerifiedAsset {
    pub(crate) fn oid(&self) -> &str {
        self.0.identity().0
    }
    pub(crate) fn size(&self) -> u64 {
        self.0.identity().1
    }
}

pub(crate) fn validate_path(path: &str) -> Result<()> {
    repository::tracked(path)
        .map(|_| ())
        .ok_or_else(|| failure("Path is outside the portable project format"))
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Portable Git paths have case-sensitive extensions on every host"
)]
pub(crate) fn validate_record_path(path: &str) -> Result<()> {
    let theme_json = path.starts_with("themes/")
        && path.ends_with(".json")
        && repository::tracked(path) == Some(repository::Kind::Text);
    if !(repository::tracked(path) == Some(repository::Kind::Json) || theme_json)
        || path.starts_with("items/")
    {
        return Err(failure(
            "Small project records require a supported JSON path",
        ));
    }
    Ok(())
}

pub(crate) fn validate_asset_path(path: &str) -> Result<()> {
    validate_path(path)?;
    if !(path.starts_with("assets/") || path.starts_with("themes/")) {
        return Err(failure("Attachments require an owner asset or theme path"));
    }
    Ok(())
}

pub(crate) fn verify_asset(root: &Path, oid: &str, size: u64) -> Result<VerifiedAsset> {
    let repo = open(root)?;
    let pointer = lfs::Pointer::from_identity(oid, size)?;
    pointer.verified_path(repo.path())?;
    Ok(VerifiedAsset(pointer))
}

/// Current mappings already refer to verified immutable bytes. Streaming checks
/// the confined path, regular file and declared size before opening its range.
pub(crate) fn available_asset_path(
    root: &Path,
    oid: &str,
    size: u64,
) -> Result<std::path::PathBuf> {
    lfs::Pointer::from_identity(oid, size)?.stored_path(open(root)?.path())
}

/// Verify the complete content identity when accepting or exporting an object.
#[cfg(test)]
pub(crate) fn verified_asset_path(root: &Path, oid: &str, size: u64) -> Result<std::path::PathBuf> {
    lfs::Pointer::from_identity(oid, size)?.verified_path(open(root)?.path())
}

pub(crate) fn install_asset(root: &Path, file: &mut fs::File) -> Result<VerifiedAsset> {
    let repo = open(root)?;
    Ok(VerifiedAsset(lfs::capture(repo.path(), file)?))
}

pub(crate) fn install_asset_bytes(root: &Path, bytes: &[u8]) -> Result<VerifiedAsset> {
    let repo = open(root)?;
    Ok(VerifiedAsset(lfs::capture_bytes(repo.path(), bytes)?))
}

pub(crate) fn copy_verified_asset(
    repo: &Repository,
    oid: &str,
    size: u64,
    output: &mut fs::File,
) -> Result<()> {
    lfs::Pointer::from_identity(oid, size)?.copy_verified(repo.path(), output)
}

pub(crate) fn open(root: &Path) -> Result<Repository> {
    let directory = root.join(DIRECTORY);
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(failure(
            "Project history must be a real bare repository directory",
        ));
    }
    let repo = Repository::open_bare(directory)?;
    if !repo.is_bare()
        || repo.workdir().is_some()
        || repo.find_reference("HEAD")?.symbolic_target()? != Some(repository::BRANCH)
    {
        return Err(failure("Project history must be bare and remain on main"));
    }
    Ok(repo)
}

pub(crate) fn initialize(root: &Path) -> Result<()> {
    let directory = root.join(DIRECTORY);
    let repo = if directory.try_exists()? {
        open(root)?
    } else {
        let mut options = RepositoryInitOptions::new();
        options
            .bare(true)
            .initial_head("main")
            .external_template(false);
        Repository::init_opts(directory, &options)?
    };
    repo.config()?.set_bool("core.fsyncObjectFiles", true)?;
    repo.config()?.set_bool("core.autocrlf", false)?;
    Ok(())
}

fn database<T>(result: store::Result<T>) -> Result<T> {
    result.map_err(|error| failure(error.to_string()))
}

/// Caller holds the history lease and has just checkpointed. Recovery installs
/// the new tag in the history projection.
pub(super) fn name_head(
    repo: &Repository,
    store: &store::Store,
    request: &super::model::NamedVersionRequest,
) -> Result<super::model::NamedVersion> {
    let head = repository::head(repo)?.ok_or_else(|| failure("Project has no checkpoints"))?;
    let project = database(store.read(crate::store::project::read_config))?;
    let named = super::history::write_named_tag(
        repo,
        head,
        request,
        project["id"]
            .as_str()
            .ok_or_else(|| failure("Project has no identity"))?,
    )?;
    recover(repo, store)?;
    Ok(named)
}

/// Caller holds the cross-process history lease. A crash before CAS retries it;
/// a crash after CAS acknowledges the already-published exact candidate. Any
/// unexpected tip is retained and reported instead of overwritten.
pub(crate) fn recover(repo: &Repository, store: &store::Store) -> Result<()> {
    let operations = database(store.read(git::pending_operations))?;
    for operation in operations {
        let project = database(store.read(|tx| {
            Ok(tx.query_row::<String, _, _>(
                "SELECT project_uuid FROM project_state",
                [],
                |row| row.get(0),
            )?)
        }))?;
        for (name, oid) in &operation.references {
            let oid = Oid::from_str(oid)?;
            super::history::named_tag(repo, name, &repo.find_tag(oid)?, Some(&project))?;
            match repo.refname_to_id(name) {
                Ok(existing) if existing != oid => {
                    return Err(failure(
                        "A named version differs from its pending publication; both copies were retained",
                    ));
                }
                Err(error) if error.code() != git2::ErrorCode::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        let observed = repository::head(repo)?.map(|oid| oid.to_string());
        if observed == operation.expected_head {
            let previous = operation
                .expected_head
                .as_deref()
                .map(Oid::from_str)
                .transpose()?;
            repository::publish_ref(repo, previous, Oid::from_str(&operation.candidate_head)?)?;
        } else if observed.as_deref() != Some(operation.candidate_head.as_str()) {
            return Err(failure(
                "Git history moved outside its prepared operation; all tips were retained",
            ));
        }
        for (name, oid) in &operation.references {
            if repo.find_reference(name).is_err() {
                repo.reference(
                    name,
                    Oid::from_str(oid)?,
                    false,
                    "Complete named version publication",
                )?;
            }
        }
        let projection = history::prepare(repo, store)?;
        let candidate = operation.candidate_head.clone();
        database(store.write(move |transaction| {
            if let Some(projection) = &projection {
                git::history::install(transaction, projection)?;
            }
            git::acknowledge(transaction, &operation, &candidate)
        }))?;
    }
    let expected = database(store.read(git::expected_head))?;
    if repository::head(repo)?.map(|oid| oid.to_string()) != expected {
        return Err(failure(
            "Git history does not match the acknowledged database head",
        ));
    }
    database(store.write(git::discard_unprepared_captures))?;
    if let Some(projection) = history::prepare(repo, store)? {
        database(store.write(move |transaction| git::history::install(transaction, &projection)))?;
    }
    Ok(())
}

fn commit_checkpoint(
    repo: &Repository,
    head: Option<Oid>,
    tree: Oid,
    device_id: &str,
    device_name: &str,
) -> Result<Oid> {
    let parent = head.map(|oid| repo.find_commit(oid)).transpose()?;
    if let Some(parent) = parent.as_ref().filter(|commit| commit.tree_id() == tree) {
        return Ok(parent.id());
    }
    let signature = git2::Signature::now("Bluink", "local@bluink.invalid")?;
    let message = format!(
        "Saved project changes\n\nBluink-Device: {device_id}\nBluink-Device-Name: {device_name}\n"
    );
    Ok(repo.commit(
        None,
        &signature,
        &signature,
        &message,
        &repo.find_tree(tree)?,
        &parent.iter().collect::<Vec<_>>(),
    )?)
}

/// The versioning worker already owns the process lease and serial lock.
pub(super) fn checkpoint_locked(
    repo: &Repository,
    store: &store::Store,
    device_id: &str,
    device_name: &str,
) -> Result<Option<Oid>> {
    recover(repo, store)?;
    let head = repository::head(repo)?;
    let id = Uuid::now_v7().to_string();
    let expected = head.map(|oid| oid.to_string());
    let now = chrono::Utc::now().timestamp_millis();
    let capture = database(
        store.write(move |transaction| git::capture(transaction, &id, expected.as_deref(), now)),
    )?;
    let paths = database(store.read(|transaction| git::captured_paths(transaction, &capture.id)))?;
    if paths.is_empty() && head.is_some() {
        // No pending sources: remove this empty reservation, preserving history.
        let id = capture.id;
        database(store.write(move |transaction| {
            transaction.execute("DELETE FROM capture WHERE id=?1", [id])?;
            Ok(())
        }))?;
        return Ok(head);
    }
    let mut objects = BTreeMap::new();
    if head.is_none() {
        objects.insert(
            ".gitattributes".to_owned(),
            Some(repo.blob(repository::ATTRIBUTES)?),
        );
        objects.insert(
            ".gitignore".to_owned(),
            Some(repo.blob(repository::IGNORE)?),
        );
    }
    let odb = repo.odb()?;
    let mut packed = super::packing::ObjectWriter::new(repo, &odb)?;
    let mut introduced = git::inventory::Pointers::new();
    for path in paths {
        validate_path(&path)?;
        let source = database(
            store.read(|transaction| git::captured_source(transaction, &capture.id, &path)),
        )?;
        let object = match source {
            git::CapturedSource::Deleted => None,
            git::CapturedSource::Json(value) => {
                if path.starts_with("items/") {
                    for pointer in super::transport::decision_pointers(&value)? {
                        inventory::add(&mut introduced, &pointer)?;
                    }
                }
                Some(packed.blob(&canonical_json::to_vec(&value)?)?)
            }
            git::CapturedSource::Asset { oid, size } => {
                let pointer = lfs::Pointer::from_identity(&oid, size)?;
                let source = pointer.verified_path(repo.path())?;
                if repository::tracked(&path) == Some(repository::Kind::Binary) {
                    inventory::add(&mut introduced, &pointer)?;
                    Some(packed.blob(&pointer.bytes())?)
                } else {
                    Some(packed.blob(&fs::read(source)?)?)
                }
            }
        };
        objects.insert(path, object);
    }
    let tree = repository::changed_tree_recording(repo, head, &objects, &mut |oid| {
        packed.record(oid).map(|_| ())
    })?;
    let candidate = commit_checkpoint(repo, head, tree, device_id, device_name)?;
    packed.record(candidate)?;
    packed.finish()?;
    let candidate_text = candidate.to_string();
    database(store.write(move |transaction| {
        if head != Some(candidate) {
            git::inventory::install(
                transaction,
                &git::inventory::Commit {
                    id: candidate_text.clone(),
                    parents: head.into_iter().map(|id| id.to_string()).collect(),
                    pointers: introduced,
                },
            )?;
        }
        git::prepare_checkpoint(transaction, &capture, &candidate_text)
    }))?;
    // Recovery is also the normal publication path, so interrupted publication
    // never relies on a second implementation of its state transitions.
    recover(repo, store)?;
    Ok(Some(candidate))
}
