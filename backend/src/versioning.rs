// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Embedded project history and synchronization. Transports exchange Git
//! objects and LFS bytes, never a live checkout.

mod checkpoint;
pub(crate) mod checkpoints;
mod coordinator;
mod history;
mod lfs;
pub(crate) mod model;
mod packing;
mod repository;
mod restore;
mod scheduling;
mod session;
pub(crate) mod sync;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
pub(crate) mod transport;
pub(crate) use coordinator::History;
pub(crate) use session::HistorySession;

#[derive(Debug, Clone)]
pub(crate) struct Error {
    message: String,
    pub(crate) state: &'static str,
}
impl Error {
    pub(crate) fn classified(mut self, state: &'static str) -> Self {
        if self.state == "needs_attention" {
            self.state = state;
        }
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
macro_rules! from_error {
    ($($kind:ty),+ $(,)?) => { $(impl From<$kind> for Error {
        fn from(value: $kind) -> Self { failure(value.to_string()) }
    })+ };
}
from_error!(std::io::Error, git2::Error, serde_json::Error, String);
pub(crate) type Result<T> = std::result::Result<T, Error>;
fn failure(message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
        state: "needs_attention",
    }
}

pub(crate) fn log_build_info() {
    let version = git2::Version::get();
    tracing::debug!(
        target: "bluink::versioning",
        libgit2 = ?version.libgit2_version(),
        vendored = version.vendored(),
        transports = "rustls+russh",
        "Embedded Git build"
    );
}

#[cfg(test)]
mod build_tests {
    /// libgit2 carries no network stack of its own; Bluink's HTTPS and SSH
    /// transports serve those URL schemes instead.
    #[test]
    fn native_module_links_vendored_git_with_bluink_transports() {
        let version = git2::Version::get();
        assert!(version.vendored());
        assert!(!version.https());
        assert!(!version.ssh());
        super::transport::register().unwrap();
        super::transport::register().unwrap();
    }
}
