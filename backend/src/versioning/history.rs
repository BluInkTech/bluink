// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    History, Result, checkpoints, failure,
    model::{HistoryPage, HistoryRequest, NamedVersion, NamedVersionRequest},
    repository,
};
use crate::domain::project::identity::ItemId;
use crate::store::io::{self, LockMode};
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

const TAG_PREFIX: &str = "refs/tags/bluink/versions/";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionMetadata {
    version: u8,
    project_id: String,
    title: String,
    item_id: Option<String>,
}

fn version(repo: &Repository, reference: &git2::Reference<'_>) -> Result<NamedVersion> {
    named_tag(repo, reference.name()?, &reference.peel_to_tag()?, None)
}

pub(super) fn named_tag(
    repo: &Repository,
    name: &str,
    tag: &git2::Tag<'_>,
    expected_project: Option<&str>,
) -> Result<NamedVersion> {
    let id = name
        .strip_prefix(TAG_PREFIX)
        .ok_or_else(|| failure("Invalid named version reference"))?;
    ItemId::try_from(id)?;
    let metadata: VersionMetadata = serde_json::from_str(
        tag.message()?
            .ok_or_else(|| failure("Named version has no metadata"))?,
    )?;
    if metadata.version != 1
        || tag.target_type() != Some(git2::ObjectType::Commit)
        || Some(tag.name()?) != name.strip_prefix("refs/tags/")
        || !valid_title(&metadata.title)
    {
        return Err(failure("Unsupported named version"));
    }
    let target = repo.find_commit(tag.target_id())?;
    let config_entry = target.tree()?.get_path(Path::new("bluink.json"))?;
    let config = crate::domain::project::config::parse_project_config(
        repo.find_blob(config_entry.id())?.content(),
    )?;
    if config["id"].as_str() != Some(metadata.project_id.as_str())
        || expected_project.is_some_and(|project| project != metadata.project_id)
    {
        return Err(failure("Named version belongs to another project"));
    }
    if let Some(id) = &metadata.item_id {
        let entry = target.tree()?.get_path(&item_path(id)?)?;
        let page: serde_json::Value =
            serde_json::from_slice(repo.find_blob(entry.id())?.content())?;
        if page["meta"]["type"] != "page" || page["id"].as_str() != Some(id) {
            return Err(failure("Named page versions require a page"));
        }
    }
    Ok(NamedVersion {
        id: id.to_owned(),
        title: metadata.title,
        item_id: metadata.item_id,
        commit_id: target.id().to_string(),
    })
}

fn valid_title(title: &str) -> bool {
    !title.trim().is_empty() && title.chars().count() <= 120 && !title.chars().any(char::is_control)
}

pub(super) fn reachable(repo: &Repository, source: &str) -> Result<Oid> {
    if source.len() != 40 {
        return Err(failure("Choose a complete checkpoint ID"));
    }
    let id = Oid::from_str(source)?;
    let head = repository::head(repo)?.ok_or_else(|| failure("Project has no checkpoints"))?;
    if id != head && !repo.graph_descendant_of(head, id)? {
        return Err(failure("Checkpoint is not part of this project's history"));
    }
    repo.find_commit(id)?;
    Ok(id)
}

fn item_path(id: &str) -> Result<std::path::PathBuf> {
    Ok(ItemId::try_from(id)?.path(Path::new("")))
}

impl History {
    pub(crate) fn historical_asset(
        &self,
        commit: &str,
        id: &str,
        filename: &str,
    ) -> Result<std::path::PathBuf> {
        if !crate::domain::project::asset::valid_filename(filename) {
            return Err(failure("Invalid historical asset filename"));
        }
        let repo = checkpoints::open(&self.root)?;
        let commit = super::history::reachable(&repo, commit)?;
        let path = ItemId::try_from(id)?
            .asset_directory(Path::new(""))
            .join(filename);
        let entry = repo.find_commit(commit)?.tree()?.get_path(&path)?;
        let pointer = super::lfs::Pointer::parse(repo.find_blob(entry.id())?.content())?;
        self.ensure_history_asset(&repo, &pointer)?;
        pointer.verified_path(repo.path())
    }
    pub(crate) fn historical_item(
        &self,
        request: &super::model::HistoryItemRequest,
    ) -> Result<serde_json::Value> {
        let repo = checkpoints::open(&self.root)?;
        let commit = super::history::reachable(&repo, &request.commit_id)?;
        let path = item_path(&request.item_id)?;
        let entry = repo.find_commit(commit)?.tree()?.get_path(&path)?;
        let value = serde_json::from_slice(repo.find_blob(entry.id())?.content())?;
        crate::domain::project::item::validate_item(&value, &request.item_id)?;
        Ok(value)
    }
    pub(crate) fn history(&self, request: &HistoryRequest) -> Result<HistoryPage> {
        checkpoints::history::read(&self.root, self.canonical()?.as_ref(), request)
    }
    pub(crate) fn named_version(&self, request: &NamedVersionRequest) -> Result<NamedVersion> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        let canonical = self.canonical()?;
        checkpoints::recover(&repo, &canonical)?;
        // A retried request returns its version without checkpointing newer saves.
        if let Some(previous) = existing_named_tag(&repo, request)? {
            return Ok(previous);
        }
        self.checkpoint_locked(&repo)?;
        checkpoints::name_head(&repo, &canonical, request)
    }
}

/// A retried request returns its original version; reusing its ID with
/// different details is rejected.
fn existing_named_tag(
    repo: &Repository,
    request: &NamedVersionRequest,
) -> Result<Option<NamedVersion>> {
    let title = request.title.trim();
    match repo.find_reference(&tag_name(request)?) {
        Ok(reference) => {
            let previous = version(repo, &reference)?;
            if previous.title != title || previous.item_id != request.item_id {
                return Err(failure(
                    "This version request was already used with different details",
                ));
            }
            Ok(Some(previous))
        }
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn tag_name(request: &NamedVersionRequest) -> Result<String> {
    let request_id = ItemId::try_from(request.request_id.as_str())?;
    if !valid_title(request.title.trim()) {
        return Err(failure("Enter a version name of 1–120 characters"));
    }
    Ok(format!("{TAG_PREFIX}{}", request_id.as_str()))
}

/// Named-version serialization and scope checks are shared by both history
/// coordinators during the storage cutover; the Git contract does not change.
pub(super) fn write_named_tag(
    repo: &Repository,
    head: Oid,
    request: &NamedVersionRequest,
    project_id: &str,
) -> Result<NamedVersion> {
    if let Some(previous) = existing_named_tag(repo, request)? {
        return Ok(previous);
    }
    let title = request.title.trim();
    let name = tag_name(request)?;
    if let Some(id) = &request.item_id {
        let entry = repo.find_commit(head)?.tree()?.get_path(&item_path(id)?)?;
        let item: serde_json::Value =
            serde_json::from_slice(repo.find_blob(entry.id())?.content())?;
        if item["meta"]["type"] != "page" || item["id"].as_str() != Some(id) {
            return Err(failure("Named page versions require a page"));
        }
    }
    let metadata = VersionMetadata {
        version: 1,
        project_id: project_id.to_owned(),
        title: title.to_owned(),
        item_id: request.item_id.clone(),
    };
    let signature = git2::Signature::now("Bluink", "local@bluink.invalid")?;
    repo.tag(
        name.strip_prefix("refs/tags/")
            .ok_or_else(|| failure("Invalid version tag"))?,
        &repo.find_object(head, None)?,
        &signature,
        &serde_json::to_string(&metadata)?,
        false,
    )?;
    version(repo, &repo.find_reference(&name)?)
}

/// Availability is a display hint, not integrity validation. Downloads and
/// restores verify object hashes before using their bytes. Shared immutable asset
/// subtrees are checked once per history page, even across many prose revisions.
#[derive(Default)]
pub(super) struct AttachmentPresence {
    blobs: BTreeMap<Oid, bool>,
    trees: BTreeMap<(Oid, String), bool>,
}

// Bluink never removes local attachment bytes, so a subtree whose attachments
// were all present stays available. Missing ones may be downloaded later and
// are rechecked on each request.
type Available = BTreeSet<(PathBuf, Oid, String)>;
static AVAILABLE: LazyLock<Mutex<Available>> = LazyLock::new(Mutex::default);

/// A page's history needs only that page's attachments; project history needs
/// every attachment and theme file.
pub(super) fn attachments_available(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    page: Option<&str>,
    known: &mut AttachmentPresence,
) -> Result<bool> {
    let mut available = true;
    if let Some(id) = page {
        let directory = ItemId::try_from(id)?.asset_directory(Path::new(""));
        if let Ok(entry) = tree.get_path(&directory) {
            let prefix = directory.to_string_lossy().replace('\\', "/");
            available &= attachment_tree(repo, &repo.find_tree(entry.id())?, &prefix, known)?;
        }
        return Ok(available);
    }
    for name in ["assets", "themes"] {
        if let Some(entry) = tree.get_name(name) {
            available &= attachment_tree(repo, &repo.find_tree(entry.id())?, name, known)?;
        }
    }
    Ok(available)
}

fn attachment_tree(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &str,
    known: &mut AttachmentPresence,
) -> Result<bool> {
    if prefix.bytes().filter(|byte| *byte == b'/').count() > 16 {
        return Err(failure("Attachment tree is too deeply nested"));
    }
    let key = (tree.id(), prefix.to_owned());
    if let Some(available) = known.trees.get(&key) {
        return Ok(*available);
    }
    let shared = (repo.path().to_owned(), tree.id(), prefix.to_owned());
    let cache = || {
        AVAILABLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    if cache().contains(&shared) {
        return Ok(true);
    }
    let mut available = true;
    for entry in tree {
        let path = format!("{prefix}/{}", entry.name()?);
        if entry.kind() == Some(git2::ObjectType::Tree) {
            available &= attachment_tree(repo, &repo.find_tree(entry.id())?, &path, known)?;
            continue;
        }
        if repository::tracked(&path) != Some(repository::Kind::Binary) {
            continue;
        }
        let present = known.blobs.get(&entry.id()).map_or_else(
            || -> Result<bool> {
                let pointer = super::lfs::Pointer::parse(repo.find_blob(entry.id())?.content())?;
                Ok(pointer.path(repo.path()).try_exists()?)
            },
            |present| Ok(*present),
        )?;
        known.blobs.insert(entry.id(), present);
        available &= present;
    }
    known.trees.insert(key, available);
    if available {
        let mut cache = cache();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(shared);
    }
    Ok(available)
}
