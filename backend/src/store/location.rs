// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Recognized unsafe live database locations are rejected before opening SQLite.
//! Completed closed backups/portable exports do not use this live-location gate.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::{Error, Result};

pub(crate) fn validate(root: &Path) -> Result<PathBuf> {
    let resolved = resolve(root)?;
    validate_roots(&resolved, &provider_roots())?;
    let existing = resolved
        .ancestors()
        .find(|path| path.exists())
        .ok_or_else(|| {
            Error::Invalid("Project location has no existing parent directory.".to_owned())
        })?;
    if registered_cloud(existing) {
        return Err(Error::Invalid("This location belongs to an OS-registered cloud sync provider. Move the complete closed project to a local folder and use Git between devices.".to_owned()));
    }
    match network_volume(existing)? {
        Some(true) => return Err(Error::Invalid("A live project database cannot be opened on a network filesystem. Move the complete closed project to a supported local folder and use Git between devices.".to_owned())),
        None => tracing::warn!(operation="storage.location", path=%resolved.display(), "This filesystem could not be identified. Keep live projects on a local disk outside cloud-sync folders; use Git or a completed backup between devices."),
        Some(false) => {},
    }
    Ok(resolved)
}

/// Canonicalize the nearest existing parent so aliases cannot bypass root checks.
fn resolve(path: &Path) -> Result<PathBuf> {
    let mut current = std::path::absolute(path)?;
    let mut missing = Vec::<OsString>::new();
    while !current.try_exists()? {
        missing.push(
            current
                .file_name()
                .ok_or_else(|| Error::Invalid("Invalid project location.".to_owned()))?
                .to_owned(),
        );
        if !current.pop() {
            return Err(Error::Invalid(
                "Project location has no existing parent.".to_owned(),
            ));
        }
    }
    let mut resolved = current.canonicalize()?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn validate_roots(path: &Path, roots: &[PathBuf]) -> Result<()> {
    for root in roots {
        let Ok(root) = resolve(root) else {
            continue;
        };
        if contained(path, &root) {
            return Err(Error::Invalid("A live project database cannot be opened inside a configured cloud-sync folder. Move or restore the complete closed project to a local folder and use Git between devices. Completed backups and portable exports may be stored here.".to_owned()));
        }
    }
    Ok(())
}

fn contained(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let parts = |path: &Path| {
            path.components()
                .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
        };
        parts(path).starts_with(&parts(root))
    }
    #[cfg(not(windows))]
    path.starts_with(root)
}

fn provider_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Some(root) = std::env::var_os(key).filter(|value| !value.is_empty()) {
            roots.push(root.into());
        }
    }
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dropbox = Vec::new();
    for key in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(path) = std::env::var_os(key) {
            dropbox.push(PathBuf::from(path).join("Dropbox/info.json"));
        }
    }
    if let Some(home) = &home {
        dropbox.push(home.join(".dropbox/info.json"));
    }
    for file in dropbox {
        let Some(value) = std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        if let Some(accounts) = value.as_object() {
            for account in accounts.values() {
                if let Some(path) = account["path"].as_str() {
                    roots.push(PathBuf::from(path));
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = home {
        let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        if icloud.is_dir() {
            roots.push(icloud);
        }
        if let Ok(providers) = std::fs::read_dir(home.join("Library/CloudStorage")) {
            roots.extend(
                providers
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.path()),
            );
        }
    }
    roots
}

#[cfg(windows)]
fn network_volume(path: &Path) -> Result<Option<bool>> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Component;
    use windows_sys::Win32::{
        Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW},
        System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE},
    };
    if path.components().any(|part| matches!(part, Component::Prefix(prefix) if matches!(prefix.kind(), std::path::Prefix::UNC(..)|std::path::Prefix::VerbatimUNC(..)))) {
        return Ok(Some(true));
    }
    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut volume = vec![0_u16; 32_768];
    #[expect(
        unsafe_code,
        reason = "The Windows query receives owned NUL-terminated input and an exactly sized writable UTF-16 buffer."
    )]
    // SAFETY: Both buffers outlive this call; the advertised output capacity is
    // its actual allocation. Windows retains neither pointer.
    let found = unsafe { GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), 32_768) };
    if found == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    #[expect(
        unsafe_code,
        reason = "The preceding successful query produced a NUL-terminated volume path in owned storage."
    )]
    // SAFETY: The successfully populated volume buffer lives through this call.
    let kind = unsafe { GetDriveTypeW(volume.as_ptr()) };
    Ok(match kind {
        DRIVE_REMOTE => Some(true),
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK => Some(false),
        _ => None,
    })
}

#[cfg(windows)]
fn registered_cloud(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::CloudFilters::{
        CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByPath,
    };
    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut information = std::mem::MaybeUninit::<CF_SYNC_ROOT_BASIC_INFO>::uninit();
    let Ok(length) = u32::try_from(std::mem::size_of::<CF_SYNC_ROOT_BASIC_INFO>()) else {
        return false;
    };
    #[expect(
        unsafe_code,
        reason = "The OS cloud-root query receives owned NUL-terminated path storage and an output buffer of its exact declared type; output bytes are never read."
    )]
    // SAFETY: Input lives through this call and the output points to exactly one
    // correctly sized BASIC_INFO. A success code identifies registered providers
    // without relying on folder names (OneDrive, Dropbox, iCloud and others).
    // https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobypath
    unsafe {
        CfGetSyncRootInfoByPath(
            path.as_ptr(),
            CF_SYNC_ROOT_INFO_BASIC,
            information.as_mut_ptr().cast(),
            length,
            std::ptr::null_mut(),
        ) >= 0
    }
}

#[cfg(not(windows))]
const fn registered_cloud(_path: &Path) -> bool {
    false
}

#[cfg(target_os = "linux")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Platform probes share a fallible interface; unavailable Linux mount metadata reports an unknown location."
)]
fn network_volume(path: &Path) -> Result<Option<bool>> {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Ok(None);
    };
    Ok(mount_kind(path, &mounts))
}

#[cfg(target_os = "linux")]
fn mount_kind(path: &Path, mounts: &str) -> Option<bool> {
    let mut selected = None;
    let mut depth = 0;
    for line in mounts.lines() {
        let Some((fields, filesystem)) = line.split_once(" - ") else {
            continue;
        };
        let Some(mountpoint) = fields.split_whitespace().nth(4) else {
            continue;
        };
        let mountpoint = PathBuf::from(
            mountpoint
                .replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\134", "\\"),
        );
        if !path.starts_with(&mountpoint) || mountpoint.components().count() < depth {
            continue;
        }
        depth = mountpoint.components().count();
        let kind = filesystem.split_whitespace().next()?;
        selected = match kind {
            "nfs" | "nfs4" | "cifs" | "smbfs" | "9p" | "ceph" | "afs" | "fuse.sshfs"
            | "fuse.glusterfs" => Some(true),
            "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "zfs" | "overlay" | "tmpfs" | "f2fs"
            | "ntfs" | "ntfs3" | "vfat" | "exfat" => Some(false),
            _ => None,
        };
    }
    selected
}

#[cfg(target_os = "macos")]
fn network_volume(path: &Path) -> Result<Option<bool>> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let mut information = std::mem::MaybeUninit::<libc::statfs>::uninit();
    #[expect(
        unsafe_code,
        reason = "statfs writes its complete struct only on success; the owned C path is NUL-terminated for the call."
    )]
    // SAFETY: statfs receives valid owned path storage and writable space for its
    // exact result struct. Initialization is assumed only after its success code.
    let status = unsafe { libc::statfs(path.as_ptr(), information.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    #[expect(
        unsafe_code,
        reason = "statfs succeeded and initialized the complete result struct."
    )]
    // SAFETY: The successful statfs call initialized this exact output struct.
    let information = unsafe { information.assume_init() };
    Ok(Some(
        information.f_flags & libc::MNT_LOCAL.cast_unsigned() == 0,
    ))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn network_volume(_path: &Path) -> Result<Option<bool>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_roots_reject_descendants_without_rejecting_similar_folder_names() {
        let temp = tempfile::tempdir().unwrap();
        let cloud = temp.path().join("Cloud account");
        let unrelated = temp.path().join("Cloud account local notes");
        std::fs::create_dir(&cloud).unwrap();
        std::fs::create_dir(&unrelated).unwrap();
        assert!(
            validate_roots(
                &resolve(&cloud.join("new/project")).unwrap(),
                std::slice::from_ref(&cloud)
            )
            .is_err()
        );
        assert!(
            validate_roots(&resolve(&unrelated).unwrap(), std::slice::from_ref(&cloud)).is_ok()
        );
        assert!(validate_roots(&resolve(&cloud).unwrap(), std::slice::from_ref(&cloud)).is_err());
        assert!(
            validate_roots(
                &resolve(&cloud.join("../Cloud account local notes")).unwrap(),
                &[cloud]
            )
            .is_ok()
        );
    }

    #[test]
    fn ordinary_local_folder_is_supported_for_creation_and_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("new/project");
        assert_eq!(validate(&missing).unwrap(), resolve(&missing).unwrap());
        std::fs::create_dir_all(&missing).unwrap();
        assert_eq!(validate(&missing).unwrap(), missing.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_into_cloud_root_cannot_bypass_location_guard() {
        let temp = tempfile::tempdir().unwrap();
        let cloud = temp.path().join("cloud");
        std::fs::create_dir(&cloud).unwrap();
        let link = temp.path().join("alias");
        std::os::unix::fs::symlink(&cloud, &link).unwrap();
        assert!(validate_roots(&resolve(&link.join("project")).unwrap(), &[cloud]).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deepest_mount_and_escaped_names_control_network_detection() {
        let mounts = "1 0 0:1 / / rw - ext4 /dev/root rw\n2 1 0:2 / /mnt/team\\040share rw - nfs4 server:/team rw\n3 1 0:3 / /mnt/team\\040share/local rw - ext4 /dev/local rw";
        assert_eq!(
            mount_kind(Path::new("/mnt/team share/project"), mounts),
            Some(true)
        );
        assert_eq!(
            mount_kind(Path::new("/mnt/team share/local/project"), mounts),
            Some(false)
        );
    }
}
