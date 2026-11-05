// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use crate::{
    domain::publication::{self, theme},
    versioning::{Result, failure, lfs, repository},
};
use git2::{Oid, Repository};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn config_at(repo: &Repository, commit: Oid) -> Result<Value> {
    use crate::domain::project::config;
    let entry = repo
        .find_commit(commit)?
        .tree()?
        .get_path(std::path::Path::new("bluink.json"))
        .map_err(|_| failure("The destination is not a Bluink project"))?;
    let blob = repo.find_blob(entry.id())?;
    config::parse_project_config(blob.content()).map_err(|message| {
        let header = serde_json::from_slice::<Value>(blob.content()).ok();
        if header.is_some_and(|v| {
            v["format"] == "bluink-project"
                && v["formatVersion"]
                    .as_u64()
                    .is_some_and(|n| n > config::PROJECT_FORMAT_VERSION)
        }) {
            failure("This project needs a newer Bluink version.").classified("needs_newer_bluink")
        } else {
            failure(message)
        }
    })
}

type Themes = BTreeMap<String, BTreeMap<String, Oid>>;

fn theme_files(files: &BTreeMap<String, Oid>) -> Result<Themes> {
    let mut themes = Themes::new();
    for (path, oid) in files {
        if let Some(rest) = path.strip_prefix("themes/") {
            let (id, file) = rest
                .split_once('/')
                .ok_or_else(|| failure("Invalid theme path"))?;
            themes
                .entry(id.into())
                .or_default()
                .insert(file.into(), *oid);
        }
    }
    Ok(themes)
}

fn theme_package(repo: &Repository, id: &str, entries: &BTreeMap<String, Oid>) -> Result<()> {
    let mut bytes = BTreeMap::new();
    let mut size = 0usize;
    for (name, oid) in entries {
        let blob = repo.find_blob(*oid)?;
        let content = if repository::tracked(&format!("themes/{id}/{name}"))
            == Some(repository::Kind::Binary)
        {
            let pointer = lfs::Pointer::parse(blob.content())?;
            if pointer.fingerprint().1 > 16 * 1024 * 1024 {
                return Err(failure("Theme file exceeds its size limit"));
            }
            let mut content = Vec::new();
            pointer.copy_verified(repo.path(), &mut content)?;
            content
        } else {
            blob.content().to_vec()
        };
        size = size
            .checked_add(content.len())
            .ok_or_else(|| failure("Theme size overflow"))?;
        if size > 16 * 1024 * 1024 || bytes.len() >= 256 {
            return Err(failure("Theme package exceeds its size or file limit"));
        }
        bytes.insert(name.clone(), content);
    }
    theme::validate_package(id, &bytes)?;
    Ok(())
}

fn publication_theme(repo: &Repository, themes: &Themes, oid: Oid) -> Result<()> {
    let value: Value = serde_json::from_slice(repo.find_blob(oid)?.content())?;
    if value["meta"]["type"] != "publication" {
        return Ok(());
    }
    let publication: publication::PublicationContent =
        serde_json::from_value(value["content"].clone())?;
    if !themes.contains_key(&publication.theme_id) && !theme::is_builtin(&publication.theme_id) {
        return Err(failure(format!(
            "Publication {} references an unavailable theme",
            value["id"]
        )));
    }
    Ok(())
}

pub(crate) fn packages(repo: &Repository, files: &BTreeMap<String, Oid>) -> Result<()> {
    let themes = theme_files(files)?;
    for (id, entries) in &themes {
        theme_package(repo, id, entries)?;
    }
    for (path, oid) in files {
        if path.starts_with("items/") {
            publication_theme(repo, &themes, *oid)?;
        }
    }
    Ok(())
}

/// `base` passed `packages`. Revalidate changed theme packages and changed
/// items; removing a theme rechecks every publication.
pub(crate) fn changed_packages(
    repo: &Repository,
    files: &BTreeMap<String, Oid>,
    base: &BTreeMap<String, Oid>,
) -> Result<()> {
    let themes = theme_files(files)?;
    let previous = theme_files(base)?;
    for (id, entries) in &themes {
        if previous.get(id) != Some(entries) {
            theme_package(repo, id, entries)?;
        }
    }
    let removed = previous.keys().any(|id| !themes.contains_key(id));
    for (path, oid) in files {
        if path.starts_with("items/") && (removed || base.get(path) != Some(oid)) {
            publication_theme(repo, &themes, *oid)?;
        }
    }
    Ok(())
}
