// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use napi::bindgen_prelude::{Error, Status};

pub(crate) fn native_error(error: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, error.to_string())
}

impl From<crate::application::error::ApplicationError> for Error {
    fn from(error: crate::application::error::ApplicationError) -> Self {
        let status = match error {
            crate::application::error::ApplicationError::InvalidRequest(_)
            | crate::application::error::ApplicationError::InvalidQuery(_)
            | crate::application::error::ApplicationError::ShuttingDown
            | crate::application::error::ApplicationError::NotInitialized(_) => Status::InvalidArg,
            crate::application::error::ApplicationError::Internal(_) => Status::GenericFailure,
        };
        Self::new(status, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::error::ApplicationError;

    #[test]
    fn lifecycle_errors_preserve_native_status_and_guidance() {
        for failure in [
            ApplicationError::ShuttingDown,
            ApplicationError::NotInitialized("Initialize the project first."),
        ] {
            let message = failure.to_string();
            let native = Error::from(failure);
            assert_eq!(native.status, Status::InvalidArg);
            assert_eq!(native.reason, message);
        }
    }

    #[test]
    fn sourced_internal_error_preserves_native_status_and_message() {
        let failure = ApplicationError::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Read denied",
        ));
        let native = Error::from(failure);
        assert_eq!(native.status, Status::GenericFailure);
        assert_eq!(native.reason, "Read denied");
    }
}
