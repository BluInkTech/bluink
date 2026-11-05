// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt;

/// The kinds the HTTP layer answers differently. Everything the client cannot
/// correct is `Internal`, which keeps its cause chain for the server log.
#[derive(Debug)]
pub(crate) enum ApplicationError {
    ProjectBusy,
    ShuttingDown,
    NotInitialized(&'static str),
    InvalidRequest(String),
    StaleDecision(String),
    InvalidQuery(Vec<crate::structured_query::document::ConditionDiagnostic>),
    Internal(anyhow::Error),
}

pub(crate) type Result<T> = std::result::Result<T, ApplicationError>;

/// Cleanup may recover a poisoned guard to release owned resources, but must
/// return the accompanying error after cleanup. Normal operations must not use
/// this helper: poisoning still rejects reads, writes and work admission.
pub(crate) fn recover_for_cleanup<T>(
    result: std::sync::LockResult<T>,
    name: &str,
) -> (T, Result<()>) {
    match result {
        Ok(guard) => (guard, Ok(())),
        Err(error) => (
            error.into_inner(),
            Err(internal_message(format!("{name} lock was poisoned."))),
        ),
    }
}

/// An unexpected failure with a cause the server log should keep.
pub(crate) fn internal_error(error: impl Into<anyhow::Error>) -> ApplicationError {
    ApplicationError::Internal(error.into())
}

/// An unexpected failure described only by a message.
pub(crate) fn internal_message(
    message: impl fmt::Display + fmt::Debug + Send + Sync + 'static,
) -> ApplicationError {
    ApplicationError::Internal(anyhow::Error::msg(message))
}

/// Poison errors borrow their guard and cannot escape the lock's lifetime.
pub(crate) fn poison_error<T>(error: std::sync::PoisonError<T>) -> ApplicationError {
    let message = error.to_string();
    drop(error);
    internal_message(message)
}

pub(crate) fn invalid_request(error: impl fmt::Display) -> ApplicationError {
    ApplicationError::InvalidRequest(error.to_string())
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectBusy => output.write_str("The project is busy. Retry the same request."),
            Self::ShuttingDown => output.write_str("The application is shutting down."),
            Self::NotInitialized(message) => output.write_str(message),
            Self::InvalidRequest(message) | Self::StaleDecision(message) => {
                output.write_str(message)
            }
            Self::InvalidQuery(diagnostics) => output.write_str(
                diagnostics
                    .first()
                    .map_or("Invalid search conditions.", |entry| entry.message.as_str()),
            ),
            // The whole chain, so a flattened message still names the cause.
            Self::Internal(error) => write!(output, "{error:#}"),
        }
    }
}

impl std::error::Error for ApplicationError {}

impl From<anyhow::Error> for ApplicationError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<std::io::Error> for ApplicationError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crate::store::Error> for ApplicationError {
    fn from(error: crate::store::Error) -> Self {
        match error {
            crate::store::Error::Busy => Self::ProjectBusy,
            crate::store::Error::Closed => Self::ShuttingDown,
            crate::store::Error::Invalid(message) => Self::InvalidRequest(message),
            crate::store::Error::InvalidQuery(diagnostics) => Self::InvalidQuery(diagnostics),
            crate::store::Error::NotFound | crate::store::Error::RevisionConflict => {
                Self::InvalidRequest(error.to_string())
            }
            crate::store::Error::Internal(error) => Self::Internal(error),
        }
    }
}

impl From<crate::application::error::MutationError> for ApplicationError {
    fn from(error: crate::application::error::MutationError) -> Self {
        use crate::application::error::MutationError;
        match error {
            MutationError::Validation(message)
            | MutationError::NotFound(message)
            | MutationError::RevisionConflict(message) => Self::InvalidRequest(message),
            MutationError::Internal(error) => Self::Internal(error),
            MutationError::Database(error) => Self::from(error),
        }
    }
}

/// Mutation failures. Validation, missing items and stale revisions are the
/// caller's to correct; `Database` keeps the store's retry classification.
#[derive(Debug)]
pub(crate) enum MutationError {
    Validation(String),
    NotFound(String),
    RevisionConflict(String),
    Internal(anyhow::Error),
    Database(crate::store::Error),
}

impl MutationError {
    pub(crate) fn internal(
        message: impl std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
    ) -> Self {
        Self::Internal(anyhow::Error::msg(message))
    }
}

impl From<anyhow::Error> for MutationError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

impl From<serde_json::Error> for MutationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crate::store::Error> for MutationError {
    fn from(error: crate::store::Error) -> Self {
        match error {
            crate::store::Error::Invalid(message) => Self::Validation(message),
            crate::store::Error::NotFound => Self::NotFound(error.to_string()),
            crate::store::Error::RevisionConflict => Self::RevisionConflict(error.to_string()),
            error => Self::Database(error),
        }
    }
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message)
            | Self::NotFound(message)
            | Self::RevisionConflict(message) => formatter.write_str(message),
            Self::Internal(error) => write!(formatter, "{error:#}"),
            Self::Database(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for MutationError {}

impl From<crate::export_jobs::error::Error> for ApplicationError {
    fn from(error: crate::export_jobs::error::Error) -> Self {
        match error {
            crate::export_jobs::error::Error::InvalidRequest(message) => {
                Self::InvalidRequest(message)
            }
            crate::export_jobs::error::Error::ShuttingDown => Self::ShuttingDown,
            crate::export_jobs::error::Error::Internal(error) => Self::Internal(error),
            crate::export_jobs::error::Error::Database(error) => Self::from(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::error::MutationError;

    #[test]
    fn export_errors_keep_retry_and_shutdown_classification_at_the_application_boundary() {
        use crate::export_jobs::error::Error;
        assert!(matches!(
            ApplicationError::from(Error::Database(crate::store::Error::Busy)),
            ApplicationError::ProjectBusy
        ));
        assert!(matches!(
            ApplicationError::from(Error::Database(crate::store::Error::Closed)),
            ApplicationError::ShuttingDown
        ));
        assert!(matches!(
            ApplicationError::from(Error::ShuttingDown),
            ApplicationError::ShuttingDown
        ));
        assert!(
            matches!(ApplicationError::from(Error::InvalidRequest("Fix this export".into())), ApplicationError::InvalidRequest(message) if message == "Fix this export")
        );
    }

    #[test]
    fn mutation_classification_preserves_client_errors_and_serialization_cause() {
        for error in [
            MutationError::Validation("Invalid item".into()),
            MutationError::NotFound("Item not found".into()),
            MutationError::RevisionConflict("Reload before saving".into()),
        ] {
            let message = error.to_string();
            assert!(matches!(
                ApplicationError::from(error),
                ApplicationError::InvalidRequest(actual) if actual == message
            ));
        }
        // JSON object keys cannot be compound values: preserve the actual
        // serializer error rather than flattening it at the engine boundary.
        let invalid = std::collections::BTreeMap::from([((1, 2), "value")]);
        let error = MutationError::from(serde_json::to_vec(&invalid).unwrap_err());
        let message = error.to_string();
        let application = ApplicationError::from(error);
        assert_eq!(application.to_string(), message);
        assert!(matches!(
            application,
            ApplicationError::Internal(cause) if cause.is::<serde_json::Error>()
        ));
    }

    #[test]
    fn busy_and_closed_reads_stay_retryable_or_shutting_down() {
        assert!(matches!(
            ApplicationError::from(crate::store::Error::Busy),
            ApplicationError::ProjectBusy
        ));
        assert!(matches!(
            ApplicationError::from(MutationError::Database(crate::store::Error::Closed)),
            ApplicationError::ShuttingDown
        ));
        let interrupted = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
            None,
        );
        assert!(matches!(
            crate::store::Error::from(interrupted),
            crate::store::Error::Busy
        ));
    }

    #[test]
    fn filesystem_error_preserves_kind_through_application_boundary() {
        let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Read denied");
        let application = ApplicationError::from(cause);
        assert_eq!(application.to_string(), "Read denied");
        let ApplicationError::Internal(cause) = application else {
            panic!("expected an internal failure");
        };
        assert_eq!(
            cause
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn internal_failures_render_their_whole_chain() {
        use anyhow::Context;
        let failure = Err::<(), _>(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Denied",
        ))
        .context("Cannot open project.json")
        .unwrap_err();
        let error = internal_error(failure);
        assert_eq!(error.to_string(), "Cannot open project.json: Denied");
        assert!(matches!(
            &error,
            ApplicationError::Internal(cause) if cause.is::<std::io::Error>()
        ));
        assert_eq!(
            internal_message("Static diagnostic").to_string(),
            "Static diagnostic"
        );
    }
}
