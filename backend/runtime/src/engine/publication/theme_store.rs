// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical reusable packages. Definition updates replace one manifest atomically;
//! clones publish a complete staged directory and never discard authored assets.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use super::theme::{self, ThemeFiles, ThemeOrigin};
use super::theme_format::{Definition, Overrides, ResolvedTheme, Web};
use crate::engine::project::{
    io::write_bytes_atomic,
    item::{list_project_item_ids, read_project_item},
    write::{MutationError, lock_relationships},
};

type Result<T> = std::result::Result<T, MutationError>;

#[cfg(test)]
mod tests;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeUsage"))]
pub(crate) struct Usage {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) archived: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeSummary"))]
pub(crate) struct Summary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) origin: ThemeOrigin,
    pub(crate) revision: String,
    pub(crate) usages: Vec<Usage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeAsset"))]
pub(crate) struct Asset {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeRecord"))]
pub(crate) struct Record {
    pub(crate) summary: Summary,
    pub(crate) definition: Definition,
    pub(crate) assets: Vec<Asset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeValidation"))]
pub(crate) struct Validation {
    pub(crate) resolved: BTreeMap<String, ResolvedTheme>,
    pub(crate) execution_sha256: String,
}

fn definition(files: &ThemeFiles) -> Result<Definition> {
    serde_json::from_slice(
        files
            .get("manifest.json")
            .ok_or_else(|| MutationError::Internal("Theme has no manifest.json.".into()))?,
    )
    .map_err(|error| MutationError::Internal(format!("Invalid theme manifest: {error}")))
}

fn usages(root: &Path, candidates: Option<Vec<String>>) -> Result<BTreeMap<String, Vec<Usage>>> {
    usages_in(
        root,
        match candidates {
            Some(ids) => ids,
            None => list_project_item_ids(root).map_err(MutationError::Internal)?,
        },
    )
}

fn usages_in(
    root: &Path,
    ids: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, Vec<Usage>>> {
    let mut result = BTreeMap::<String, Vec<Usage>>::new();
    for id in ids {
        if let Some(item) = read_project_item(root, &id).map_err(MutationError::Read)?
            && item["meta"]["type"] == "publication"
            && let Some(theme) = item["content"]["themeId"].as_str()
        {
            result.entry(theme.into()).or_default().push(Usage {
                id,
                title: item["meta"]["title"].as_str().unwrap_or("").into(),
                archived: item["meta"]["state"] == "archived",
            });
        }
    }
    Ok(result)
}

fn record(
    root: &Path,
    id: &str,
    origin: ThemeOrigin,
    files: &ThemeFiles,
    candidates: Option<Vec<String>>,
) -> Result<Record> {
    let definition = definition(files)?;
    theme::validate_manifest(&definition, files, id).map_err(MutationError::Internal)?;
    let assets = files
        .iter()
        .filter(|(path, _)| path.as_str() != "manifest.json")
        .map(|(path, bytes)| Asset {
            path: path.clone(),
            bytes: bytes.len(),
            sha256: super::digest_hex(&sha2::Sha256::digest(bytes)),
        })
        .collect();
    Ok(Record {
        summary: Summary {
            id: id.into(),
            name: definition.name.clone(),
            version: definition.version.clone(),
            origin,
            revision: theme::hash_files(files),
            usages: usages(root, candidates)?.remove(id).unwrap_or_default(),
        },
        definition,
        assets,
    })
}

use sha2::Digest;

pub(crate) fn read(root: &Path, id: &str, candidates: Option<Vec<String>>) -> Result<Record> {
    themes_directory(root, false)?;
    let (origin, files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    record(root, id, origin, &files, candidates)
}

pub(crate) fn list(root: &Path, candidates: Option<Vec<String>>) -> Result<Vec<Summary>> {
    let directory = themes_directory(root, false)?;
    let mut ids = vec!["classic".to_owned(), "editorial".to_owned()];
    if directory.exists() {
        for entry in
            fs::read_dir(directory).map_err(|error| MutationError::Internal(error.to_string()))?
        {
            let entry = entry.map_err(|error| MutationError::Internal(error.to_string()))?;
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| MutationError::Validation("Theme paths must be UTF-8.".into()))?;
            // Only our hidden atomic staging directories are outside the catalogue.
            if id.starts_with(".theme-") {
                continue;
            }
            if !theme::is_theme_id(&id) {
                return Err(MutationError::Validation(format!(
                    "Invalid theme folder: {id}"
                )));
            }
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let mut used = usages(root, candidates)?;
    let mut result = Vec::new();
    for id in ids {
        let (origin, files) = theme::load_theme(root, &id).map_err(MutationError::Validation)?;
        let definition = definition(&files)?;
        theme::validate_manifest(&definition, &files, &id).map_err(MutationError::Internal)?;
        result.push(Summary {
            id: id.clone(),
            name: definition.name,
            version: definition.version,
            origin,
            revision: theme::hash_files(&files),
            usages: used.remove(&id).unwrap_or_default(),
        });
    }
    result.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(result)
}

fn themes_directory(root: &Path, create: bool) -> Result<PathBuf> {
    let directory = root.join("themes");
    for path in [root, directory.as_path()] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(MutationError::Validation(
                    "Project and themes folders must be real directories.".into(),
                ));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && path == directory => (),
            Err(error) => return Err(MutationError::Internal(error.to_string())),
        }
    }
    if create {
        fs::create_dir_all(&directory)
            .map_err(|error| MutationError::Internal(error.to_string()))?;
    }
    Ok(directory)
}

fn check_revision(files: &ThemeFiles, revision: &str) -> Result<()> {
    if revision != theme::hash_files(files) {
        return Err(MutationError::RevisionConflict("Version conflict: the theme package was modified by another writer. Reload it before saving.".into()));
    }
    Ok(())
}

fn named(name: &str) -> Result<String> {
    if name.chars().any(char::is_control) {
        return Err(MutationError::Validation(
            "Theme names cannot contain control characters.".into(),
        ));
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(MutationError::Validation(
            "Enter a theme name of 1–80 characters without control characters.".into(),
        ));
    }
    Ok(name.into())
}

fn manifest(files: &mut ThemeFiles, definition: &Definition) -> Result<()> {
    named(&definition.name)?;
    let mut bytes = serde_json::to_vec_pretty(definition).map_err(MutationError::Serialization)?;
    bytes.push(b'\n');
    files.insert("manifest.json".into(), bytes);
    if files.len() > theme::MAX_THEME_FILES
        || files.values().map(Vec::len).sum::<usize>() > theme::MAX_THEME_BYTES
    {
        return Err(MutationError::Validation(
            "Theme exceeds the package size limit.".into(),
        ));
    }
    theme::validate_manifest(definition, files, &definition.id).map_err(MutationError::Validation)
}

pub(super) fn draft(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
    format: super::plan::PublicationFormat,
) -> Result<(theme::ThemeSnapshot, ThemeFiles)> {
    themes_directory(root, false)?;
    let (origin, mut files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(MutationError::Validation("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let snapshot =
        theme::capture_files(id, origin, &files, format).map_err(MutationError::Validation)?;
    Ok((snapshot, files))
}

pub(super) fn write_files(directory: &Path, files: &ThemeFiles) -> Result<()> {
    for (relative, bytes) in files {
        if !theme::safe_theme_path(relative) {
            return Err(MutationError::Validation(format!(
                "Unsafe theme path: {relative}"
            )));
        }
        let path = directory.join(relative);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| MutationError::Internal("Theme file has no parent.".into()))?,
        )
        .map_err(|error| MutationError::Internal(error.to_string()))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| MutationError::Internal(error.to_string()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| MutationError::Internal(error.to_string()))?;
    }
    Ok(())
}

fn publish(
    root: &Path,
    mut files: ThemeFiles,
    mut definition: Definition,
    name: &str,
) -> Result<Record> {
    definition.id = format!("theme-t{}", Uuid::now_v7().simple());
    definition.name = named(name)?;
    manifest(&mut files, &definition)?;
    let directory = themes_directory(root, true)?;
    let staging = tempfile::Builder::new()
        .prefix(".theme-stage-")
        .tempdir_in(&directory)
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    write_files(staging.path(), &files)?;
    // A fresh identity cannot be referenced by an existing publication.
    let result = record(
        root,
        &definition.id,
        ThemeOrigin::Project,
        &files,
        Some(Vec::new()),
    )?;
    fs::rename(staging.path(), directory.join(&definition.id))
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    Ok(result)
}

pub(crate) fn create(root: &Path, name: &str) -> Result<Record> {
    themes_directory(root, false)?;
    let _lock = lock_relationships(root).map_err(MutationError::Storage)?;
    let files = theme::builtin_theme("classic")
        .ok_or_else(|| MutationError::Internal("Missing Classic theme.".into()))?;
    let mut definition = definition(&files)?;
    definition.web = Web::default();
    publish(root, ThemeFiles::new(), definition, name)
}

pub(crate) fn duplicate(
    root: &Path,
    id: &str,
    revision: &str,
    name: &str,
    draft: Option<&Definition>,
) -> Result<Record> {
    themes_directory(root, false)?;
    let _lock = lock_relationships(root).map_err(MutationError::Storage)?;
    let (_, files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    check_revision(&files, revision)?;
    let definition = if let Some(draft) = draft {
        if draft.id != id {
            return Err(MutationError::Validation("Theme IDs are immutable.".into()));
        }
        draft.clone()
    } else {
        definition(&files)?
    };
    publish(root, files, definition, name)
}

pub(crate) fn validate(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
) -> Result<Validation> {
    themes_directory(root, false)?;
    let (_, mut files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(MutationError::Validation("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let resolved = super::plan::PublicationFormat::ALL
        .into_iter()
        .map(|format| {
            let target = format.name();
            super::theme_format::resolve(definition, target, &Overrides::default())
                .map(|value| (target.into(), value))
                .map_err(MutationError::Validation)
        })
        .collect::<Result<_>>()?;
    Ok(Validation {
        resolved,
        execution_sha256: theme::execution_hash(definition, &files)
            .map_err(MutationError::Internal)?,
    })
}

pub(crate) fn update(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
) -> Result<Record> {
    themes_directory(root, false)?;
    let _lock = lock_relationships(root).map_err(MutationError::Storage)?;
    let (origin, mut files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    if origin == ThemeOrigin::Builtin {
        return Err(MutationError::Validation(
            "Built-in themes are read-only. Duplicate the theme to edit it.".into(),
        ));
    }
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(MutationError::Validation("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let result = record(root, id, origin, &files, None)?;
    write_bytes_atomic(
        &root.join("themes").join(id).join("manifest.json"),
        &files["manifest.json"],
    )
    .map_err(MutationError::Storage)?;
    Ok(result)
}

pub(crate) fn delete(root: &Path, id: &str, revision: &str) -> Result<()> {
    themes_directory(root, false)?;
    let _lock = lock_relationships(root).map_err(MutationError::Storage)?;
    let (origin, files) = theme::load_theme(root, id).map_err(MutationError::Validation)?;
    if origin == ThemeOrigin::Builtin {
        return Err(MutationError::Validation(
            "Built-in themes cannot be deleted.".into(),
        ));
    }
    check_revision(&files, revision)?;
    if usages(root, None)?
        .get(id)
        .is_some_and(|used| !used.is_empty())
    {
        return Err(MutationError::Validation(
            "This theme is used by saved or archived publications and cannot be deleted.".into(),
        ));
    }
    let directory = themes_directory(root, false)?;
    let removed = tempfile::Builder::new()
        .prefix(".theme-removed-")
        .tempdir_in(&directory)
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    // Remove the catalogue entry atomically before reclaiming the owned package.
    fs::rename(directory.join(id), removed.path().join("package"))
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    removed
        .close()
        .map_err(|error| MutationError::Internal(error.to_string()))
}

/// Called under the same relationship lock as deletion and canonical item saves.
pub(crate) fn validate_publication_reference(
    root: &Path,
    item: &serde_json::Value,
) -> std::result::Result<(), String> {
    if item["meta"]["type"] == "publication" {
        let id = item["content"]["themeId"]
            .as_str()
            .ok_or("Publication has no theme ID.")?;
        theme::load_theme(root, id)?;
    }
    Ok(())
}
