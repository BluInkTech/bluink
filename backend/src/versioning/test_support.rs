// SPDX-License-Identifier: MPL-2.0
//! Test fixtures use the same canonical transactions as the application.
use crate::store::{
    self, Store, git,
    io::{self, LockMode},
    item, mutations,
};
use serde_json::{Value, json};
use std::{
    io::{Seek, Write},
    path::{Path, PathBuf},
};

/// The production checkpoint under the history lease, with a fixed device.
pub(crate) fn checkpoint(
    root: &Path,
    store: &Store,
    device_id: &str,
    device_name: &str,
) -> super::Result<Option<git2::Oid>> {
    let repo = super::checkpoints::open(root)?;
    let _lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| super::failure(format!("{error:#}")))?;
    super::checkpoints::checkpoint_locked(&repo, store, device_id, device_name)
}

/// The production named-version publication after a fixed-device checkpoint.
pub(crate) fn named_version(
    root: &Path,
    store: &Store,
    request: &super::model::NamedVersionRequest,
    device_id: &str,
    device_name: &str,
) -> super::Result<super::model::NamedVersion> {
    let repo = super::checkpoints::open(root)?;
    let _lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
        .map_err(|error| super::failure(format!("{error:#}")))?;
    super::checkpoints::checkpoint_locked(&repo, store, device_id, device_name)?;
    super::checkpoints::name_head(&repo, store, request)
}

pub(super) fn create(root: &Path, timezone: &str) -> store::Result<Value> {
    let template =
        serde_json::from_slice(include_bytes!("../../resources/workspace-starter.json"))?;
    store::project::open_or_create(root, timezone, &template).map(|(_, config)| config)
}
pub(super) fn read(root: &Path, id: &str) -> store::Result<Option<Value>> {
    Store::shared(root)?.read(|tx| item::read(tx, id, true))
}
pub(super) fn save(
    root: &Path,
    input: Value,
    expected: Option<&str>,
    _day: &str,
) -> store::Result<Value> {
    let store = Store::shared(root)?;
    let previous = input["id"]
        .as_str()
        .map(|id| read(root, id))
        .transpose()?
        .flatten();
    let value = crate::store::commands::assemble_owned(input, previous.as_ref(), "UTC")
        .map_err(store::Error::Invalid)?;
    if previous
        .as_ref()
        .and_then(|value| value["revision"].as_str())
        != expected
    {
        return Err(store::Error::RevisionConflict);
    }
    let change = mutations::PreparedChange::new(value.clone(), expected.map(str::to_owned))?;
    store.write(move |tx| mutations::apply(tx, &[change], &[], None))?;
    Ok(json!({"item":value}))
}
pub(super) fn asset(root: &Path, id: &str, name: &str, bytes: &[u8]) -> store::Result<()> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    let verified = super::checkpoints::install_asset(root, &mut file)
        .map_err(|error| store::Error::Invalid(error.to_string()))?;
    let path = crate::domain::project::identity::ItemId::try_from(id)
        .map_err(store::Error::Invalid)?
        .asset_directory(Path::new(""))
        .join(name)
        .to_string_lossy()
        .replace('\\', "/");
    let id = id.to_owned();
    Store::shared(root)?.write(move |tx| {
        let owner = tx.query_row("SELECT id FROM entity WHERE uuid=?1", [id], |row| {
            row.get(0)
        })?;
        let generation = tx.query_row(
            "UPDATE project_state SET generation=generation+1 RETURNING generation",
            [],
            |row| row.get(0),
        )?;
        git::put_asset(tx, &path, Some(owner), &verified, generation)
    })
}
pub(super) fn asset_file(root: &Path, id: &str, name: &str) -> store::Result<PathBuf> {
    let path = crate::domain::project::identity::ItemId::try_from(id)
        .map_err(store::Error::Invalid)?
        .asset_directory(Path::new(""))
        .join(name)
        .to_string_lossy()
        .replace('\\', "/");
    let (oid, size) = Store::shared(root)?.read(|tx| {
        Ok(tx.query_row::<(String, i64), _, _>(
            "SELECT a.oid,a.size FROM asset a JOIN asset_path p ON p.oid=a.oid WHERE p.path=?1",
            [path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    })?;
    let repo =
        super::checkpoints::open(root).map_err(|error| store::Error::Invalid(error.to_string()))?;
    super::lfs::Pointer::from_identity(
        &oid,
        u64::try_from(size).map_err(|_| store::Error::Invalid("Invalid asset size".into()))?,
    )
    .and_then(|pointer| pointer.verified_path(repo.path()))
    .map_err(|error| store::Error::Invalid(error.to_string()))
}
