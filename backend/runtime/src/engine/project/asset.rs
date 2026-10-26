// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use crate::engine::project::identity::ItemId;
use crate::engine::project::io::write_bytes_atomic;
use crate::engine::project::item::read_project_item;
use crate::engine::project::write::MutationError;

pub(crate) fn write_asset(
    root: &Path,
    id: &str,
    filename: &str,
    data: &[u8],
) -> Result<String, MutationError> {
    let identity = ItemId::try_from(id).map_err(MutationError::Validation)?;
    if !valid_filename(filename) {
        return Err(MutationError::Validation(
            "Asset filename must not contain a path or reserved character.".to_owned(),
        ));
    }
    let shard = identity.shard_directory(root);
    let item = identity.directory(root);
    let assets = item.join("assets");
    let path = assets.join(filename);
    // Reject linked directories before reading the owner or creating assets.
    // The same rule applies to CLI/import writes, not just HTTP uploads.
    for path in [root.to_owned(), root.join("items"), shard, item, assets] {
        confined_entry(&path, true)?;
    }
    confined_entry(&identity.path(root), false)?;
    confined_entry(&path, false)?;
    if read_project_item(root, id)
        .map_err(MutationError::Read)?
        .is_none()
    {
        return Err(MutationError::NotFound(format!("Item {id} not found")));
    }
    write_bytes_atomic(&path, data).map_err(MutationError::Storage)?;
    Ok(format!("/assets/{id}/{filename}"))
}

fn confined_entry(path: &Path, directory: bool) -> Result<(), MutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || (directory && !metadata.is_dir())
                || (!directory && !metadata.is_file()) =>
        {
            Err(MutationError::Validation(
                "Asset path must stay within the owning item.".to_owned(),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(MutationError::Storage(
            super::storage_error::StorageError::io(path, error),
        )),
    }
}

pub(crate) fn valid_filename(filename: &str) -> bool {
    let stem = filename
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let windows_device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    !filename.is_empty()
        && filename.len() <= 255
        && filename != "."
        && filename != ".."
        && !windows_device
        && !filename.ends_with('.')
        && !filename.ends_with(' ')
        && !filename.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            ) || character.is_control()
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{valid_filename, write_asset};
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_project_item;

    #[test]
    fn confines_assets_to_the_owning_item() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::test_support::seed_fixture_snippets(&root);
        crate::engine::project::properties::initialize(&root).unwrap();
        let id = "01a0e2a0-0000-7000-8000-000000000001";
        let item: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.join(format!("items/01/{id}/item.json"))).unwrap(),
        )
        .unwrap();
        write_project_item(&root, &item, None, "2026-09-28").unwrap();
        let url = write_asset(&root, id, "cover.png", &[0, 1, 2]).unwrap();
        assert_eq!(url, format!("/assets/{id}/cover.png"));
        assert_eq!(
            std::fs::read(root.join(format!("items/01/{id}/assets/cover.png"))).unwrap(),
            vec![0, 1, 2]
        );
        assert!(write_asset(&root, id, "../escape.png", &[]).is_err());
        assert!(write_asset(&root, id, "..\\escape.png", &[]).is_err());
        #[cfg(unix)]
        {
            let outside = root.join("outside");
            std::fs::create_dir(&outside).unwrap();
            let assets = root.join(format!("items/01/{id}/assets"));
            std::fs::remove_dir_all(&assets).unwrap();
            std::os::unix::fs::symlink(&outside, &assets).unwrap();
            assert!(matches!(
                write_asset(&root, id, "escape.png", b"private"),
                Err(crate::engine::project::write::MutationError::Validation(_))
            ));
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
            std::fs::remove_file(&assets).unwrap();
            let owner = root.join(format!("items/01/{id}/item.json"));
            std::fs::rename(&owner, outside.join("item.json")).unwrap();
            std::os::unix::fs::symlink(outside.join("item.json"), &owner).unwrap();
            assert!(write_asset(&root, id, "escape.png", b"private").is_err());
            assert!(!assets.exists());
        }
        temporary.close().unwrap();
    }

    #[test]
    fn rejects_reserved_and_cross_platform_unsafe_filenames() {
        for filename in [
            "", ".", "..", "a/b", "a\\b", "a:b", "a?b", "a.", "a ", "CON.png", "LPT1.txt",
        ] {
            assert!(!valid_filename(filename), "{filename}");
        }
        assert!(valid_filename("cover image.png"));
    }
}
