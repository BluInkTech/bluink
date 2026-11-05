// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Provider-neutral Git and attachment exchange. The coordinator owns branch
//! acceptance and canonical writes; transports never check out a worktree.

pub(crate) mod credentials;
mod git;
mod http;
pub(crate) mod keys;
pub(crate) mod model;
mod objects;
mod remote;
mod smart;
mod ssh;
#[cfg(test)]
mod tests;
mod wire;

pub(crate) use git::{fetch, push};
#[cfg(test)]
use objects::inventory;
pub(crate) use objects::{Direction, transfer_lfs};
pub(in crate::versioning) use objects::{
    blob_pointers, decision_pointers, local_inventory, shard_pointers, transfer_pointers,
    usage_for_pointers,
};
pub(crate) use remote::Remote;
pub(crate) use smart::register;
pub(crate) use ssh::inspect_host;

/// Secrets are intentionally neither Debug nor Serialize. Passphrases are
/// supplied by the caller for this session and never persisted by this module.
#[derive(Clone, Default)]
pub(crate) struct Auth {
    pub(crate) account: String,
    pub(crate) token: Option<String>,
    pub(crate) private_key: Option<std::path::PathBuf>,
    pub(crate) passphrase: Option<String>,
    /// SHA256:<unpadded base64>, scoped to Remote's exact host and port.
    pub(crate) host_fingerprint: Option<String>,
    pub(crate) use_agent: bool,
    pub(crate) cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Auth {
    pub(crate) fn check_cancelled(&self) -> super::Result<()> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Err(cancelled());
        }
        Ok(())
    }
}

fn cancelled() -> super::Error {
    super::failure("Synchronization cancelled; local changes are preserved")
}
