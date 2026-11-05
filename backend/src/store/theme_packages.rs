// SPDX-License-Identifier: MPL-2.0
//! Snapshot theme mappings together. Package validation and binary reads happen
//! before the writer, which rechecks every affected mapping before publication.
use super::{Error, Result, Store, assets, changes, git};
use crate::domain::publication::theme::{MAX_THEME_BYTES, MAX_THEME_FILES};
use crate::{
    domain::project::canonical_json, store::changes::ChangeDomain,
    versioning::checkpoints::VerifiedAsset,
};
use rusqlite::{Transaction, params};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) type Files = BTreeMap<String, Vec<u8>>;
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Stamp(Vec<(String, String)>);

fn prefix(id: &str) -> Result<String> {
    if !crate::domain::publication::theme::is_theme_id(id) {
        return Err(Error::Invalid("Invalid theme identity.".into()));
    }
    Ok(format!("themes/{id}/"))
}
fn stamp(tx: &Transaction<'_>, id: &str) -> Result<Stamp> {
    let pattern = format!("{}*", prefix(id)?);
    let rows=tx.prepare("SELECT path,revision FROM project_record WHERE path GLOB ?1 UNION ALL SELECT path,oid FROM asset_path WHERE path GLOB ?1 ORDER BY path")?.query_map([pattern],|row|Ok((row.get(0)?,row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(Stamp(rows))
}

pub(crate) fn load(root: &Path, id: &str) -> Result<(Stamp, Files)> {
    let prefix = prefix(id)?;
    let store = Store::shared(root)?;
    let (version, mut files, objects) = store.read(|tx| {
        let version = stamp(tx, id)?;
        if version.0.is_empty() {
            return Err(Error::Invalid(format!(
                "Publication theme {id} does not exist."
            )));
        }
        if version.0.len() > MAX_THEME_FILES {
            return Err(Error::Invalid("Theme has too many files.".into()));
        }
        let mut files = Files::new();
        let mut objects = Vec::new();
        let mut size = 0usize;
        for (path, _) in &version.0 {
            let key = path
                .strip_prefix(&prefix)
                .ok_or_else(|| Error::corrupt("Theme path escaped its package."))?
                .to_owned();
            if !crate::domain::publication::theme::safe_theme_path(&key) {
                return Err(Error::corrupt("Unsafe theme package path."));
            }
            if let Some(object) = assets::mapping(tx, path)? {
                size = size.saturating_add(usize::try_from(object.size).unwrap_or(usize::MAX));
                objects.push((key, object));
            } else {
                let value: String = tx.query_row(
                    "SELECT canonical_json FROM project_record WHERE path=?1",
                    [path],
                    |row| row.get(0),
                )?;
                let bytes = canonical_json::to_vec(&serde_json::from_str::<Value>(&value)?)?;
                size = size.saturating_add(bytes.len());
                files.insert(key, bytes);
            }
            if size > MAX_THEME_BYTES {
                return Err(Error::Invalid(
                    "Theme files exceed the 16 MiB package limit.".into(),
                ));
            }
        }
        Ok((version, files, objects))
    })?;
    for (key, object) in objects {
        files.insert(key, object.read_verified(root, MAX_THEME_BYTES as u64)?);
    }
    Ok((version, files))
}

pub(crate) fn ids(tx: &Transaction<'_>) -> Result<Vec<String>> {
    let paths=tx.prepare("SELECT path FROM project_record WHERE path GLOB 'themes/*' UNION ALL SELECT path FROM asset_path WHERE path GLOB 'themes/*'")?.query_map([],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let ids = paths
        .iter()
        .filter_map(|path| theme_id(path))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    Ok(ids.into_iter().collect())
}

fn theme_id(path: &str) -> Option<&str> {
    path.strip_prefix("themes/")?
        .split_once('/')
        .map(|(id, _)| id)
}

enum DeltaFile {
    Bytes(Vec<u8>),
    Object(assets::Object),
}
impl DeltaFile {
    fn size(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            Self::Object(object) => object.size,
        }
    }
    fn read(self, root: &Path) -> Result<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            Self::Object(object) => object.read_verified(root, MAX_THEME_BYTES as u64),
        }
    }
}

/// Validate the exact package produced by the incoming paths, keeping local
/// files unchanged unless the candidate explicitly replaces or removes them.
pub(crate) fn validate_delta(
    root: &Path,
    records: &[(String, Option<Value>)],
    assets: &[(String, Option<VerifiedAsset>)],
) -> Result<()> {
    let changed = records
        .iter()
        .map(|(path, _)| path)
        .chain(assets.iter().map(|(path, _)| path))
        .filter_map(|path| theme_id(path))
        .collect::<BTreeSet<_>>();
    if changed.is_empty() {
        return Ok(());
    }
    let store = Store::shared(root)?;
    for id in changed {
        let prefix = prefix(id)?;
        let mut files = if store.read(|tx| stamp(tx, id))?.0.is_empty() {
            BTreeMap::new()
        } else {
            load(root, id)?
                .1
                .into_iter()
                .map(|(name, bytes)| (name, DeltaFile::Bytes(bytes)))
                .collect()
        };
        for (path, value) in records {
            if let Some(name) = path.strip_prefix(&prefix) {
                if let Some(value) = value {
                    files.insert(
                        name.to_owned(),
                        DeltaFile::Bytes(canonical_json::to_vec(value)?),
                    );
                } else {
                    files.remove(name);
                }
            }
        }
        for (path, value) in assets {
            if let Some(name) = path.strip_prefix(&prefix) {
                if let Some(asset) = value {
                    files.insert(
                        name.to_owned(),
                        DeltaFile::Object(assets::Object {
                            oid: asset.oid().to_owned(),
                            size: asset.size(),
                        }),
                    );
                } else {
                    files.remove(name);
                }
            }
        }
        if files.is_empty() {
            continue;
        }
        if files.len() > MAX_THEME_FILES
            || files
                .values()
                .try_fold(0u64, |sum, file| sum.checked_add(file.size()))
                .is_none_or(|size| size > MAX_THEME_BYTES as u64)
        {
            return Err(Error::Invalid(
                "Theme exceeds the 16 MiB package limit.".into(),
            ));
        }
        let files = files
            .into_iter()
            .map(|(name, file)| Ok((name, file.read(root)?)))
            .collect::<Result<Files>>()?;
        crate::domain::publication::theme::validate_package(id, &files).map_err(Error::Invalid)?;
    }
    Ok(())
}

pub(crate) fn guard_deleted_records(
    tx: &Transaction<'_>,
    records: &[(String, Option<Value>)],
) -> Result<()> {
    for (path, value) in records {
        if value.is_some() {
            continue;
        }
        if let Some(id) = theme_id(path)
            && path == &format!("themes/{id}/manifest.json")
        {
            require_unused(tx, id)?;
        }
    }
    Ok(())
}

fn require_unused(tx: &Transaction<'_>, id: &str) -> Result<()> {
    let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM item i JOIN body b ON b.id=i.body_id WHERE i.type='publication' AND json_extract(b.content_json,'$.themeId')=?1)",[id],|row|row.get(0))?;
    if used {
        return Err(Error::Invalid(
            "This theme is used by saved or archived publications and cannot be deleted.".into(),
        ));
    }
    Ok(())
}

enum Prepared {
    Record(Value),
    Asset(VerifiedAsset),
}

pub(crate) fn save(
    root: &Path,
    id: &str,
    expected: Option<Stamp>,
    source: Option<(String, Stamp)>,
    files: &Files,
) -> Result<()> {
    if crate::domain::publication::theme::is_builtin(id) {
        return Err(Error::Invalid("Built-in theme IDs are reserved.".into()));
    }
    if files.len() > MAX_THEME_FILES
        || files
            .values()
            .try_fold(0usize, |size, bytes| size.checked_add(bytes.len()))
            .is_none_or(|size| size > MAX_THEME_BYTES)
    {
        return Err(Error::Invalid(
            "Theme exceeds the 16 MiB package limit.".into(),
        ));
    }
    let prefix = prefix(id)?;
    let store = Store::shared(root)?;
    let mut prepared = BTreeMap::new();
    for (name, bytes) in files {
        if !crate::domain::publication::theme::safe_theme_path(name) {
            return Err(Error::Invalid("Unsafe theme package path.".into()));
        }
        let entry = if Path::new(name).extension().is_some_and(|ext| ext == "json") {
            Prepared::Record(serde_json::from_slice(bytes)?)
        } else {
            Prepared::Asset(assets::install_bytes(root, bytes)?)
        };
        prepared.insert(format!("{prefix}{name}"), entry);
    }
    let id = id.to_owned();
    store.write(move |tx| {
        if let Some((source, expected)) = source
            && stamp(tx, &source)? != expected
        {
            return Err(Error::RevisionConflict);
        }
        let current = stamp(tx, &id)?;
        if expected
            .as_ref()
            .map_or(!current.0.is_empty(), |expected| expected != &current)
        {
            return Err(Error::RevisionConflict);
        }
        let generation = changed(tx)?;
        for (path, _) in current.0 {
            if !prepared.contains_key(&path) {
                remove(tx, &path, generation)?;
            }
        }
        for (path, entry) in prepared {
            match entry {
                Prepared::Record(value) => git::put_record(
                    tx,
                    &path,
                    &value,
                    &uuid::Uuid::now_v7().to_string(),
                    generation,
                )?,
                Prepared::Asset(asset) => {
                    if assets::mapping(tx, &path)?.is_none_or(|object| object.oid != asset.oid()) {
                        git::put_asset(tx, &path, None, &asset, generation)?;
                    }
                }
            }
        }
        Ok(())
    })
}

pub(crate) fn update_manifest(root: &Path, id: &str, expected: Stamp, value: Value) -> Result<()> {
    let path = format!("{}manifest.json", prefix(id)?);
    let id = id.to_owned();
    Store::shared(root)?.write(move |tx| {
        if stamp(tx, &id)? != expected {
            return Err(Error::RevisionConflict);
        }
        let generation = changed(tx)?;
        git::put_record(
            tx,
            &path,
            &value,
            &uuid::Uuid::now_v7().to_string(),
            generation,
        )
    })
}

pub(crate) fn require_reference(tx: &Transaction<'_>, item: &Value) -> Result<()> {
    if item["meta"]["type"] != "publication" {
        return Ok(());
    }
    let id = item["content"]["themeId"]
        .as_str()
        .ok_or_else(|| Error::Invalid("Publication has no theme ID.".into()))?;
    if crate::domain::publication::theme::is_builtin(id) {
        return Ok(());
    }
    let path = format!("{}manifest.json", prefix(id)?);
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_record WHERE path=?1)",
        [path],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(Error::Invalid(format!(
            "Publication theme {id} does not exist."
        )));
    }
    Ok(())
}

pub(crate) fn delete(root: &Path, id: &str, expected: Stamp) -> Result<()> {
    let id = id.to_owned();
    Store::shared(root)?.write(move |tx| {
        let current = stamp(tx, &id)?;
        if current != expected {
            return Err(Error::RevisionConflict);
        }
        require_unused(tx, &id)?;
        let generation = changed(tx)?;
        for (path, _) in current.0 {
            remove(tx, &path, generation)?;
        }
        Ok(())
    })
}
fn changed(tx: &Transaction<'_>) -> Result<i64> {
    changes::publish(
        tx,
        &BTreeSet::from([ChangeDomain::Settings, ChangeDomain::History]),
        &[],
    )
}
fn remove(tx: &Transaction<'_>, path: &str, generation: i64) -> Result<()> {
    tx.execute("DELETE FROM project_record WHERE path=?1", params![path])?;
    tx.execute("DELETE FROM asset_path WHERE path=?1", params![path])?;
    git::mark_deleted(tx, path, generation)
}

#[cfg(test)]
pub(crate) fn install_fixture(root: &Path, id: &str, directory: &Path) {
    if !root.join(super::DATABASE_NAME).exists() {
        Store::create(root, uuid::Uuid::now_v7()).unwrap();
    }
    crate::versioning::checkpoints::initialize(root).unwrap();
    let store = Store::shared(root).unwrap();
    let previous = store.read(|tx| stamp(tx, id)).unwrap();
    let mut files = Files::new();
    let mut folders = vec![directory.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                folders.push(path);
            } else {
                let name = path
                    .strip_prefix(directory)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                files.insert(name, std::fs::read(path).unwrap());
            }
        }
    }
    save(
        root,
        id,
        (!previous.0.is_empty()).then_some(previous),
        None,
        &files,
    )
    .unwrap();
}
