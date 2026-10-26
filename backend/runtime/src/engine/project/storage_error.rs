// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt;
use std::path::{Path, PathBuf};

/// A canonical-storage failure retains the path and original operating-system cause.
#[derive(Debug)]
pub(crate) enum StorageError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Serialization(serde_json::Error),
    Clock(std::time::SystemTimeError),
    InvalidPath(&'static str),
}

impl StorageError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(output, "{}: {source}", path.display()),
            Self::Serialization(source) => fmt::Display::fmt(source, output),
            Self::Clock(source) => fmt::Display::fmt(source, output),
            Self::InvalidPath(message) => output.write_str(message),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialization(source) => Some(source),
            Self::Clock(source) => Some(source),
            Self::InvalidPath(_) => None,
        }
    }
}
