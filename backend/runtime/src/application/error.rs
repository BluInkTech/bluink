// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt;

#[derive(Debug)]
pub(crate) enum ApplicationError {
    InvalidRequest(String),
    Internal(String),
}

pub(crate) type Result<T> = std::result::Result<T, ApplicationError>;

pub(crate) fn internal_error(error: impl fmt::Display) -> ApplicationError {
    ApplicationError::Internal(error.to_string())
}

pub(crate) fn invalid_request(error: impl fmt::Display) -> ApplicationError {
    ApplicationError::InvalidRequest(error.to_string())
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) | Self::Internal(message) => output.write_str(message),
        }
    }
}

impl std::error::Error for ApplicationError {}

impl From<std::io::Error> for ApplicationError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<crate::engine::project::write::MutationError> for ApplicationError {
    fn from(error: crate::engine::project::write::MutationError) -> Self {
        match error {
            crate::engine::project::write::MutationError::Expected(message) => {
                Self::InvalidRequest(message)
            }
            crate::engine::project::write::MutationError::Unexpected(message) => {
                Self::Internal(message)
            }
        }
    }
}
