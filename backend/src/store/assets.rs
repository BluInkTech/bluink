// SPDX-License-Identifier: MPL-2.0
//! Portable attachment names map to immutable binary objects. Install the bytes
//! before publishing their mapping, then recheck the owner in the writer.
use super::{Error, Result, Store, changes, git};
use crate::{
    domain::project::{asset::valid_filename, identity::ItemId},
    store::changes::ChangeDomain,
    versioning::checkpoints,
};
use rusqlite::{OptionalExtension, Transaction};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) struct Object {
    pub(crate) oid: String,
    pub(crate) size: u64,
}
impl Object {
    /// Read the captured identity once, bounded by its declared size and the caller's limit.
    pub(crate) fn read_verified(&self, root: &Path, limit: u64) -> Result<Vec<u8>> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        if self.size > limit {
            return Err(Error::Invalid(
                "Attachment exceeds the permitted size.".into(),
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(self.available(root)?)?
            .take(self.size.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).ok() != Some(self.size)
            || checkpoints::digest_hex(&Sha256::digest(&bytes)) != self.oid
        {
            return Err(Error::corrupt(
                "Attachment content does not match its captured identity.",
            ));
        }
        Ok(bytes)
    }

    /// Stream the captured identity into `output`, verifying its size and digest.
    /// The caller discards `output` on failure.
    pub(crate) fn copy_verified(&self, root: &Path, output: &mut impl Write) -> Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut input =
            std::fs::File::open(self.available(root)?)?.take(self.size.saturating_add(1));
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut size = 0_u64;
        loop {
            let length = input.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
            output.write_all(&buffer[..length])?;
            size += length as u64;
        }
        if size != self.size || checkpoints::digest_hex(&digest.finalize()) != self.oid {
            return Err(Error::corrupt(
                "Attachment content does not match its captured identity.",
            ));
        }
        Ok(())
    }

    pub(crate) fn available(&self, root: &Path) -> Result<PathBuf> {
        checkpoints::available_asset_path(root, &self.oid, self.size)
            .map_err(|error| Error::Internal(error.into()))
    }
}

pub(crate) fn owner_path(id: &str, name: &str) -> Result<String> {
    if !valid_filename(name) {
        return Err(Error::Invalid(
            "Asset filename must not contain a path or reserved character.".into(),
        ));
    }
    let owner = ItemId::try_from(id).map_err(Error::Invalid)?;
    Ok(owner
        .asset_directory(Path::new(""))
        .join(name)
        .to_string_lossy()
        .replace('\\', "/"))
}

pub(crate) fn mapping(tx: &Transaction<'_>, path: &str) -> Result<Option<Object>> {
    let row = tx
        .query_row(
            "SELECT a.oid,a.size FROM asset_path p JOIN asset a ON a.oid=p.oid WHERE p.path=?1",
            [path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    row.map(|(oid, size)| {
        Ok(Object {
            oid,
            size: u64::try_from(size).map_err(|error| Error::Internal(error.into()))?,
        })
    })
    .transpose()
}

pub(crate) fn install_bytes(root: &Path, bytes: &[u8]) -> Result<checkpoints::VerifiedAsset> {
    checkpoints::install_asset_bytes(root, bytes).map_err(|error| Error::Internal(error.into()))
}

pub(crate) fn upload(root: &Path, id: &str, name: &str, bytes: &[u8]) -> Result<String> {
    let path = owner_path(id, name)?;
    let store = Store::shared(root)?;
    store.read(|tx| owner(tx, id))?;
    let asset = install_bytes(root, bytes)?;
    let owner_uuid = id.to_owned();
    store.write(move |tx| {
        let owner = owner(tx, &owner_uuid)?;
        if mapping(tx, &path)?.is_some_and(|existing| existing.oid == asset.oid()) {
            return Ok(());
        }
        let generation = changes::publish(
            tx,
            &BTreeSet::from([ChangeDomain::Items, ChangeDomain::History]),
            &[],
        )?;
        git::put_asset(tx, &path, Some(owner), &asset, generation)
    })?;
    Ok(format!("/assets/{id}/{name}"))
}

fn owner(tx: &Transaction<'_>, uuid: &str) -> Result<i64> {
    tx.query_row("SELECT id FROM item WHERE uuid=?1", [uuid], |row| {
        row.get(0)
    })
    .optional()?
    .ok_or(Error::NotFound)
}
