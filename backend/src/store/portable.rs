// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! One immutable portable-tree boundary for demo installation and Git imports.
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use super::{Error, Result};

mod export;
mod import;
pub(crate) use export::export;
pub(crate) use import::import;
mod delta;
pub(crate) use delta::PreparedDelta;
#[cfg(test)]
mod tests;

/// A Git implementation binds to a captured tree OID and protected ref set, not
/// a moving HEAD. A folder implementation owns its validated import capture.
pub(crate) trait Source {
    fn paths(&self) -> Result<Vec<String>>;
    fn read_json(&self, path: &str) -> Result<Vec<u8>>;
    /// Returns verified readable bytes for a current asset. Missing historical
    /// assets belong to history and do not appear in the current portable tree.
    fn open_asset(&self, path: &str) -> Result<File>;
    /// Seed independent bare Git/LFS storage, preserving imported refs and names.
    /// A folder source initializes empty history and returns no imported head.
    fn seed_history(&self, destination: &Path) -> Result<Option<String>>;
}

pub(crate) struct Folder {
    root: PathBuf,
    paths: BTreeSet<String>,
}

impl Folder {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        if root.join(".git").exists() || root.join("history.git").exists() {
            return Err(Error::Invalid(
                "Import the Git tree to preserve this project's history.".to_owned(),
            ));
        }
        let mut paths = BTreeSet::from(["bluink.json".to_owned()]);
        for name in ["items", "assets", "themes"] {
            let directory = root.join(name);
            if directory.try_exists()? {
                collect(&root, &directory, &mut paths)?;
            }
        }
        Ok(Self { root, paths })
    }

    fn file(&self, path: &str) -> Result<File> {
        if !self.paths.contains(path) {
            return Err(Error::Invalid(
                "Path is outside this import capture.".to_owned(),
            ));
        }
        let path = self.root.join(path);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::Invalid(
                "Portable sources must be regular files.".to_owned(),
            ));
        }
        Ok(File::open(path)?)
    }
}

fn collect(root: &Path, path: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::Invalid(
            "Portable project links are not supported.".to_owned(),
        ));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(root, &entry?.path(), paths)?;
        }
    } else if metadata.is_file() {
        let relative = path
            .strip_prefix(root)
            .map_err(|error| Error::Invalid(error.to_string()))?
            .to_str()
            .ok_or_else(|| Error::Invalid("Portable paths must be valid Unicode.".to_owned()))?
            .replace('\\', "/");
        crate::versioning::checkpoints::validate_path(&relative)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        paths.insert(relative);
    } else {
        return Err(Error::Invalid(
            "Portable sources must be regular files.".to_owned(),
        ));
    }
    Ok(())
}

impl Source for Folder {
    fn paths(&self) -> Result<Vec<String>> {
        Ok(self.paths.iter().cloned().collect())
    }
    fn read_json(&self, path: &str) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        self.file(path)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn open_asset(&self, path: &str) -> Result<File> {
        self.file(path)
    }
    fn seed_history(&self, destination: &Path) -> Result<Option<String>> {
        crate::versioning::checkpoints::initialize(destination)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(None)
    }
}
