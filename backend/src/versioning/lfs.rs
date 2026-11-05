// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Local LFS object storage; no network transport is used by local history.
use super::{Result, failure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const HEADER: &str = "version https://git-lfs.github.com/spec/v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Pointer {
    oid: String,
    size: u64,
}

impl Pointer {
    pub(super) fn identity(&self) -> (&str, u64) {
        (&self.oid, self.size)
    }

    pub(super) fn from_identity(oid: &str, size: u64) -> Result<Self> {
        Self::parse(format!("{HEADER}\noid sha256:{oid}\nsize {size}\n").as_bytes())
    }

    pub(super) fn fingerprint(&self) -> (String, u64) {
        (self.oid.clone(), self.size)
    }

    pub(super) fn bytes(&self) -> Vec<u8> {
        format!("{HEADER}\noid sha256:{}\nsize {}\n", self.oid, self.size).into_bytes()
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|_| failure("Invalid LFS pointer"))?;
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return Err(failure("Missing LFS pointer header"));
        }
        let oid = lines
            .next()
            .and_then(|line| line.strip_prefix("oid sha256:"))
            .filter(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| failure("Invalid LFS object identity"))?;
        let size = lines
            .next()
            .and_then(|line| line.strip_prefix("size "))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| failure("Invalid LFS object size"))?;
        if lines.next().is_some() {
            return Err(failure("Unsupported LFS pointer extension"));
        }
        Ok(Self {
            oid: oid.to_owned(),
            size,
        })
    }

    pub(super) fn path(&self, git: &Path) -> PathBuf {
        git.join("lfs/objects")
            .join(&self.oid[..2])
            .join(&self.oid[2..4])
            .join(&self.oid)
    }

    pub(super) fn available_path(&self, git: &Path) -> Result<Option<PathBuf>> {
        let path = self.path(git);
        check_components(git, &path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() != self.size {
            return Err(failure(
                "A local attachment history object has an invalid size or file type",
            ));
        }
        Ok(Some(path))
    }

    /// Objects are hash-checked when they enter the store, so finding one only
    /// checks its file type and size. Readers that copy or send its bytes
    /// verify them with `copy_verified` or `verified_path`.
    pub(super) fn stored_path(&self, git: &Path) -> Result<PathBuf> {
        self.available_path(git)?
            .ok_or_else(|| failure("A local attachment history object is missing"))
    }

    pub(super) fn copy_verified(&self, git: &Path, output: &mut impl Write) -> Result<()> {
        let (oid, size) = hash_copy(&mut File::open(self.stored_path(git)?)?, output)?;
        if oid != self.oid || size != self.size {
            return Err(failure("A local attachment history object is corrupt"));
        }
        Ok(())
    }

    pub(super) fn verified_path(&self, git: &Path) -> Result<PathBuf> {
        self.copy_verified(git, &mut std::io::sink())?;
        self.stored_path(git)
    }
}

/// Receive a declared object through a bounded stream. Nothing becomes visible
/// at its final path until both its identity and byte count have been verified.
pub(super) fn receive(git: &Path, pointer: &Pointer, input: &mut impl Read) -> Result<()> {
    let staging = git.join("lfs/tmp");
    check_components(git, &staging)?;
    fs::create_dir_all(&staging)?;
    let mut temporary = tempfile::NamedTempFile::new_in(staging)?;
    let (oid, size) = hash_copy(
        &mut input.take(pointer.size.saturating_add(1)),
        &mut temporary,
    )?;
    if oid != pointer.oid || size != pointer.size {
        return Err(failure("Attachment transfer failed its hash or size check"));
    }
    temporary.as_file().sync_all()?;
    let destination = pointer.path(git);
    check_components(git, &destination)?;
    fs::create_dir_all(
        destination
            .parent()
            .ok_or_else(|| failure("Invalid attachment path"))?,
    )?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            pointer.stored_path(git)?;
            Ok(())
        }
        Err(error) => Err(error.error.into()),
    }
}

fn check_components(git: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(git)
        .map_err(|_| failure("Attachment path escaped its object store"))?;
    let mut current = git.to_owned();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure(
                    "Attachment object stores cannot contain symbolic links",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn hash_copy(input: &mut impl Read, output: &mut impl Write) -> Result<(String, u64)> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let length = input.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        output.write_all(&buffer[..length])?;
        digest.update(&buffer[..length]);
        size = size
            .checked_add(length as u64)
            .ok_or_else(|| failure("Attachment is too large"))?;
    }
    Ok((digest_hex(&digest.finalize()), size))
}

pub(crate) fn digest_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

/// The caller opened this handle during capture. Atomic canonical replacements
/// cannot change its bytes; in-place external changes abort this checkpoint.
pub(super) fn capture(git: &Path, file: &mut File) -> Result<Pointer> {
    let before = file.metadata()?;
    stage_object(git, |temporary| {
        let (oid, size) = hash_copy(file, temporary)?;
        let after = file.metadata()?;
        if before.len() != size || after.len() != size || before.modified()? != after.modified()? {
            return Err(failure(
                "An attachment changed during history capture; retry the checkpoint",
            ));
        }
        Ok((oid, size))
    })
}

/// Bytes already in memory, such as an upload, are hashed as they are staged.
pub(super) fn capture_bytes(git: &Path, bytes: &[u8]) -> Result<Pointer> {
    stage_object(git, |temporary| hash_copy(&mut &bytes[..], temporary))
}

/// Stage an object, then publish it under its digest. An object that is
/// already stored is kept, so the same bytes are never written twice.
fn stage_object(
    git: &Path,
    write: impl FnOnce(&mut tempfile::NamedTempFile) -> Result<(String, u64)>,
) -> Result<Pointer> {
    let staging = git.join("lfs/tmp");
    check_components(git, &staging)?;
    fs::create_dir_all(&staging)?;
    let mut temporary = tempfile::NamedTempFile::new_in(staging)?;
    let (oid, size) = write(&mut temporary)?;
    temporary.as_file().sync_all()?;
    let pointer = Pointer { oid, size };
    let destination = pointer.path(git);
    check_components(git, &destination)?;
    fs::create_dir_all(
        destination
            .parent()
            .ok_or_else(|| failure("LFS object has no directory"))?,
    )?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            pointer.stored_path(git)?;
        }
        Err(error) => return Err(error.error.into()),
    }
    Ok(pointer)
}
