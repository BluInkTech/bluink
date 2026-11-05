// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{Progress, model::*};
use crate::store::io;
use crate::versioning::{History, Result, failure, repository, transport};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    pub(super) origin: String,
    pub(super) head: Option<String>,
    tags: std::collections::BTreeMap<String, String>,
}

impl History {
    pub(crate) fn sync_status(&self) -> Result<SyncStatus> {
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let origin = origin(&repo)?;
        let outside = match self.check_status_head(&repo) {
            Ok(()) => None,
            Err(error) if error.state == "changed_outside_bluink" => Some(error),
            Err(error) => return Err(error),
        };
        let receipt = if origin.is_some() {
            read_receipt(&repo)?
        } else {
            Receipt::default()
        };
        let tag_match = origin.as_ref() == Some(&receipt.origin)
            && tag_names(&super::tags(&repo)?) == receipt.tags;
        let progress = self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))?;
        let status = self.history_status_at(&repo)?;
        let head = status.head.clone();
        let synced = tag_match && receipt.head == head && !status.dirty;
        let state = if outside.is_some() {
            "changed_outside_bluink"
        } else if origin.is_none() {
            "local_only"
        } else if progress.phase.is_some() {
            "syncing"
        } else if let Some(error) = &progress.error {
            error.state
        } else if status.error.is_some() {
            "checkpoint_failed"
        } else if synced {
            "synced"
        } else if status.dirty {
            "saved"
        } else if head.is_some() {
            "checkpointed"
        } else {
            "ready"
        };
        let phase = progress.phase.clone();
        let error = outside
            .as_ref()
            .or(progress.error.as_ref())
            .map(ToString::to_string)
            .or_else(|| status.error.clone());
        let last_synced_head = (origin.as_ref() == Some(&receipt.origin))
            .then(|| receipt.head.clone())
            .flatten();
        let remote_head = if progress.observed_destination {
            progress.remote_head.map(|id| id.to_string())
        } else {
            last_synced_head.clone()
        };
        drop(progress);
        let mut recovery_refs = Vec::new();
        if matches!(state, "history_needs_repair" | "remote_changed") {
            let tips: std::collections::BTreeSet<_> =
                [head.as_ref(), remote_head.as_ref(), receipt.head.as_ref()]
                    .into_iter()
                    .flatten()
                    .collect();
            for tip in tips {
                let name = format!("refs/bluink/recovery/{tip}");
                match repo.refname_to_id(&name) {
                    Ok(id) => recovery_refs.push(RecoveryRef {
                        name,
                        commit: id.to_string(),
                    }),
                    Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(SyncStatus {
            origin,
            state: state.into(),
            phase,
            error,
            head,
            last_synced_head,
            decisions: 0, // Application fills this from the current index.
            recovery_refs,
            remote_head,
        })
    }

    pub(crate) fn configure_sync(&self, request: &ConfigureSync) -> Result<SyncStatus> {
        let _sync = self
            .sync_serial
            .lock()
            .map_err(|_| failure("Sync lock was poisoned"))?;
        let serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), io::LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        if let Some(url) = &request.origin {
            let remote = transport::Remote::parse(url)?;
            if request.create.unwrap_or(false) {
                remote.initialize()?;
            }
            let auth = self.auth(&remote)?;
            let old_origin = origin(&repo)?;
            let fetched = transport::fetch(&repo, &remote, &auth)?;
            if let Some(head) = fetched.head {
                let incoming = super::validation::config_at(&repo, head)?;
                let current = self
                    .canonical()?
                    .read(crate::store::project::read_config)
                    .map_err(|error| failure(error.to_string()))?;
                if incoming["id"] != current["id"] {
                    return Err(failure(
                        "The destination belongs to another project. Open it as a separate project.",
                    ));
                }
                let local = repository::head(&repo)?
                    .ok_or_else(|| failure("Create a checkpoint before connecting"))?;
                repo.merge_base(local, head).map_err(|_| {
                    failure("The destination has unrelated history. Open it separately.")
                })?;
            }
            if fetched.head.is_none() && old_origin.as_ref() != Some(url) {
                let mut heads = super::history_refs(&repo)?;
                heads.extend(repository::head(&repo)?);
                if transport::usage_for_pointers(&repo, &self.inventory(&repo, &heads, Some(&[]))?)?
                    .1
                    > 0
                {
                    let old=old_origin.as_ref().ok_or_else(||failure("Download missing historical attachments from the previous destination before connecting an empty repository"))?;
                    let source = transport::Remote::parse(old)?;
                    self.transfer_cached(
                        &repo,
                        &source,
                        &self.auth(&source)?,
                        &heads,
                        transport::Direction::Download,
                        true,
                    )?;
                }
            }
            if old_origin.is_some() {
                repo.remote_set_url("origin", url)?;
            } else {
                repo.remote("origin", url)?;
            }
        } else if origin(&repo)?.is_some() {
            repo.remote_delete("origin")?;
        }
        *self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))? = Progress::default();
        drop(serial);
        self.sync_status()
    }

    pub(in crate::versioning) fn sync_phase(&self, phase: &str) -> Result<()> {
        let mut progress = self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))?;
        progress.phase = Some(phase.into());
        progress.error = None;
        drop(progress);
        Ok(())
    }

    pub(crate) fn finish_sync(
        &self,
        error: Option<crate::versioning::Error>,
    ) -> Result<SyncStatus> {
        let mut progress = self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))?;
        progress.phase = None;
        progress.error = error;
        drop(progress);
        if let Err(error) = self.release_pending_sync() {
            tracing::warn!(operation = "sync.finish", error = %error, "Prepared sync candidate remains protected");
        }
        self.sync_status()
    }
}

pub(in crate::versioning) fn origin(repo: &git2::Repository) -> Result<Option<String>> {
    match repo.find_remote("origin") {
        Ok(remote) => Ok(Some(remote.url()?.to_owned())),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn read_receipt(repo: &git2::Repository) -> Result<Receipt> {
    match std::fs::read(repo.path().join("bluink-sync.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Receipt::default()),
        Err(error) => Err(error.into()),
    }
}

fn tag_names(
    tags: &std::collections::BTreeMap<String, git2::Oid>,
) -> std::collections::BTreeMap<String, String> {
    tags.iter()
        .map(|(name, oid)| (name.clone(), oid.to_string()))
        .collect()
}

pub(super) fn write_receipt(
    repo: &git2::Repository,
    origin: String,
    head: git2::Oid,
    tags: &std::collections::BTreeMap<String, git2::Oid>,
) -> Result<()> {
    io::write_json_atomic(
        &repo.path().join("bluink-sync.json"),
        &Receipt {
            origin,
            head: Some(head.to_string()),
            tags: tag_names(tags),
        },
    )
    .map_err(|error| failure(format!("{error:#}")))
}
