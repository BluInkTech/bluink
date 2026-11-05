// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Independent full backups. A manifest is published only after the closed
//! database and all required Git/LFS objects have been copied and verified.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{DATABASE_NAME, Error, Result, Store, location, maintenance, snapshot};
use crate::versioning::checkpoints::{self, HistoryCopy};

const MANIFEST: &str = "backup.json";
const PENDING: &str = "project.sqlite3.pending";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Backup {
    version: u32,
    pub(crate) snapshot: snapshot::Snapshot,
    database_sha256: String,
    pub(crate) history: HistoryCopy,
}

/// The destination must be new. Failure keeps the partial destination for
/// diagnosis; it never reports a partial backup as ready.
pub(crate) fn create(store: &Store, destination: &Path) -> Result<Backup> {
    let source = store
        .path()
        .parent()
        .ok_or_else(|| Error::Invalid("Project database has no parent directory.".to_owned()))?;
    fs::create_dir(destination)?;
    let pin = maintenance::protect(store, "backup", destination)?;
    let result = (|| {
        checkpoints::cover_backup(source, store)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let pending = destination.join(PENDING);
        let snapshot = snapshot::create(store, &pin.id, &pending)?;
        let connection = Connection::open_with_flags(&pending, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let history = checkpoints::copy_pinned_history(source, destination, &connection)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        drop(connection);
        let backup = Backup {
            version: 1,
            snapshot,
            database_sha256: digest(&pending)?,
            history,
        };
        fs::rename(&pending, destination.join(DATABASE_NAME))?;
        crate::store::io::create_json_atomic(&destination.join(MANIFEST), &backup)
            .map_err(|error| Error::Invalid(format!("{error:#}")))?;
        Ok(backup)
    })();
    // Any pin blocks all retention and packing, so a handled failure must not
    // leave one behind. After a crash, housekeeping reclaims it.
    maintenance::release(store, pin)?;
    result
}

/// Restore into a new supported local directory. The live database filename is
/// published last, after history verification and a new feed epoch are durable.
pub(crate) fn restore(source: &Path, destination: &Path) -> Result<Backup> {
    let backup: Backup = serde_json::from_slice(&fs::read(source.join(MANIFEST))?)?;
    if backup.version != 1 {
        return Err(Error::Invalid("Unsupported backup format.".to_owned()));
    }
    let database = source.join(DATABASE_NAME);
    if digest(&database)? != backup.database_sha256 {
        return Err(Error::Invalid(
            "Backup database checksum does not match its manifest.".to_owned(),
        ));
    }
    let destination = location::validate(destination)?;
    fs::create_dir(&destination)?;
    let pending = destination.join(PENDING);
    copy_new(&database, &pending)?;
    let mut connection = Connection::open_with_flags(&pending, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    if snapshot::verify(&connection, &backup.snapshot.maintenance_pin)? != backup.snapshot {
        return Err(Error::Invalid(
            "Backup database does not match its manifest.".to_owned(),
        ));
    }
    let history = checkpoints::copy_pinned_history(source, &destination, &connection)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    if history.unavailable_history != backup.history.unavailable_history {
        return Err(Error::Invalid(
            "Backup historical attachment inventory changed.".to_owned(),
        ));
    }
    snapshot::prepare_restore(&mut connection, &backup.snapshot)?;
    drop(connection);
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pending)?
        .sync_all()?;
    fs::rename(pending, destination.join(DATABASE_NAME))?;
    Ok(backup)
}

fn digest(path: &Path) -> Result<String> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(Error::Invalid(
            "Backup database must be a regular file.".to_owned(),
        ));
    }
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = vec![0_u8; 65_536];
    loop {
        let count = input.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(checkpoints::digest_hex(&hash.finalize()))
}

fn copy_new(source: &Path, destination: &Path) -> Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
