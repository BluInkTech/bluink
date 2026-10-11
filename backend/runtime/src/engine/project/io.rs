// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Write a canonical JSON file through a same-directory temporary file.
/// The generated path can be shared by project config, items, and markers.
pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let source = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    write_bytes(path, &source, false)
}

/// Publish a new canonical file only if the path does not already exist.
/// Linking the prepared file avoids exposing a partial new project config.
pub(crate) fn create_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let source = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    write_bytes(path, &source, true)
}

pub(crate) fn write_bytes_atomic(path: &Path, source: &[u8]) -> Result<(), String> {
    write_bytes(path, source, false)
}

fn write_bytes(path: &Path, source: &[u8], create_only: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Canonical file has no parent directory")?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(
        ".{}.{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid canonical filename")?,
        std::process::id(),
        now,
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("{}: {error}", temporary.display()))?;
        file.write_all(source)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("{}: {error}", temporary.display()))?;
        if create_only {
            fs::hard_link(&temporary, path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let _ = fs::remove_file(&temporary);
            Ok(())
        } else {
            fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{create_json_atomic, write_bytes_atomic, write_json_atomic};

    #[test]
    fn replaces_json_without_leaving_a_temporary_file() {
        let root = std::env::temp_dir().join(format!("bluink-core-write-{}", std::process::id()));
        let path = root.join("bluink.json");
        write_json_atomic(&path, &json!({"version": 1})).unwrap();
        write_json_atomic(&path, &json!({"version": 2})).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_a_new_file_without_overwriting_it() {
        let root = std::env::temp_dir().join(format!("bluink-core-create-{}", std::process::id()));
        let path = root.join("bluink.json");
        create_json_atomic(&path, &json!({"version": 1})).unwrap();
        assert!(create_json_atomic(&path, &json!({"version": 2})).is_err());
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomically_replaces_binary_data() {
        let root = std::env::temp_dir().join(format!("bluink-core-binary-{}", std::process::id()));
        let path = root.join("image.png");
        write_bytes_atomic(&path, &[0, 1, 2]).unwrap();
        write_bytes_atomic(&path, &[3, 4]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![3, 4]);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
