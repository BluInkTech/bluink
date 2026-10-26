// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt;

#[derive(Debug)]
pub(crate) enum ApplicationError {
    ShuttingDown,
    NotInitialized(&'static str),
    InvalidRequest(String),
    InvalidQuery(Vec<crate::query::document::ConditionDiagnostic>),
    Internal(Box<dyn std::error::Error + Send + Sync>),
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
            Err(internal_error(format!("{name} lock was poisoned."))),
        ),
    }
}

pub(crate) fn internal_error(
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> ApplicationError {
    ApplicationError::Internal(error.into())
}

/// Poison errors borrow their guard and cannot escape the lock's lifetime.
pub(crate) fn poison_error<T>(error: std::sync::PoisonError<T>) -> ApplicationError {
    let message = error.to_string();
    drop(error);
    internal_error(message)
}

pub(crate) fn invalid_request(error: impl fmt::Display) -> ApplicationError {
    ApplicationError::InvalidRequest(error.to_string())
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShuttingDown => output.write_str("The application is shutting down."),
            Self::NotInitialized(message) => output.write_str(message),
            Self::InvalidRequest(message) => output.write_str(message),
            Self::InvalidQuery(diagnostics) => output.write_str(
                diagnostics
                    .first()
                    .map_or("Invalid search conditions.", |entry| entry.message.as_str()),
            ),
            Self::Internal(error) => fmt::Display::fmt(error, output),
        }
    }
}

impl std::error::Error for ApplicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ApplicationError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(Box::new(error))
    }
}

impl From<crate::engine::project::item::ItemReadError> for ApplicationError {
    fn from(error: crate::engine::project::item::ItemReadError) -> Self {
        Self::Internal(Box::new(error))
    }
}

impl From<crate::engine::project::write::MutationError> for ApplicationError {
    fn from(error: crate::engine::project::write::MutationError) -> Self {
        match error {
            crate::engine::project::write::MutationError::Validation(message)
            | crate::engine::project::write::MutationError::NotFound(message)
            | crate::engine::project::write::MutationError::RevisionConflict(message) => {
                Self::InvalidRequest(message)
            }
            crate::engine::project::write::MutationError::Internal(message) => {
                Self::Internal(message.into())
            }
            crate::engine::project::write::MutationError::Serialization(error) => {
                Self::Internal(Box::new(error))
            }
            crate::engine::project::write::MutationError::Storage(error) => {
                Self::Internal(Box::new(error))
            }
            crate::engine::project::write::MutationError::Read(error) => Self::from(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::project::write::MutationError;
    use std::error::Error;

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
        let error = MutationError::Serialization(serde_json::to_vec(&invalid).unwrap_err());
        let message = error.to_string();
        assert!(error.source().unwrap().is::<serde_json::Error>());
        let application = ApplicationError::from(error);
        assert_eq!(application.to_string(), message);
        assert!(application.source().unwrap().is::<serde_json::Error>());
    }

    #[test]
    fn canonical_json_cause_survives_direct_mutation_and_report_boundaries() {
        use crate::engine::project::{identity::ItemId, item, reports::ReportError};

        let root = tempfile::tempdir().unwrap();
        let id = "01a0e2a0-0000-7000-8000-000000000001";
        let identity = ItemId::try_from(id).unwrap();
        std::fs::create_dir_all(identity.directory(root.path())).unwrap();
        let path = identity.path(root.path());
        std::fs::write(&path, b"{broken JSON").unwrap();
        for boundary in 0..3 {
            let read = item::read_project_item(root.path(), id).unwrap_err();
            let application = match boundary {
                0 => ApplicationError::from(read),
                1 => ApplicationError::from(MutationError::Read(read)),
                _ => ApplicationError::from(ReportError::Read(read)),
            };
            assert!(matches!(application, ApplicationError::Internal(_)));
            assert!(
                application
                    .to_string()
                    .contains(&path.display().to_string())
            );
            let read = application
                .source()
                .unwrap()
                .downcast_ref::<item::ItemReadError>()
                .unwrap();
            assert!(read.source().unwrap().is::<serde_json::Error>());
        }
        root.close().unwrap();
    }

    #[test]
    fn filesystem_error_preserves_kind_through_application_boundary() {
        let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Read denied");
        let application = ApplicationError::from(cause);
        assert_eq!(application.to_string(), "Read denied");
        let source = application
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    }
    #[test]
    fn internal_adapter_preserves_concrete_causes_and_message_only_errors() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Denied");
        let error = super::internal_error(io);
        assert_eq!(error.to_string(), "Denied");
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let error = super::internal_error(json);
        assert!(error.source().unwrap().is::<serde_json::Error>());
        for error in [
            super::internal_error("Static diagnostic"),
            super::internal_error(String::from("Owned diagnostic")),
        ] {
            assert!(error.to_string().contains("diagnostic"));
        }
    }
}
