// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical reusable packages. Definition updates replace one manifest atomically;
//! clones publish a complete staged directory and never discard authored assets.
use crate::domain::project::canonical_json;
use crate::domain::publication::{
    theme::{self, ThemeFiles, ThemeOrigin},
    theme_format::{Definition, Overrides, ResolvedTheme, Web},
};
use crate::export_jobs::error::Error;
use crate::store::{Store, theme_packages};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

type Result<T> = std::result::Result<T, Error>;

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
            .ok_or_else(|| Error::internal("Theme has no manifest.json."))?,
    )
    .map_err(|error| Error::internal(format!("Invalid theme manifest: {error}")))
}

fn storage(error: crate::store::Error) -> Error {
    match error {
        crate::store::Error::Invalid(message)=>Error::InvalidRequest(message),
        crate::store::Error::RevisionConflict=>Error::InvalidRequest("Version conflict: the theme package was modified by another writer. Reload it before saving.".into()),
        other=>Error::Database(other),
    }
}

/// Publications that use each theme, keyed by theme ID.
fn usages(root: &Path) -> Result<BTreeMap<String, Vec<Usage>>> {
    Store::shared(root).and_then(|store|store.read(|tx| {
        let mut statement=tx.prepare("SELECT json_extract(b.content_json,'$.themeId'),i.uuid,i.title,i.state='archived' FROM item i JOIN body b ON b.id=i.body_id WHERE i.type='publication' ORDER BY i.uuid")?;
        let mut rows=statement.query([])?;let mut result=BTreeMap::<String,Vec<Usage>>::new();
        while let Some(row)=rows.next()? {
            if let Some(theme)=row.get::<_,Option<String>>(0)? {
                result.entry(theme).or_default().push(Usage{id:row.get(1)?,title:row.get(2)?,archived:row.get(3)?});
            }
        }
        Ok(result)
    })).map_err(storage)
}

fn record(id: &str, origin: ThemeOrigin, files: &ThemeFiles, usages: Vec<Usage>) -> Result<Record> {
    let definition = definition(files)?;
    theme::validate_manifest(&definition, files, id).map_err(Error::internal)?;
    let assets = files
        .iter()
        .filter(|(path, _)| path.as_str() != "manifest.json")
        .map(|(path, bytes)| Asset {
            path: path.clone(),
            bytes: bytes.len(),
            sha256: crate::domain::publication::digest_hex(&sha2::Sha256::digest(bytes)),
        })
        .collect();
    Ok(Record {
        summary: Summary {
            id: id.into(),
            name: definition.name.clone(),
            version: definition.version.clone(),
            origin,
            revision: theme::hash_files(files),
            usages,
        },
        definition,
        assets,
    })
}

use sha2::Digest;

fn used_by(root: &Path, id: &str) -> Result<Vec<Usage>> {
    Ok(usages(root)?.remove(id).unwrap_or_default())
}

pub(crate) fn read(root: &Path, id: &str) -> Result<Record> {
    let (origin, files) =
        crate::export_jobs::theme::load_theme(root, id).map_err(Error::InvalidRequest)?;
    record(id, origin, &files, used_by(root, id)?)
}

pub(crate) fn list(root: &Path) -> Result<Vec<Summary>> {
    let mut ids = vec!["classic".to_owned(), "editorial".to_owned()];
    ids.extend(
        Store::shared(root)
            .and_then(|store| store.read(theme_packages::ids))
            .map_err(storage)?,
    );
    let mut used = usages(root)?;
    let mut result = Vec::new();
    for id in ids {
        let (origin, files) =
            crate::export_jobs::theme::load_theme(root, &id).map_err(Error::InvalidRequest)?;
        let definition = definition(&files)?;
        theme::validate_manifest(&definition, &files, &id).map_err(Error::internal)?;
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

fn check_revision(files: &ThemeFiles, revision: &str) -> Result<()> {
    if revision != theme::hash_files(files) {
        return Err(Error::InvalidRequest("Version conflict: the theme package was modified by another writer. Reload it before saving.".into()));
    }
    Ok(())
}

fn named(name: &str) -> Result<String> {
    if name.chars().any(char::is_control) {
        return Err(Error::InvalidRequest(
            "Theme names cannot contain control characters.".into(),
        ));
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(Error::InvalidRequest(
            "Enter a theme name of 1–80 characters without control characters.".into(),
        ));
    }
    Ok(name.into())
}

fn manifest(files: &mut ThemeFiles, definition: &Definition) -> Result<()> {
    named(&definition.name)?;
    let bytes = canonical_json::to_vec(definition)?;
    files.insert("manifest.json".into(), bytes);
    if files.len() > theme::MAX_THEME_FILES
        || files.values().map(Vec::len).sum::<usize>() > theme::MAX_THEME_BYTES
    {
        return Err(Error::InvalidRequest(
            "Theme exceeds the package size limit.".into(),
        ));
    }
    theme::validate_manifest(definition, files, &definition.id).map_err(Error::InvalidRequest)
}

pub(super) fn draft(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
    format: crate::domain::publication::plan::PublicationFormat,
) -> Result<(theme::ThemeSnapshot, ThemeFiles)> {
    let (origin, mut files) =
        crate::export_jobs::theme::load_theme(root, id).map_err(Error::InvalidRequest)?;
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(Error::InvalidRequest("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let snapshot =
        theme::capture_files(id, origin, &files, format).map_err(Error::InvalidRequest)?;
    Ok((snapshot, files))
}

fn publish(
    root: &Path,
    mut files: ThemeFiles,
    mut definition: Definition,
    name: &str,
    source: Option<(String, theme_packages::Stamp)>,
) -> Result<Record> {
    definition.id = format!("theme-t{}", Uuid::now_v7().simple());
    definition.name = named(name)?;
    manifest(&mut files, &definition)?;
    for (path, bytes) in &mut files {
        if Path::new(path).extension().is_some_and(|ext| ext == "json") {
            let value: serde_json::Value = serde_json::from_slice(bytes)?;
            *bytes = canonical_json::to_vec(&value)?;
        }
    }
    // A new package has no publications yet.
    let result = record(&definition.id, ThemeOrigin::Project, &files, Vec::new())?;
    theme_packages::save(root, &definition.id, None, source, &files).map_err(storage)?;
    Ok(result)
}

pub(crate) fn create(root: &Path, name: &str) -> Result<Record> {
    let files =
        theme::builtin_theme("classic").ok_or_else(|| Error::internal("Missing Classic theme."))?;
    let mut definition = definition(&files)?;
    definition.web = Web::default();
    publish(root, ThemeFiles::new(), definition, name, None)
}

pub(crate) fn duplicate(
    root: &Path,
    id: &str,
    revision: &str,
    name: &str,
    draft: Option<&Definition>,
) -> Result<Record> {
    let (source, files) = if let Some(files) = theme::builtin_theme(id) {
        (None, files)
    } else {
        let (stamp, files) = theme_packages::load(root, id).map_err(storage)?;
        (Some((id.to_owned(), stamp)), files)
    };
    check_revision(&files, revision)?;
    let definition = if let Some(draft) = draft {
        if draft.id != id {
            return Err(Error::InvalidRequest("Theme IDs are immutable.".into()));
        }
        draft.clone()
    } else {
        definition(&files)?
    };
    publish(root, files, definition, name, source)
}

pub(crate) fn validate(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
) -> Result<Validation> {
    let (_, mut files) =
        crate::export_jobs::theme::load_theme(root, id).map_err(Error::InvalidRequest)?;
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(Error::InvalidRequest("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let resolved = crate::domain::publication::plan::PublicationFormat::ALL
        .into_iter()
        .map(|format| {
            let target = format.name();
            crate::domain::publication::theme_format::resolve(
                definition,
                target,
                &Overrides::default(),
            )
            .map(|value| (target.into(), value))
            .map_err(Error::InvalidRequest)
        })
        .collect::<Result<_>>()?;
    Ok(Validation {
        resolved,
        execution_sha256: theme::execution_hash(definition, &files).map_err(Error::internal)?,
    })
}

pub(crate) fn update(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
) -> Result<Record> {
    if theme::is_builtin(id) {
        return Err(Error::InvalidRequest(
            "Built-in themes are read-only. Duplicate the theme to edit it.".into(),
        ));
    }
    let (stamp, mut files) = theme_packages::load(root, id).map_err(storage)?;
    check_revision(&files, revision)?;
    if definition.id != id {
        return Err(Error::InvalidRequest("Theme IDs are immutable.".into()));
    }
    manifest(&mut files, definition)?;
    let result = record(id, ThemeOrigin::Project, &files, used_by(root, id)?)?;
    theme_packages::update_manifest(root, id, stamp, serde_json::to_value(definition)?)
        .map_err(storage)?;
    Ok(result)
}

pub(crate) fn delete(root: &Path, id: &str, revision: &str) -> Result<()> {
    if theme::is_builtin(id) {
        return Err(Error::InvalidRequest(
            "Built-in themes cannot be deleted.".into(),
        ));
    }
    let (stamp, files) = theme_packages::load(root, id).map_err(storage)?;
    check_revision(&files, revision)?;
    theme_packages::delete(root, id, stamp).map_err(storage)
}
