// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt;

/// Store failures. The named kinds drive retries and client responses;
/// everything a caller cannot correct is `Internal`, with its cause chain.
#[derive(Debug)]
pub(crate) enum Error {
    Invalid(String),
    InvalidQuery(Vec<crate::domain::project::query::ConditionDiagnostic>),
    RevisionConflict,
    NotFound,
    Busy,
    Closed,
    Internal(anyhow::Error),
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Stored data that breaks an invariant the store maintains.
    pub(crate) fn corrupt(message: impl Into<String>) -> Self {
        Self::Internal(anyhow::Error::msg(message.into()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => output.write_str(message),
            Self::InvalidQuery(diagnostics) => output.write_str(
                diagnostics
                    .first()
                    .map_or("Invalid query.", |diagnostic| diagnostic.message.as_str()),
            ),
            Self::RevisionConflict => {
                output.write_str("The item changed since it was read. Reload it before saving.")
            }
            Self::NotFound => output.write_str("The item does not exist."),
            Self::Busy => output.write_str("The project is busy. Retry the same request."),
            Self::Closed => output.write_str("The project database is closed."),
            // The whole chain, so a flattened message still names the cause.
            Self::Internal(error) => write!(output, "{error:#}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<anyhow::Error> for Error {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        // Interrupts come only from the request deadline or a cancellation.
        if matches!(
            error.sqlite_error_code(),
            Some(
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::OperationInterrupted
            )
        ) {
            Self::Busy
        } else {
            Self::Internal(error.into())
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.into())
    }
}

pub(super) fn poisoned<T>(error: std::sync::PoisonError<T>) -> Error {
    let message = format!("Database connection pool was poisoned: {error}");
    drop(error);
    Error::corrupt(message)
}
