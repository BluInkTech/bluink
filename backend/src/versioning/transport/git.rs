// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::{Result, failure};
use super::{Auth, Remote};
use git2::{Oid, RemoteCallbacks, Repository};
use std::collections::BTreeMap;

pub(crate) struct Fetched {
    pub(crate) head: Option<Oid>,
    pub(crate) tags: BTreeMap<String, Oid>,
}

const VERSIONS: &str = "refs/tags/bluink/versions/";

/// Progress, cancellation and push results. Credentials and host trust belong
/// to the Bluink transports, which receive them through `smart::with_operation`.
pub(super) fn callbacks(auth: &Auth) -> RemoteCallbacks<'_> {
    let mut callbacks = RemoteCallbacks::new();
    // Progress callbacks run only while data arrives, so they cannot detect a
    // stall; a total limit would only abort large transfers that are progressing.
    callbacks.transfer_progress(move |_| auth.check_cancelled().is_ok());
    callbacks.sideband_progress(move |_| auth.check_cancelled().is_ok());
    callbacks.push_negotiation(move |_| {
        auth.check_cancelled()
            .map_err(|_| git2::Error::from_str("Synchronization cancelled"))
    });
    callbacks.push_update_reference(|_, status| {
        if status.is_some() {
            Err(git2::Error::new(
                git2::ErrorCode::NotFastForward,
                git2::ErrorClass::Callback,
                "Push rejected; fetch and retry without force",
            ))
        } else {
            Ok(())
        }
    });
    callbacks
}

fn check_local(remote: &Remote) -> Result<()> {
    if matches!(remote, Remote::Local(_)) {
        remote
            .local_repository()
            .map_err(|error| error.classified("destination_unavailable"))?;
    }
    Ok(())
}

fn advertised(repo: &Repository, remote: &Remote, auth: &Auth) -> Result<Fetched> {
    auth.check_cancelled()?;
    check_local(remote)?;
    super::register()?;
    let mut connection = repo.remote_anonymous(&remote.url()?)?;
    let heads = super::smart::with_operation(remote, auth, || -> Result<Vec<(String, Oid)>> {
        let connected = connection
            .connect_auth(git2::Direction::Fetch, Some(callbacks(auth)), None)
            .map_err(|error| diagnose(remote, auth, error, false))?;
        Ok(connected
            .list()
            .map_err(safe_git_error)?
            .iter()
            .map(|head| (head.name().to_owned(), head.oid()))
            .collect())
    })?;
    let mut result = Fetched {
        head: None,
        tags: BTreeMap::new(),
    };
    let mut has_references = false;
    for (name, oid) in heads {
        has_references |= name.starts_with("refs/");
        if name == "refs/heads/main" {
            result.head = Some(oid);
        }
        if name.starts_with(VERSIONS) && !name.ends_with("^{}") {
            crate::domain::project::identity::ItemId::try_from(&name[VERSIONS.len()..])?;
            result.tags.insert(name, oid);
        }
    }
    if result.head.is_none() && has_references {
        return Err(failure(
            "Destination contains history without Bluink's main branch; open the existing project instead",
        ));
    }
    Ok(result)
}

pub(crate) fn fetch(repo: &Repository, remote: &Remote, auth: &Auth) -> Result<Fetched> {
    let advertised = advertised(repo, remote, auth)?;
    let mut specs = Vec::new();
    if advertised.head.is_some() {
        specs.push("+refs/heads/main:refs/bluink/incoming/main".to_owned());
    }
    for name in advertised.tags.keys() {
        specs.push(format!(
            "+{name}:refs/bluink/incoming/tags/{}",
            &name[VERSIONS.len()..]
        ));
    }
    if specs.is_empty() {
        return Ok(advertised);
    }
    let mut fetch = git2::FetchOptions::new();
    fetch
        .remote_callbacks(callbacks(auth))
        .download_tags(git2::AutotagOption::None)
        .follow_redirects(git2::RemoteRedirect::None);
    let mut connection = repo.remote_anonymous(&remote.url()?)?;
    super::smart::with_operation(remote, auth, || {
        connection
            .fetch(&specs, Some(&mut fetch), Some("Bluink fetch"))
            .map_err(|error| diagnose(remote, auth, error, false))
    })?;
    // Read the actual received refs, since the remote may have moved between
    // advertisement and pack transfer. Never pretend the earlier OID was fetched.
    Ok(Fetched {
        head: advertised
            .head
            .map(|_| repo.refname_to_id("refs/bluink/incoming/main"))
            .transpose()?,
        tags: advertised
            .tags
            .keys()
            .map(|name| {
                Ok((
                    name.clone(),
                    repo.refname_to_id(&format!(
                        "refs/bluink/incoming/tags/{}",
                        &name[VERSIONS.len()..]
                    ))?,
                ))
            })
            .collect::<Result<_>>()?,
    })
}

pub(crate) fn push(
    repo: &Repository,
    remote: &Remote,
    auth: &Auth,
    head: Oid,
    tags: &BTreeMap<String, Oid>,
) -> Result<()> {
    check_local(remote)?;
    let before = advertised(repo, remote, auth)?;
    for (name, id) in tags {
        if !name.starts_with(VERSIONS) {
            return Err(failure("Only Bluink named-version tags can be pushed"));
        }
        if before.tags.get(name).is_some_and(|previous| previous != id) {
            return Err(failure(
                "Named version differs on the destination; neither copy was replaced",
            )
            .classified("push_rejected"));
        }
    }
    let mut specs = vec![format!("{head}:refs/heads/main")];
    specs.extend(
        tags.iter()
            .filter(|(name, _)| !before.tags.contains_key(*name))
            .map(|(name, id)| format!("{id}:{name}")),
    );
    let mut options = git2::PushOptions::new();
    options
        .remote_callbacks(callbacks(auth))
        .follow_redirects(git2::RemoteRedirect::None);
    let mut connection = repo.remote_anonymous(&remote.url()?)?;
    super::smart::with_operation(remote, auth, || {
        connection
            .push(&specs, Some(&mut options))
            .map_err(|error| diagnose(remote, auth, error, true))
    })
}

fn diagnose(remote: &Remote, auth: &Auth, error: git2::Error, push: bool) -> super::super::Error {
    let original = safe_git_error(error);
    if original.state != "destination_unavailable" || auth.check_cancelled().is_err() {
        return original;
    }
    match remote {
        Remote::Local(_) => None,
        Remote::Https(url) => super::http::diagnose_git(url, auth, push),
        Remote::Ssh(_) => super::ssh::diagnose_git(remote, auth),
    }
    .unwrap_or(original)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Error conversion consumes the transport error in map_err without retaining secret-bearing diagnostics."
)]
pub(super) fn safe_git_error(error: git2::Error) -> super::super::Error {
    // Only our own fixed messages and the public host fingerprint are exposed;
    // provider diagnostics may echo credentials or signed URLs.
    match error.code() {
        git2::ErrorCode::Certificate if error.class() == git2::ErrorClass::Callback => {
            use base64::Engine as _;
            if let Some(hash) = error
                .message()
                .strip_prefix("Host verification required: SHA256:")
                .and_then(|value| {
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(value)
                        .ok()
                })
                && hash.len() == 32
            {
                return failure(format!(
                    "Host verification required: {}",
                    super::ssh::fingerprint(&hash)
                ))
                .classified("verify_server");
            }
            failure("Inspect and verify the SSH server fingerprint").classified("verify_server")
        }
        git2::ErrorCode::Auth => failure("Needs sign-in: check the account, key or passphrase")
            .classified("needs_sign_in"),
        git2::ErrorCode::NotFastForward | git2::ErrorCode::Modified => {
            failure("Push rejected; fetch and retry without force").classified("push_rejected")
        }
        git2::ErrorCode::Timeout => {
            failure("Destination connection timed out").classified("offline")
        }
        _ => failure(format!(
            "Git transport failed ({:?}/{:?}); local changes are preserved",
            error.class(),
            error.code()
        ))
        .classified("destination_unavailable"),
    }
}
