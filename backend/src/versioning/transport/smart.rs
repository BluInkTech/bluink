// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bluink-owned wire transports for libgit2's smart protocol. libgit2 keeps
//! objects, negotiation and packing; `https://` and `ssh://` bytes travel over
//! reqwest/rustls and russh with the same credential and host-trust policy as
//! attachments. Credentials come from the operation scoped by `with_operation`,
//! because custom subtransports cannot reach libgit2's credential callbacks.
use super::super::{Result, failure};
use super::{Auth, Remote};
use std::{
    cell::RefCell,
    sync::{Arc, OnceLock},
};

/// One fetch, push or advertisement, seen by the transports libgit2 creates on
/// this thread while it runs.
pub(super) struct Operation {
    pub(super) remote: Remote,
    pub(super) auth: Auth,
}

impl Operation {
    /// The URL libgit2 asks for must still be the destination the user chose
    /// and supplied credentials for.
    pub(super) fn confirm(&self, url: &str) -> std::result::Result<(), git2::Error> {
        if Remote::parse(url).ok().as_ref() == Some(&self.remote) {
            Ok(())
        } else {
            Err(git2::Error::from_str("Credential destination changed"))
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<Operation>>> = const { RefCell::new(None) };
}

struct Restore(Option<Arc<Operation>>);

impl Drop for Restore {
    fn drop(&mut self) {
        let previous = self.0.take();
        ACTIVE.with(|active| *active.borrow_mut() = previous);
    }
}

/// libgit2 runs a transport on the thread that started the operation, so a
/// thread-local scope reaches exactly the transports created by `run`.
pub(super) fn with_operation<T>(remote: &Remote, auth: &Auth, run: impl FnOnce() -> T) -> T {
    let operation = Arc::new(Operation {
        remote: remote.clone(),
        auth: auth.clone(),
    });
    let _restore = Restore(ACTIVE.with(|active| active.borrow_mut().replace(operation)));
    run()
}

fn active() -> Option<Arc<Operation>> {
    ACTIVE.with(|active| active.borrow().clone())
}

pub(super) fn auth_error() -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Auth,
        git2::ErrorClass::Callback,
        "Needs sign-in",
    )
}

/// Carries the "Host verification required: SHA256:…" message, which
/// `safe_git_error` turns back into the `verify_server` state.
pub(super) fn host_verification(message: &str) -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Certificate,
        git2::ErrorClass::Callback,
        message,
    )
}

/// Registers the transports once. libgit2's transport table is unsynchronized,
/// so the host calls this before opening any repository; transport entry
/// points call it too, which orders every later lookup after registration.
pub(crate) fn register() -> Result<()> {
    static REGISTERED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            for (prefix, rpc) in [("https", true), ("ssh", false)] {
                #[expect(
                    unsafe_code,
                    reason = "libgit2 requires transport registration to be synchronized with transport creation."
                )]
                // SAFETY: OnceLock serializes registration, and every Bluink
                // transport creation either follows host startup or passes
                // through this initializer first.
                let registered = unsafe {
                    git2::transport::register(prefix, move |remote| {
                        let operation = active();
                        if rpc {
                            git2::transport::Transport::smart(
                                remote,
                                true,
                                super::http::GitSubtransport::new(operation),
                            )
                        } else {
                            git2::transport::Transport::smart(
                                remote,
                                false,
                                super::ssh::GitSubtransport::new(operation),
                            )
                        }
                    })
                };
                registered.map_err(|_| format!("Unable to register the {prefix} Git transport"))?;
            }
            Ok(())
        })
        .clone()
        .map_err(failure)
}

/// The operation a newly created subtransport serves, or the error libgit2
/// reports when Git network access happens outside one.
pub(super) fn require(
    operation: Option<&Arc<Operation>>,
    url: &str,
) -> std::result::Result<Arc<Operation>, git2::Error> {
    let operation = operation.ok_or_else(|| {
        git2::Error::from_str("Git network access requires a Bluink synchronization")
    })?;
    operation.confirm(url)?;
    Ok(Arc::clone(operation))
}
