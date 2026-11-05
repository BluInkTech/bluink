// SPDX-License-Identifier: MPL-2.0
//! Export one saved generation without keeping a reader on the live WAL.
use crate::{
    domain::project::{canonical_json, identity::ItemId},
    store::{Error, Result, Store, item, maintenance, snapshot},
    versioning::checkpoints,
};
use rusqlite::{Connection, OpenFlags, Transaction};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Export {
    generation: i64,
    items: u64,
    records: u64,
    assets: u64,
    portable_bytes: u64,
    snapshot_bytes: u64,
}

pub(crate) fn export(store: &Store, destination: &Path) -> Result<Export> {
    if destination.try_exists()? {
        return Err(Error::Invalid(
            "A portable export requires a new destination folder.".into(),
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error::Invalid("Export destination has no parent.".into()))?;
    fs::create_dir_all(parent)?;
    let root = store
        .path()
        .parent()
        .ok_or_else(|| Error::Invalid("Project database has no parent.".into()))?;
    // Private snapshot bytes always stay in the local project, even when the
    // portable tree is exported to a cloud or removable destination.
    let private = tempfile::Builder::new()
        .prefix(".export-snapshot-")
        .tempdir_in(root)?;
    let stage = tempfile::Builder::new()
        .prefix(".bluink-export-")
        .tempdir_in(parent)?;
    let pin = maintenance::protect(store, "export", destination)?;
    let result = (|| {
        let file = private.path().join("snapshot.sqlite3");
        let info = snapshot::create(store, &pin.id, &file)?;
        let mut connection = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let transaction = connection.transaction()?;
        let mut report = Export {
            generation: info.generation,
            items: 0,
            records: 0,
            assets: 0,
            portable_bytes: 0,
            snapshot_bytes: fs::metadata(&file)?.len(),
        };
        write_items(&transaction, stage.path(), &mut report)?;
        write_records(&transaction, stage.path(), &mut report)?;
        write_assets(&transaction, root, stage.path(), &mut report)?;
        transaction.commit()?;
        if destination.try_exists()? {
            return Err(Error::Invalid(
                "The export destination was created during preparation.".into(),
            ));
        }
        fs::rename(stage.path(), destination)?;
        Ok(report)
    })();
    // A handled failure has no remaining worker reading the attachments. After
    // a crash, housekeeping reclaims the pin once its lock is free.
    maintenance::release(store, pin)?;
    result
}

fn new_file(path: &Path) -> Result<File> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| Error::Invalid("Invalid portable output path.".into()))?,
    )?;
    Ok(File::options().write(true).create_new(true).open(path)?)
}
fn write_json(path: &Path, value: &Value) -> Result<u64> {
    let bytes = canonical_json::to_vec(value)?;
    let mut file = new_file(path)?;
    file.write_all(&bytes)?;
    Ok(file.metadata()?.len())
}
fn write_items(tx: &Transaction<'_>, stage: &Path, report: &mut Export) -> Result<()> {
    let mut statement = tx.prepare("SELECT uuid FROM item ORDER BY id")?;
    for id in statement.query_map([], |row| row.get::<_, String>(0))? {
        let id = id?;
        let path = ItemId::try_from(id.as_str())
            .map_err(Error::Invalid)?
            .path(stage);
        let value = item::read(tx, &id, true)?.ok_or(Error::NotFound)?;
        report.portable_bytes += write_json(&path, &value)?;
        report.items += 1;
    }
    Ok(())
}
fn write_records(tx: &Transaction<'_>, stage: &Path, report: &mut Export) -> Result<()> {
    let mut statement =
        tx.prepare("SELECT path,canonical_json FROM project_record ORDER BY path")?;
    for record in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (path, value) = record?;
        checkpoints::validate_record_path(&path)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        report.portable_bytes += write_json(&stage.join(path), &serde_json::from_str(&value)?)?;
        report.records += 1;
    }
    Ok(())
}
fn write_assets(
    tx: &Transaction<'_>,
    root: &Path,
    stage: &Path,
    report: &mut Export,
) -> Result<()> {
    let repo = checkpoints::open(root).map_err(|error| Error::Invalid(error.to_string()))?;
    let mut statement = tx.prepare(
        "SELECT p.path,a.oid,a.size FROM asset_path p JOIN asset a ON a.oid=p.oid ORDER BY p.path",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (path, oid, size) = row?;
        checkpoints::validate_asset_path(&path)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let size = u64::try_from(size).map_err(|_| Error::corrupt("Invalid asset size"))?;
        checkpoints::copy_verified_asset(&repo, &oid, size, &mut new_file(&stage.join(path))?)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        report.portable_bytes += size;
        report.assets += 1;
    }
    Ok(())
}
