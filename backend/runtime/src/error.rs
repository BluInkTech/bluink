// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use napi::bindgen_prelude::{Error, Status};

pub(crate) fn native_error(error: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, error.to_string())
}

impl From<crate::application::error::ApplicationError> for Error {
    fn from(error: crate::application::error::ApplicationError) -> Self {
        let status = match error {
            crate::application::error::ApplicationError::InvalidRequest(_) => Status::InvalidArg,
            crate::application::error::ApplicationError::Internal(_) => Status::GenericFailure,
        };
        Self::new(status, error.to_string())
    }
}
