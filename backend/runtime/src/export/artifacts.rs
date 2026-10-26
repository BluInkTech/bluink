// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::io::{Seek, Write};
use std::path::Path;

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::application::error::{Result, internal_error};

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn id(value: &str) -> bool {
    value.len() == 36
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

fn safe_name(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains(['\\', '\0']))
    {
        return false;
    }
    match parts.as_slice() {
        ["index.html" | "search.js"] => true,
        [page] => page
            .strip_prefix("p-")
            .and_then(|page| page.strip_suffix(".html"))
            .is_some_and(id),
        ["pages", page] => page.strip_suffix(".html").is_some_and(id),
        ["assets", asset] => asset.split_once('.').is_some_and(|(hash, extension)| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && !extension.is_empty()
                && extension.bytes().all(|byte| byte.is_ascii_lowercase())
        }),
        ["assets", page, _] => id(page),
        ["fonts", "licenses", license] => license.strip_suffix(".txt").is_some_and(slug),
        ["fonts", family, font] => slug(family) && font.strip_suffix(".woff2").is_some_and(slug),
        _ => false,
    }
}

fn collect(root: &Path, relative: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &path, files)?;
        } else if kind.is_file() {
            let name = path
                .to_str()
                .ok_or_else(|| internal_error("Unsupported generated site filename."))?
                .replace('\\', "/");
            if !safe_name(&name) {
                return Err(internal_error(format!("Unsafe site bundle path: {name}")));
            }
            files.push(name);
            if files.len() > 10_000 {
                return Err(internal_error("Site bundle has too many files."));
            }
        } else {
            return Err(internal_error("Unsupported generated site file."));
        }
    }
    Ok(())
}

pub(super) fn site_zip<W: Write + Seek>(root: &Path, output: W) -> Result<W> {
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(internal_error("Unsupported generated site root."));
    }
    let mut files = Vec::new();
    collect(root, Path::new(""), &mut files)?;
    files.sort_unstable();
    let mut writer = ZipWriter::new(output);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(false);
    let mut size: u64 = 0;
    for name in files {
        let file = super::preview::file(root, &name)
            .ok_or_else(|| internal_error("Generated site changed while bundling."))?;
        let mut file = std::fs::File::open(file)?;
        let length = file.metadata()?.len();
        size = size
            .checked_add(length + 30 + name.len() as u64)
            .filter(|size| u32::try_from(*size).is_ok())
            .ok_or_else(|| internal_error("Site bundle exceeds ZIP32 size limit."))?;
        writer.start_file(name, options).map_err(internal_error)?;
        if std::io::copy(&mut file, &mut writer)? != length {
            return Err(internal_error("Generated site changed while bundling."));
        }
    }
    writer.finish().map_err(internal_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn site_zip(root: &Path) -> Result<Vec<u8>> {
        super::site_zip(root, Cursor::new(Vec::new())).map(Cursor::into_inner)
    }

    #[test]
    fn zip_roundtrip_is_deterministic_and_rejects_unexpected_files() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("search.js"), "search").unwrap();
        std::fs::write(folder.path().join("index.html"), "home").unwrap();
        let bytes = site_zip(folder.path()).unwrap();
        assert_eq!(bytes, site_zip(folder.path()).unwrap());
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.by_index(0).unwrap().name(), "index.html");
        let mut html = String::new();
        archive
            .by_name("index.html")
            .unwrap()
            .read_to_string(&mut html)
            .unwrap();
        assert_eq!(html, "home");
        std::fs::write(folder.path().join("unexpected.txt"), "private").unwrap();
        assert!(site_zip(folder.path()).is_err());
        for path in [
            "../index.html",
            "fonts/../index.html",
            "assets/x.png",
            "assets/00000000-0000-7000-8000-000000000001/..",
            "fonts/a/evil\\name.woff2",
        ] {
            assert!(!safe_name(path), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn generated_file_lookup_and_zip_reject_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("index.html"), "outside").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("index.html"),
            root.path().join("index.html"),
        )
        .unwrap();
        assert!(super::super::preview::file(root.path(), "index.html").is_none());
        assert!(site_zip(root.path()).is_err());
        let linked_root = root.path().join("linked-root");
        std::os::unix::fs::symlink(outside.path(), &linked_root).unwrap();
        assert!(super::super::preview::file(&linked_root, "index.html").is_none());
        assert!(site_zip(&linked_root).is_err());
    }
}
