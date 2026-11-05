// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::{Result, failure};
use super::{
    Auth, Remote,
    model::{AuthInput, AuthStatus, TrustInput},
};
use crate::store::io::{LockMode, lock_file};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Database {
    accounts: BTreeMap<String, BTreeMap<String, Account>>,
    selected: BTreeMap<String, String>,
    hosts: BTreeMap<String, String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Account {
    token: Option<String>,
    private_key: Option<PathBuf>,
    use_agent: bool,
}

type SessionKey = (PathBuf, String, String);
static PASSPHRASES: OnceLock<Mutex<BTreeMap<SessionKey, String>>> = OnceLock::new();

fn database(user: &Path) -> Result<Database> {
    let path = user.join("credentials.json");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(failure("Credential store must be a regular file"))
        }
        Ok(_) => serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|_| failure("Credential store is invalid; local history is still available")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Database::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(user: &Path, database: &Database) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(user)?;
    // tempfile creates owner-only files on Unix; Windows inherits the user's ACL.
    temporary.write_all(&serde_json::to_vec_pretty(database)?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(user.join("credentials.json"))
        .map_err(|e| e.error)?;
    Ok(())
}

impl Auth {
    pub(crate) fn load(user: &Path, remote: &Remote) -> Result<Self> {
        if matches!(remote, Remote::Local(_)) {
            return Ok(Self::default());
        }
        std::fs::create_dir_all(user)?;
        let _lock = lock_file(&user.join("credentials.lock"), LockMode::Shared)
            .map_err(|error| failure(format!("{error:#}")))?;
        let database = database(user)?;
        let origin = remote.url()?;
        let account = database.selected.get(&origin).cloned().unwrap_or_default();
        let saved = database
            .accounts
            .get(&origin)
            .and_then(|accounts| accounts.get(&account));
        let passphrase = PASSPHRASES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| failure("Credential session lock failed"))?
            .get(&(user.to_owned(), origin, account.clone()))
            .cloned();
        Ok(Self {
            account,
            token: saved.and_then(|saved| saved.token.clone()),
            private_key: saved.and_then(|saved| saved.private_key.clone()),
            passphrase,
            host_fingerprint: remote
                .host_key()
                .and_then(|host| database.hosts.get(&host).cloned()),
            use_agent: saved.is_some_and(|saved| saved.use_agent),
            cancel: None,
        })
    }

    pub(crate) fn status(&self) -> AuthStatus {
        AuthStatus {
            account: self.account.clone(),
            has_token: self.token.is_some(),
            private_key: self
                .private_key
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            host_fingerprint: self.host_fingerprint.clone(),
            use_agent: self.use_agent,
        }
    }
}

pub(crate) fn configure_auth(user: &Path, remote: &Remote, input: AuthInput) -> Result<AuthStatus> {
    if matches!(remote, Remote::Local(_)) {
        return Err(failure("Local repositories do not use credentials"));
    }
    if input.account.trim().is_empty() || input.account.chars().any(char::is_control) {
        return Err(failure("Choose an account name"));
    }
    let private_key = input.private_key.map(PathBuf::from);
    if let Some(key) = &private_key
        && (!key.is_absolute() || !key.is_file())
    {
        return Err(failure(
            "Choose an existing private key file using its absolute path",
        ));
    }
    std::fs::create_dir_all(user)?;
    let lock = lock_file(&user.join("credentials.lock"), LockMode::Exclusive)
        .map_err(|error| failure(format!("{error:#}")))?;
    let mut database = database(user)?;
    let origin = remote.url()?;
    let accounts = database.accounts.entry(origin.clone()).or_default();
    let mut sessions = PASSPHRASES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| failure("Credential session lock failed"))?;
    let key = (user.to_owned(), origin.clone(), input.account.clone());
    let clear_passphrase = if input.forget {
        accounts.remove(&input.account);
        if database.selected.get(&origin) == Some(&input.account) {
            database.selected.remove(&origin);
        }
        true
    } else {
        let saved = accounts.entry(input.account.clone()).or_default();
        if let Some(use_agent) = input.use_agent {
            saved.use_agent = use_agent;
        }
        if let Some(token) = input.token {
            saved.token = (!token.is_empty()).then_some(token);
        }
        let key_changed = private_key
            .as_ref()
            .is_some_and(|path| saved.private_key.as_ref() != Some(path));
        if let Some(private_key) = private_key {
            saved.private_key = Some(private_key);
        }
        database.selected.insert(origin, input.account);
        key_changed
    };
    save(user, &database)?;
    if let Some(passphrase) = input.passphrase.filter(|_| !input.forget) {
        sessions.insert(key, passphrase);
    } else if clear_passphrase {
        sessions.remove(&key);
    }
    drop(sessions);
    drop(lock);
    Ok(Auth::load(user, remote)?.status())
}

pub(crate) fn trust_host(user: &Path, remote: &Remote, input: &TrustInput) -> Result<AuthStatus> {
    use base64::Engine as _;
    let fingerprint = input.fingerprint.strip_prefix("SHA256:").and_then(|v| {
        base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(v)
            .ok()
    });
    if fingerprint.is_none_or(|bytes| bytes.len() != 32) {
        return Err(failure("Invalid SHA-256 host fingerprint"));
    }
    let host = remote
        .host_key()
        .ok_or_else(|| failure("Only SSH repositories have host keys"))?;
    std::fs::create_dir_all(user)?;
    let lock = lock_file(&user.join("credentials.lock"), LockMode::Exclusive)
        .map_err(|error| failure(format!("{error:#}")))?;
    let mut database = database(user)?;
    if database.hosts.get(&host) != input.previous_fingerprint.as_ref() {
        return Err(failure(
            "Host trust changed; inspect the key again before replacing it",
        ));
    }
    database.hosts.insert(host, input.fingerprint.clone());
    save(user, &database)?;
    drop(lock);
    Ok(Auth::load(user, remote)?.status())
}
