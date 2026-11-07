// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Checks on the compiled PDF before it is published.
use std::{fs, io::Read, path::Path};

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
