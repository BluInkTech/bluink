// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use super::error::{ApplicationError, Result};

pub const SCHEMA_FINGERPRINT: &str = "bluink-v15-typed-document-relations";

pub(crate) fn resolve(root: &Path, project_id: &str, location: Option<&str>) -> Result<PathBuf> {
    let machine = hostname::get()?.to_string_lossy().to_lowercase();
    let machine = machine
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let machine = if machine.is_empty() {
        "local"
    } else {
        &machine
    };
    let name = format!("tantivy-{machine}-v1-{SCHEMA_FINGERPRINT}");
    let directory = match location {
        None | Some("" | "project") => root.join(".cache"),
        Some("system") => system_directory()?.join(project_id),
        Some(custom) => {
            let custom = PathBuf::from(custom);
            let custom = if custom.is_absolute() {
                custom
            } else {
                std::env::current_dir()?.join(custom)
            };
            custom.join(project_id)
        }
    };
    Ok(directory.join(name))
}

fn system_directory() -> Result<PathBuf> {
    let home = || {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .ok_or_else(|| ApplicationError::Internal("Cannot resolve home directory.".to_owned()))
    };
    if cfg!(windows) {
        let base = match std::env::var_os("LOCALAPPDATA") {
            Some(path) => PathBuf::from(path),
            None => home()?.join("AppData/Local"),
        };
        Ok(base.join("Bluink/cache"))
    } else if cfg!(target_os = "macos") {
        Ok(home()?.join("Library/Caches/Bluink"))
    } else {
        let base = match std::env::var_os("XDG_CACHE_HOME") {
            Some(path) => PathBuf::from(path),
            None => home()?.join(".cache"),
        };
        Ok(base.join("bluink"))
    }
}

/// The descriptor, rather than a PID marker, owns the lock. Never unlink this file:
/// doing so would allow another process to lock a different inode at the same path.
pub(crate) struct CacheLease {
    file: File,
}

impl CacheLease {
    pub(crate) fn acquire(cache: &Path) -> Result<Self> {
        let mut sidecar = cache.as_os_str().to_owned();
        sidecar.push(".lock");
        let sidecar = PathBuf::from(sidecar);
        fs::create_dir_all(sidecar.parent().ok_or_else(|| {
            ApplicationError::Internal("Cache lock has no parent directory.".to_owned())
        })?)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(sidecar)?;
        if !file.try_lock_exclusive()? {
            return Err(ApplicationError::InvalidRequest(
                "The local project cache is already in use. Stop the existing Bluink server before trying again.".to_owned(),
            ));
        }
        Ok(Self { file })
    }
}

impl Drop for CacheLease {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly inherit the open-file
        // description before exec. Closing our descriptor alone would leave
        // the lease locked until that child closes its copy.
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_releases_without_deleting_the_lock_file() {
        let root = std::env::temp_dir().join(format!("bluink-cache-{}", std::process::id()));
        let cache = root.join("index");
        let first = CacheLease::acquire(&cache).unwrap();
        assert!(CacheLease::acquire(&cache).is_err());
        drop(first);
        assert!(root.join("index.lock").is_file());
        drop(CacheLease::acquire(&cache).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locations_keep_machine_and_schema_scoped_names() {
        let root = std::env::temp_dir().join("bluink-location");
        let project = resolve(&root, "project-id", None).unwrap();
        assert_eq!(project.parent().unwrap(), root.join(".cache"));
        assert!(
            project
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(SCHEMA_FINGERPRINT)
        );
        let custom = resolve(&root, "project-id", root.to_str()).unwrap();
        assert_eq!(custom.parent().unwrap(), root.join("project-id"));
    }

    #[test]
    fn inherited_descriptor_does_not_extend_cache_ownership() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("index");
        let lease = CacheLease::acquire(&cache).unwrap();
        let inherited = lease.file.try_clone().unwrap();
        drop(lease);
        let next = CacheLease::acquire(&cache).unwrap();
        assert!(CacheLease::acquire(&cache).is_err());
        drop(inherited);
        assert!(CacheLease::acquire(&cache).is_err());
        drop(next);
        drop(CacheLease::acquire(&cache).unwrap());
    }
}
