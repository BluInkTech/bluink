// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Checks around the packaged compiler, independent of document serialization.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

#[derive(PartialEq, Eq)]
struct BinaryIdentity {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
}

impl BinaryIdentity {
    fn read(path: &Path) -> Result<Self, String> {
        let inspect = || {
            let path = fs::canonicalize(path)?;
            let metadata = fs::metadata(&path)?;
            Ok::<_, std::io::Error>(Self {
                path,
                len: metadata.len(),
                modified: metadata.modified()?,
                created: metadata.created().ok(),
            })
        };
        inspect().map_err(|error| format!("Cannot inspect packaged Typst: {error}"))
    }
}

/// Cache only the last successfully verified binary. Replacement or a different
/// resource path triggers a new check; failed probes are always retried.
#[derive(Default)]
struct VersionCache(Mutex<Option<BinaryIdentity>>);

impl VersionCache {
    fn verify(
        &self,
        path: &Path,
        query: impl FnOnce(&Path) -> Result<Output, String>,
    ) -> Result<(), String> {
        let identity = BinaryIdentity::read(path)?;
        if self.0.lock().map_err(|error| error.to_string())?.as_ref() == Some(&identity) {
            return Ok(());
        }
        // Never hold the cache lock while launching or waiting on a process.
        crate::export_jobs::pdf::check_typst_version(&query(&identity.path)?)?;
        if BinaryIdentity::read(path)? != identity {
            return Err("Packaged Typst changed while checking its version.".to_owned());
        }
        *self.0.lock().map_err(|error| error.to_string())? = Some(identity);
        Ok(())
    }
}

pub(super) fn verify_typst(path: &Path) -> Result<(), String> {
    static CACHE: OnceLock<VersionCache> = OnceLock::new();
    CACHE
        .get_or_init(VersionCache::default)
        .verify(path, |binary| {
            Command::new(binary)
                .arg("--version")
                .output()
                .map_err(|error| format!("Cannot start packaged Typst: {error}"))
        })
}

/// Inspect size and signature without allocating a buffer for the entire PDF.
pub(super) fn validate_pdf(path: &Path) -> Result<(), String> {
    let inspect = || {
        let mut file = fs::File::open(path)?;
        let len = file.metadata()?.len();
        if !(5..=1024 * 1024 * 1024).contains(&len) {
            return Ok(false);
        }
        let mut signature = [0; 5];
        file.read_exact(&mut signature)?;
        Ok::<_, std::io::Error>(&signature == b"%PDF-")
    };
    if !inspect().map_err(|error| format!("Cannot read compiled PDF: {error}"))? {
        return Err("Typst produced an invalid or oversized PDF.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_jobs::pdf::tests::version_output;
    use std::cell::Cell;

    #[test]
    fn version_cache_rechecks_changed_binaries_and_retries_failures() {
        let temporary = tempfile::tempdir().unwrap();
        let binary = temporary.path().join("typst");
        fs::write(&binary, b"first").unwrap();
        let cache = VersionCache::default();
        let probes = Cell::new(0);
        let query = |_: &Path| {
            probes.set(probes.get() + 1);
            Ok(version_output(0, "typst 0.15.1 (test)\n", ""))
        };
        cache.verify(&binary, query).unwrap();
        cache.verify(&binary, query).unwrap();
        assert_eq!(probes.get(), 1);
        fs::write(&binary, b"replacement").unwrap();
        assert!(cache.verify(&binary, |_| Err("blocked".into())).is_err());
        assert!(
            cache
                .verify(&binary, |_| Ok(version_output(
                    0,
                    "typst 0.14.0 (test)",
                    ""
                )))
                .is_err()
        );
        cache.verify(&binary, query).unwrap();
        assert_eq!(probes.get(), 2);
        let other = temporary.path().join("other");
        fs::copy(&binary, &other).unwrap();
        cache.verify(&other, query).unwrap();
        assert_eq!(probes.get(), 3);
        fs::remove_file(&other).unwrap();
        assert!(cache.verify(&other, query).is_err());
    }

    #[test]
    fn version_cache_rejects_replacement_during_verification() {
        let temporary = tempfile::tempdir().unwrap();
        let binary = temporary.path().join("typst");
        fs::write(&binary, b"first").unwrap();
        let error = VersionCache::default()
            .verify(&binary, |path| {
                fs::write(path, b"replacement").unwrap();
                Ok(version_output(0, "typst 0.15.1 (test)", ""))
            })
            .unwrap_err();
        assert!(error.contains("changed while checking"));
    }

    #[test]
    fn pdf_validation_preserves_signature_and_size_limits() {
        let temporary = tempfile::tempdir().unwrap();
        let pdf = temporary.path().join("result.pdf");
        assert!(validate_pdf(&pdf).is_err());
        for bytes in [b"".as_slice(), b"%PDF", b"wrong"] {
            fs::write(&pdf, bytes).unwrap();
            assert!(
                validate_pdf(&pdf)
                    .unwrap_err()
                    .contains("invalid or oversized")
            );
        }
        fs::write(&pdf, b"%PDF-").unwrap();
        validate_pdf(&pdf).unwrap();
        // Extend the file to exercise the boundary without a GiB read buffer.
        let file = fs::OpenOptions::new().write(true).open(&pdf).unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();
        validate_pdf(&pdf).unwrap();
        file.set_len(1024 * 1024 * 1024 + 1).unwrap();
        assert!(
            validate_pdf(&pdf)
                .unwrap_err()
                .contains("invalid or oversized")
        );
    }
}
