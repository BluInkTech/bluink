// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Provider-neutral synchronization. Fetch/merge preparation holds no canonical
//! mutation gate; acceptance reuses the durable restore publication journal.
use super::{History, Result, failure, repository, transport};
use crate::store::io::{self, LockMode};
use git2::{Oid, Repository};
use std::collections::BTreeMap;

type Files = BTreeMap<String, Oid>;

const PENDING: &str = "refs/bluink/pending/sync";

mod assets;
pub(crate) mod auth;
mod candidate;
mod cloning;
mod decisions;
pub(crate) mod model;
mod packages;
mod state;
pub(super) use state::origin;
#[cfg(test)]
mod tests;
mod theme_dependencies;
pub(crate) mod validation;

#[derive(Default)]
pub(super) struct Progress {
    phase: Option<String>,
    error: Option<super::Error>,
    last_automatic_attempt: Option<std::time::Instant>,
    observed_destination: bool,
    remote_head: Option<Oid>,
}

pub(crate) struct Prepared {
    pub(crate) previous: Oid,
    pub(crate) candidate: Oid,
    generation: i64,
    origin: Option<String>,
    remote_head: Option<Oid>,
    tags: BTreeMap<String, Oid>,
}

impl History {
    fn inventory(
        &self,
        repo: &Repository,
        heads: &[Oid],
        exclude: Option<&[Oid]>,
    ) -> Result<Vec<super::lfs::Pointer>> {
        let canonical = self.canonical()?;
        let mut work = super::checkpoints::inventory::Work::default();
        let objects = match exclude {
            Some(known) => {
                super::checkpoints::inventory::history(repo, &canonical, heads, known, &mut work)?
            }
            None => super::checkpoints::inventory::current(repo, &canonical, heads, &mut work)?,
        };
        tracing::debug!(
            operation = "sync.inventory",
            required_objects = objects.len(),
            commit_diffs = work.commit_diffs,
            pointer_blobs = work.pointer_blobs,
            tree_shard_fills = work.shard_fills,
            "Prepared attachment inventory"
        );
        Ok(objects)
    }

    fn transfer_cached(
        &self,
        repo: &Repository,
        remote: &transport::Remote,
        auth: &transport::Auth,
        heads: &[Oid],
        direction: transport::Direction,
        history: bool,
    ) -> Result<()> {
        let objects = self.inventory(repo, heads, history.then_some(&[]))?;
        transport::transfer_pointers(repo, remote, auth, &objects, direction)
    }

    pub(super) fn ensure_history_attachments(&self, repo: &Repository, commit: Oid) -> Result<()> {
        let objects = self.inventory(repo, &[commit], None)?;
        if transport::usage_for_pointers(repo, &objects)?.1 == 0 {
            return Ok(());
        }
        let origin = state::origin(repo)?.ok_or_else(||failure("This checkpoint needs attachments that are not available offline. Connect its sync destination to download them."))?;
        let remote = transport::Remote::parse(&origin)?;
        transport::transfer_pointers(
            repo,
            &remote,
            &self.auth(&remote)?,
            &objects,
            transport::Direction::Download,
        )
    }

    pub(super) fn ensure_history_asset(
        &self,
        repo: &Repository,
        pointer: &super::lfs::Pointer,
    ) -> Result<()> {
        if pointer.available_path(repo.path())?.is_some() {
            return Ok(());
        }
        let origin = state::origin(repo)?.ok_or_else(||failure("This attachment is not available offline. Connect its sync destination to download it."))?;
        let remote = transport::Remote::parse(&origin)?;
        transport::transfer_pointers(
            repo,
            &remote,
            &self.auth(&remote)?,
            std::slice::from_ref(pointer),
            transport::Direction::Download,
        )
    }
    pub(crate) fn claim_automatic_sync(&self) -> Result<bool> {
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        if state::origin(&repo)?.is_none() {
            return Ok(false);
        }
        let mut progress = self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))?;
        let now = std::time::Instant::now();
        if progress
            .last_automatic_attempt
            .is_some_and(|at| now.duration_since(at) < std::time::Duration::from_mins(1))
        {
            return Ok(false);
        }
        let receipt = state::read_receipt(&repo)?;
        let status = self.status()?;
        let due = self.due()?
            || (!status.dirty
                && (receipt.head != status.head
                    || progress.last_automatic_attempt.is_none_or(|at| {
                        now.duration_since(at) >= std::time::Duration::from_mins(5)
                    })));
        if due {
            progress.last_automatic_attempt = Some(now);
        }
        drop(progress);
        Ok(due)
    }
    pub(crate) fn history_download(&self) -> Result<model::HistoryDownload> {
        let serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let mut heads =
            vec![repository::head(&repo)?.ok_or_else(|| failure("Project has no checkpoint"))?];
        heads.extend(history_refs(&repo)?);
        drop(serial);
        let (reachable_bytes, missing_bytes) =
            transport::usage_for_pointers(&repo, &self.inventory(&repo, &heads, Some(&[]))?)?;
        Ok(model::HistoryDownload {
            reachable_bytes,
            missing_bytes,
        })
    }
    pub(crate) fn prepare_sync(&self) -> Result<Prepared> {
        self.sync_phase("Fetching")?;
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        let origin =
            state::origin(&repo)?.ok_or_else(|| failure("Choose a sync destination first"))?;
        let remote = transport::Remote::parse(&origin)?;
        let auth = self.auth(&remote)?;
        self.checkpoint_locked(&repo)
            .map_err(|e| e.classified("checkpoint_failed"))?;
        let head = repository::head(&repo)?.ok_or_else(|| failure("Project has no checkpoint"))?;
        let generation = self.generation()?;
        let incoming = transport::fetch(&repo, &remote, &auth).map_err(|e| {
            e.classified(if matches!(remote, transport::Remote::Local(_)) {
                "destination_unavailable"
            } else {
                "offline"
            })
        })?;
        self.record_remote_head(incoming.head)?;
        check_remote_history(&repo, &origin, incoming.head)?;
        if let Some(other) = incoming.head
            && other != head
            && !repo.graph_descendant_of(head, other)?
        {
            if validation::config_at(&repo, other)?["id"]
                != validation::config_at(&repo, head)?["id"]
            {
                retain_history(&repo, head)?;
                retain_history(&repo, other)?;
                return Err(failure(
                    "Destination belongs to another project; both histories are preserved",
                )
                .classified("remote_changed"));
            }
            self.sync_phase("Downloading attachments")?;
            self.transfer_cached(
                &repo,
                &remote,
                &auth,
                &[other],
                transport::Direction::Download,
                false,
            )
            .map_err(|e| e.classified("attachments_unavailable"))?;
        }
        let (candidate, merged) = match incoming.head {
            None => (head, None),
            Some(other) => match self.merge_heads(&repo, head, other) {
                Ok(candidate) => candidate,
                Err(error) => {
                    retain_history(&repo, head)?;
                    retain_history(&repo, other)?;
                    return Err(error.classified("history_needs_repair"));
                }
            },
        };
        // Protect prepared objects while application publication waits for its
        // read barrier. Packing and restart cannot discard accepted candidates.
        repo.reference(PENDING, candidate, true, "Prepared sync")?;
        if candidate != head {
            if Some(candidate) != incoming.head {
                self.sync_phase("Downloading attachments")?;
                self.transfer_cached(
                    &repo,
                    &remote,
                    &auth,
                    &[candidate],
                    transport::Direction::Download,
                    false,
                )
                .map_err(|e| e.classified("attachments_unavailable"))?;
            }
            validate_incoming(&repo, head, candidate, merged)?;
        }
        check_tags(&repo, &incoming.tags, candidate)?;
        Ok(Prepared {
            previous: head,
            candidate,
            generation,
            origin: Some(origin),
            remote_head: incoming.head,
            tags: incoming.tags,
        })
    }

    fn record_remote_head(&self, head: Option<Oid>) -> Result<()> {
        let mut progress = self
            .sync_state
            .lock()
            .map_err(|_| failure("Sync state lock was poisoned"))?;
        progress.observed_destination = true;
        progress.remote_head = head;
        drop(progress);
        Ok(())
    }

    /// Returns the candidate and, for a new merge, the head and candidate file
    /// maps it already built, so preparation need not walk both trees again.
    fn merge_heads(
        &self,
        repo: &Repository,
        head: Oid,
        other: Oid,
    ) -> Result<(Oid, Option<(Files, Files)>)> {
        if head == other || repo.graph_descendant_of(head, other)? {
            return Ok((head, None));
        }
        if repo.graph_descendant_of(other, head)? {
            return Ok((other, None));
        }
        let bases = repo.merge_bases(head, other)?;
        if bases.len() != 1 {
            return Err(failure(
                "History needs repair: a unique common ancestor is required. Both histories are preserved.",
            ));
        }
        let base = repository::files(repo, bases[0])?;
        let current = repository::files(repo, head)?;
        let incoming = repository::files(repo, other)?;
        // A divergent branch need not contain our newer decision records yet.
        // Validate its changes since our accepted common ancestor, then validate
        // the merged candidate against our accepted state before publication.
        super::restore::validate_changes(repo, &incoming, &base)?;
        self.sync_phase("Merging")?;
        let files = candidate::merge(
            repo,
            (bases[0], &base),
            (head, &current),
            (other, &incoming),
        )?;
        let changes = tree_changes(&current, &files);
        let tree = repository::changed_tree(repo, Some(head), &changes)?;
        let incoming = repo.find_commit(other)?;
        let device = candidate::device_name(&incoming);
        let message = if changes.is_empty() {
            format!("Joined history from {device}")
        } else {
            format!(
                "Merged changes from {device}: {}",
                super::checkpoint::change_summary(&changes)
            )
        };
        let commit = self.commit_with_parents(repo, &[head, other], tree, &message)?;
        Ok((commit, Some((current, files))))
    }

    pub(crate) fn apply_sync(&self, prepared: &Prepared) -> Result<()> {
        self.sync_phase("Applying changes")?;
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        if state::origin(&repo)? != prepared.origin {
            return Err(failure("Sync destination changed during preparation"));
        }
        let local = tags(&repo)?;
        let mut additions = BTreeMap::new();
        for (name, oid) in &prepared.tags {
            match local.get(name) {
                Some(previous) if previous != oid => {
                    return Err(failure("A named version changed after sync preparation"));
                }
                Some(_) => {}
                None => {
                    additions.insert(name.clone(), *oid);
                }
            }
        }
        if prepared.candidate != prepared.previous || !additions.is_empty() {
            self.apply_sync_commit(
                &repo,
                prepared.previous,
                prepared.candidate,
                prepared.generation,
                &additions,
            )?;
        }
        crate::versioning::checkpoints::recover(&repo, self.canonical()?.as_ref())
    }

    pub(crate) fn push_sync(&self, prepared: &Prepared) -> Result<()> {
        self.sync_phase("Uploading attachments")?;
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        // Include saves made during preparation/application before attempting
        // the next push. A rejected merge remains a parent of this checkpoint.
        self.checkpoint_locked(&repo)?;
        let head = repository::head(&repo)?.ok_or_else(|| failure("Project has no checkpoint"))?;
        let origin = state::origin(&repo)?.ok_or_else(|| failure("No sync destination"))?;
        let remote = transport::Remote::parse(&origin)?;
        let auth = self.auth(&remote)?;
        let tags = tags(&repo)?;
        if Some(head) != prepared.remote_head || tags != prepared.tags {
            let mut heads = vec![head];
            let mut known: Vec<_> = prepared.remote_head.into_iter().collect();
            if tags != prepared.tags {
                for (name, oid) in &tags {
                    if prepared.tags.get(name) != Some(oid) {
                        heads.push(*oid);
                    }
                }
                known.extend(prepared.tags.values().copied());
            }
            let objects = self.inventory(&repo, &heads, Some(&known))?;
            transport::transfer_pointers(
                &repo,
                &remote,
                &auth,
                &objects,
                transport::Direction::Upload,
            )
            .map_err(|e| e.classified("attachments_unavailable"))?;
            self.sync_phase("Pushing")?;
            transport::push(&repo, &remote, &auth, head, &tags)?;
        }
        state::write_receipt(&repo, origin, head, &tags)?;
        self.record_remote_head(Some(head))?;
        release_pending(&repo, self.canonical()?.as_ref())
    }

    /// Called when a sync finishes, pushed or not. Failures leave the protection.
    pub(super) fn release_pending_sync(&self) -> Result<()> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let _process = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        release_pending(&repo, self.canonical()?.as_ref())
    }

    pub(crate) fn download_history_assets(&self) -> Result<model::SyncStatus> {
        let _sync = self
            .sync_serial
            .lock()
            .map_err(|_| failure("Sync lock was poisoned"))?;
        let result = self.download_history();
        let status = self.finish_sync(result.as_ref().err().cloned())?;
        result?;
        Ok(status)
    }

    fn download_history(&self) -> Result<()> {
        self.sync_phase("Downloading history attachments")?;
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = crate::versioning::checkpoints::open(&self.root)?;
        let origin =
            state::origin(&repo)?.ok_or_else(|| failure("Choose a sync destination first"))?;
        let remote = transport::Remote::parse(&origin)?;
        let auth = self.auth(&remote)?;
        let mut heads = vec![repository::head(&repo)?.ok_or_else(|| failure("No history"))?];
        heads.extend(history_refs(&repo)?);
        self.transfer_cached(
            &repo,
            &remote,
            &auth,
            &heads,
            transport::Direction::Download,
            true,
        )
    }
}

/// A merge reuses its file maps to validate the candidate against the accepted head.
fn validate_incoming(
    repo: &Repository,
    head: Oid,
    candidate: Oid,
    merged: Option<(Files, Files)>,
) -> Result<()> {
    let (current, files) = match merged {
        Some(maps) => maps,
        None => (
            repository::files(repo, head)?,
            repository::files(repo, candidate)?,
        ),
    };
    super::restore::validate_candidate(repo, &files, &current, false)
        .map_err(|e| e.classified("history_needs_repair"))
}

/// The pending ref keeps a prepared candidate reachable until it is published
/// or abandoned. A durable operation that still awaits recovery keeps it.
fn release_pending(repo: &Repository, store: &crate::store::Store) -> Result<()> {
    let recovering = store
        .read(|tx| Ok(!crate::store::git::pending_operations(tx)?.is_empty()))
        .map_err(|error| failure(error.to_string()))?;
    if !recovering && let Ok(mut reference) = repo.find_reference(PENDING) {
        reference.delete()?;
    }
    Ok(())
}

fn tree_changes(
    before: &BTreeMap<String, Oid>,
    after: &BTreeMap<String, Oid>,
) -> BTreeMap<String, Option<Oid>> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| (path.clone(), after.get(path).copied()))
        .collect()
}

fn check_remote_history(repo: &Repository, origin: &str, incoming: Option<Oid>) -> Result<()> {
    let receipt = state::read_receipt(repo)?;
    if receipt.origin != origin {
        return Ok(());
    }
    let Some(previous) = receipt.head else {
        return Ok(());
    };
    let previous = Oid::from_str(&previous)?;
    if let Some(head) = incoming
        && (head == previous || repo.graph_descendant_of(head, previous)?)
    {
        return Ok(());
    }
    retain_history(repo, previous)?;
    if let Some(incoming) = incoming {
        retain_history(repo, incoming)?;
    }
    Err(
        failure("The destination history was rewritten or removed. Both histories are preserved.")
            .classified("remote_changed"),
    )
}

fn tags(repo: &Repository) -> Result<BTreeMap<String, Oid>> {
    repo.references_glob("refs/tags/bluink/versions/*")?
        .map(|reference| {
            let reference = reference?;
            Ok((
                reference.name()?.into(),
                reference
                    .target()
                    .ok_or_else(|| failure("Named version must be a direct reference"))?,
            ))
        })
        .collect()
}

fn retain_history(repo: &Repository, id: Oid) -> Result<()> {
    let name = format!("refs/bluink/recovery/{id}");
    match repo.refname_to_id(&name) {
        Ok(current) if current == id => Ok(()),
        Ok(_) => Err(failure(
            "A history recovery reference changed outside Bluink",
        )),
        Err(error) if error.code() == git2::ErrorCode::NotFound => {
            repo.reference(&name, id, false, "Preserved sync history")?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn history_refs(repo: &Repository) -> Result<Vec<Oid>> {
    let mut ids = Vec::new();
    for reference in repo.references()? {
        let reference = reference?;
        let name = reference.name()?;
        if name.starts_with("refs/bluink/")
            || name.starts_with("refs/tags/bluink/versions/")
            || name.starts_with("refs/remotes/")
        {
            ids.push(reference.peel_to_commit()?.id());
        }
    }
    Ok(ids)
}

fn check_tags(repo: &Repository, incoming: &BTreeMap<String, Oid>, candidate: Oid) -> Result<()> {
    if incoming.is_empty() {
        return Ok(());
    }
    let config = validation::config_at(repo, candidate)?;
    for (name, oid) in incoming {
        match repo.refname_to_id(name) {
            Ok(previous) if previous != *oid => {
                return Err(failure(
                    "A named version differs on the destination; neither copy was replaced",
                ));
            }
            Ok(_) => {}
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                super::history::named_tag(
                    repo,
                    name,
                    &repo.find_tag(*oid)?,
                    config["id"].as_str(),
                )?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn install_tags(repo: &Repository, incoming: &BTreeMap<String, Oid>) -> Result<()> {
    for (name, oid) in incoming {
        match repo.refname_to_id(name) {
            Ok(existing) if existing != *oid => {
                return Err(failure(
                    "A named version differs on the destination; neither copy was replaced",
                ));
            }
            Ok(_) => {}
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                repo.reference(name, *oid, false, "Received named version")?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
