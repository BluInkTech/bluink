// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Publish a completed file beside its destination without replacing user data.
use std::{io::Write, path::Path};

pub(super) fn publish_file(
    bytes: &[u8],
    destination: &Path,
    label: &str,
    extension: &str,
) -> Result<(), String> {
    if !destination
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
    {
        return Err(format!("{label} output filename must end in .{extension}"));
    }
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "{label} output already exists: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect {label} output: {error}")),
    }
    let parent = destination
        .parent()
        .filter(|path| path.is_dir())
        .ok_or_else(|| format!("{label} output parent must be an existing folder"))?;
    let mut stage = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    stage.write_all(bytes).map_err(|error| error.to_string())?;
    stage
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    // The check above gives a useful error; this operation also closes the race
    // with another process creating the destination after that check.
    stage.persist_noclobber(destination).map_err(|error| {
        format!(
            "Cannot publish {label} output without replacing an existing file: {}",
            error.error
        )
    })?;
    Ok(())
}
