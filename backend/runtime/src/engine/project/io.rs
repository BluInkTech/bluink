// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use serde::Serialize;

use super::storage_error::StorageError;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub(crate) enum LockMode {
    Shared,
    Exclusive,
}

/// Keep the returned descriptor alive for the entire operation. Lock files
/// remain in place so concurrent callers always lock the same filesystem object.
pub(crate) fn lock_file(path: &Path, mode: LockMode) -> Result<fs::File, StorageError> {
    let parent = path.parent().ok_or(StorageError::InvalidPath(
        "Lock file has no parent directory",
    ))?;
    fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| StorageError::io(path, error))?;
    let _wait = crate::diagnostics::waits::Wait::new(crate::diagnostics::waits::Kind::StorageLock);
    match mode {
        LockMode::Shared => FileExt::lock_shared(&file),
        LockMode::Exclusive => file.lock_exclusive(),
    }
    .map_err(|error| StorageError::io(path, error))?;
    Ok(file)
}

/// Write a canonical JSON file through a same-directory temporary file.
/// The generated path can be shared by project config, items, and markers.
pub(crate) fn write_json_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), StorageError> {
    let source = serde_json::to_vec(value).map_err(StorageError::Serialization)?;
    write_bytes(path, &source, false)
}

/// Publish a new canonical file only if the path does not already exist.
/// Linking the prepared file avoids exposing a partial new project config.
pub(crate) fn create_json_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), StorageError> {
    let source = serde_json::to_vec(value).map_err(StorageError::Serialization)?;
    write_bytes(path, &source, true)
}

pub(crate) fn write_bytes_atomic(path: &Path, source: &[u8]) -> Result<(), StorageError> {
    write_bytes(path, source, false)
}

fn write_bytes(path: &Path, source: &[u8], create_only: bool) -> Result<(), StorageError> {
    let parent = path.parent().ok_or(StorageError::InvalidPath(
        "Canonical file has no parent directory",
    ))?;
    fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(StorageError::Clock)?
        .as_nanos();
    let temporary = parent.join(format!(
        ".{}.{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or(StorageError::InvalidPath("Invalid canonical filename"))?,
        std::process::id(),
        now,
        // Uniqueness only; file creation provides the cross-thread/process exclusion.
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| StorageError::io(&temporary, error))?;
        file.write_all(source)
            .and_then(|()| file.sync_all())
            .map_err(|error| StorageError::io(&temporary, error))?;
        // Preparation is complete. Publish a closed file so descriptor lifetime
        // cannot overlap replacement or temporary-link cleanup.
        drop(file);
        if create_only {
            fs::hard_link(&temporary, path).map_err(|error| StorageError::io(path, error))?;
            let _ = fs::remove_file(&temporary);
            Ok(())
        } else {
            fs::rename(&temporary, path).map_err(|error| StorageError::io(path, error))
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::StorageError;
    use fs4::fs_std::FileExt;
    use serde_json::json;
    use std::error::Error;

    use super::{LockMode, create_json_atomic, lock_file, write_bytes_atomic, write_json_atomic};

    #[test]
    fn shared_locks_coexist_and_exclusive_locks_release_with_the_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("locks/test.lock");
        let first = lock_file(&path, LockMode::Shared).unwrap();
        let second = lock_file(&path, LockMode::Shared).unwrap();
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(!probe.try_lock_exclusive().unwrap());
        drop(first);
        assert!(!probe.try_lock_exclusive().unwrap());
        drop(second);
        assert!(probe.try_lock_exclusive().unwrap());
        FileExt::unlock(&probe).unwrap();
        let exclusive = lock_file(&path, LockMode::Exclusive).unwrap();
        assert!(!FileExt::try_lock_shared(&probe).unwrap());
        drop(exclusive);
        assert!(FileExt::try_lock_shared(&probe).unwrap());
        assert!(path.exists());
    }

    #[test]
    fn replaces_json_without_leaving_a_temporary_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let path = root.join("bluink.json");
        write_json_atomic(&path, &json!({"version": 1})).unwrap();
        write_json_atomic(&path, &json!({"version": 2})).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        temporary.close().unwrap();
    }

    #[test]
    fn creates_a_new_file_without_overwriting_it() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let path = root.join("bluink.json");
        create_json_atomic(&path, &json!({"version": 1})).unwrap();
        let failure = create_json_atomic(&path, &json!({"version": 2})).unwrap_err();
        assert!(matches!(
            &failure,
            StorageError::Io { path: failed_path, source }
                if failed_path == &path && source.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert!(failure.source().unwrap().is::<std::io::Error>());
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        temporary.close().unwrap();
    }

    #[test]
    fn atomically_replaces_binary_data() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let path = root.join("image.png");
        write_bytes_atomic(&path, &[0, 1, 2]).unwrap();
        write_bytes_atomic(&path, &[3, 4]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![3, 4]);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        temporary.close().unwrap();
    }

    #[test]
    fn failed_serialization_does_not_replace_canonical_data() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("item.json");
        write_json_atomic(&path, &json!({"saved": true})).unwrap();
        let invalid = std::collections::BTreeMap::from([((1, 2), "value")]);
        let failure = write_json_atomic(&path, &invalid).unwrap_err();
        assert!(matches!(&failure, StorageError::Serialization(_)));
        assert!(failure.source().unwrap().is::<serde_json::Error>());
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"saved":true}"#);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        root.close().unwrap();
    }

    #[test]
    fn failed_replacement_reports_target_and_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("occupied");
        std::fs::create_dir(&path).unwrap();
        let failure = write_bytes_atomic(&path, b"new content").unwrap_err();
        assert!(matches!(
            &failure,
            StorageError::Io { path: failed_path, .. } if failed_path == &path
        ));
        assert!(failure.source().unwrap().is::<std::io::Error>());
        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        root.close().unwrap();
    }
}
