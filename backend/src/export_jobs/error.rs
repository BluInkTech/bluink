// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Export failures remain independent of application handlers and transport.
use std::fmt;

#[derive(Debug)]
pub(crate) enum Error {
    InvalidRequest(String),
    ShuttingDown,
    Internal(anyhow::Error),
    Database(crate::store::Error),
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub(crate) fn internal(
        message: impl fmt::Display + fmt::Debug + Send + Sync + 'static,
    ) -> Self {
        Self::Internal(anyhow::Error::msg(message))
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => f.write_str(message),
            Self::ShuttingDown => f.write_str("The application is shutting down."),
            Self::Internal(error) => write!(f, "{error:#}"),
            Self::Database(error) => fmt::Display::fmt(error, f),
        }
    }
}
impl std::error::Error for Error {}
impl From<crate::store::Error> for Error {
    fn from(error: crate::store::Error) -> Self {
        Self::Database(error)
    }
}
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
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.into())
    }
}
pub(crate) fn internal_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Internal(anyhow::Error::new(error))
}
pub(crate) fn internal_message(message: impl Into<String>) -> Error {
    Error::internal(message.into())
}
pub(crate) fn invalid_request(message: impl Into<String>) -> Error {
    Error::InvalidRequest(message.into())
}
pub(crate) fn poison_error<T>(error: std::sync::PoisonError<T>) -> Error {
    let message = error.to_string();
    drop(error);
    internal_message(message)
}
