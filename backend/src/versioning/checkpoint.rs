// SPDX-License-Identifier: MPL-2.0
//! The history worker checkpoints canonical SQLite captures without scanning files.
use super::{History, Result, checkpoints, failure, model::HistoryStatus, repository};
use crate::store::io::{self, LockMode};
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};

impl History {
    pub(super) fn canonical(&self) -> Result<Arc<crate::store::Store>> {
        let mut cached = self
            .database
            .lock()
            .map_err(|_| failure("History database lock was poisoned"))?;
        if let Some(store) = cached.as_ref() {
            return Ok(Arc::clone(store));
        }
        let store =
            crate::store::Store::shared(&self.root).map_err(|error| failure(error.to_string()))?;
        *cached = Some(Arc::clone(&store));
        drop(cached);
        Ok(store)
    }
    pub(super) fn generation(&self) -> Result<i64> {
        self.canonical()?
            .read(|tx| {
                Ok(tx.query_row("SELECT generation FROM project_state", [], |row| row.get(0))?)
            })
            .map_err(|error| failure(error.to_string()))
    }
    #[cfg(test)]
    pub(crate) fn initialize(self: &Arc<Self>) -> Result<()> {
        self.initialize_with_progress(&|_, _, _| {})
    }
    pub(crate) fn initialize_with_progress(
        &self,
        report: &dyn Fn(&'static str, usize, usize),
    ) -> Result<()> {
        let _serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        checkpoints::initialize(&self.root)?;
        report("Recovering local Git history", 0, 0);
        let repo = checkpoints::open(&self.root)?;
        let _lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        checkpoints::recover(&repo, self.canonical()?.as_ref())?;
        report("Checkpointing saved changes", 0, 0);
        self.checkpoint_locked(&repo)
    }
    pub(crate) fn checkpoint(&self) -> Result<HistoryStatus> {
        let serial = self
            .serial
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let repo = checkpoints::open(&self.root)?;
        let lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        self.checkpoint_locked(&repo)?;
        drop(lease);
        drop(serial);
        self.status()
    }
    pub(super) fn checkpoint_locked(&self, repo: &Repository) -> Result<()> {
        let device = self.device()?;
        checkpoints::checkpoint_locked(repo, self.canonical()?.as_ref(), &device.id, &device.name)?;
        let mut state = self
            .schedule
            .lock()
            .map_err(|_| failure("History state lock was poisoned"))?;
        state.first = None;
        state.last = None;
        state.error = None;
        drop(state);
        Ok(())
    }
    pub(super) fn check_status_head(&self, repo: &Repository) -> Result<()> {
        if let Ok(_serial) = self.serial.try_lock() {
            let expected = self
                .canonical()?
                .read(crate::store::git::expected_head)
                .map_err(|error| failure(error.to_string()))?;
            if repository::head(repo)?.map(|oid| oid.to_string()) != expected {
                return Err(
                    failure("Git history differs from the acknowledged database head")
                        .classified("changed_outside_bluink"),
                );
            }
        }
        Ok(())
    }
    pub(crate) fn status(&self) -> Result<HistoryStatus> {
        let repo = checkpoints::open(&self.root)?;
        self.check_status_head(&repo)?;
        self.history_status_at(&repo)
    }
    pub(super) fn history_status_at(&self, repo: &Repository) -> Result<HistoryStatus> {
        let (head, dirty) = self
            .canonical()?
            .read(|tx| {
                Ok((
                    crate::store::git::expected_head(tx)?,
                    tx.query_row::<bool, _, _>(
                        "SELECT EXISTS(SELECT 1 FROM dirty_path)",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .map_err(|error| failure(error.to_string()))?;
        let last_checkpoint_at = head
            .as_deref()
            .map(|head| repository::timestamp(&repo.find_commit(Oid::from_str(head)?)?))
            .transpose()?;
        let error = self
            .schedule
            .lock()
            .map_err(|_| failure("History state lock was poisoned"))?
            .error
            .clone();
        Ok(HistoryStatus {
            head,
            dirty,
            last_checkpoint_at,
            error,
            local_only: super::sync::origin(repo)?.is_none(),
        })
    }
    pub(crate) fn stop(&self) -> Result<()> {
        let result = self.checkpoint().map(|_| ());
        let cached = self
            .database
            .lock()
            .map_err(|_| failure("History database lock was poisoned"))?
            .take();
        drop(cached);
        result
    }
    pub(super) fn due(&self) -> Result<bool> {
        let generation = self.generation()?;
        let pending = self
            .canonical()?
            .read(|tx| {
                Ok(tx.query_row::<bool, _, _>(
                    "SELECT EXISTS(SELECT 1 FROM dirty_path)",
                    [],
                    |row| row.get(0),
                )?)
            })
            .map_err(|error| failure(error.to_string()))?;
        let mut state = self
            .schedule
            .lock()
            .map_err(|_| failure("History state lock was poisoned"))?;
        let now = std::time::Instant::now();
        if pending {
            state.first.get_or_insert(now);
            if state.generation != Some(generation) {
                state.last = Some(now);
                state.generation = Some(generation);
            }
        } else {
            state.first = None;
            state.last = None;
        }
        let due = state.checkpoint_due(now);
        drop(state);
        Ok(due)
    }
    pub(super) fn spawn_worker(
        self: &Arc<Self>,
    ) -> Result<(crossbeam_channel::Sender<()>, std::thread::JoinHandle<()>)> {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let store = Arc::downgrade(self);
        let handle = std::thread::Builder::new()
            .name("bluink-history".into())
            .spawn(move || {
                let mut maintained = std::time::Instant::now();
                let mut housekeeping = crate::store::maintenance::Housekeeping::default();
                while matches!(receiver.recv_timeout(Duration::from_secs(5)), Err(crossbeam_channel::RecvTimeoutError::Timeout)) {
                    let Some(store) = store.upgrade() else { break };
                    if let Err(error) = store.canonical().and_then(|database| housekeeping.tick(&database).map_err(|error| failure(error.to_string()))) {
                        tracing::warn!(operation="storage.maintenance",error=%error,"Project housekeeping will retry");
                    }
                    if let Err(error) = store.checkpoint_if_due() {
                        if let Ok(mut state) = store.schedule.lock() {
                            state.error = Some(error.to_string());
                        }
                        tracing::warn!(operation = "history.checkpoint", error = %error, "Local checkpoint failed");
                    }
                    if maintained.elapsed() >= Duration::from_mins(5) {
                        maintained = std::time::Instant::now();
                        if let Err(error) = store.maintain(|| !receiver.is_empty()) {
                            tracing::warn!(operation = "history.maintenance", error = %error, "History packing retained original objects after failure");
                        }
                    }
                }
            })?;
        Ok((sender, handle))
    }
    fn checkpoint_if_due(&self) -> Result<()> {
        if self.due()? {
            self.checkpoint()?;
        }
        Ok(())
    }
    fn maintain(&self, cancelled: impl Fn() -> bool) -> Result<()> {
        let Ok(_serial) = self.serial.try_lock() else {
            return Ok(());
        };
        let repo = checkpoints::open(&self.root)?;
        let _lease = io::lock_file(&repo.path().join("bluink.lock"), LockMode::Exclusive)
            .map_err(|error| failure(format!("{error:#}")))?;
        let protected=self.canonical()?.read(|tx|Ok(tx.query_row::<bool,_,_>("SELECT EXISTS(SELECT 1 FROM maintenance_pin) OR EXISTS(SELECT 1 FROM git_operation)",[],|row|row.get(0))?)).map_err(|error|failure(error.to_string()))?;
        if !protected {
            super::packing::maintain(&repo, &cancelled)?;
            // Merged packs can be removed only once this handle stops mapping them.
            let path = repo.path().to_owned();
            drop(repo);
            super::packing::consolidate(&path, &cancelled)?;
        }
        Ok(())
    }
    pub(super) fn commit(
        &self,
        repo: &Repository,
        parent: Option<Oid>,
        tree: Oid,
        message: &str,
    ) -> Result<Oid> {
        self.commit_with_parents(repo, parent.as_slice(), tree, message)
    }

    pub(super) fn commit_with_parents(
        &self,
        repo: &Repository,
        parents: &[Oid],
        tree: Oid,
        message: &str,
    ) -> Result<Oid> {
        let device = self.device()?;
        let signature = git2::Signature::now("Bluink", "local@bluink.invalid")?;
        let message = format!(
            "{message}\n\nBluink-Device: {}\nBluink-Device-Name: {}\n",
            device.id, device.name
        );
        let parents = parents
            .iter()
            .map(|id| repo.find_commit(*id))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(repo.commit(
            None,
            &signature,
            &signature,
            &message,
            &repo.find_tree(tree)?,
            &parents.iter().collect::<Vec<_>>(),
        )?)
    }

    fn device(&self) -> Result<Device> {
        let _lock = io::lock_file(
            &self.user_directory.join("device.lock"),
            LockMode::Exclusive,
        )
        .map_err(|error| failure(format!("{error:#}")))?;
        let path = self.user_directory.join("device.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let device: Device = serde_json::from_slice(&bytes)?;
                crate::domain::project::identity::ItemId::try_from(device.id.as_str())?;
                if device.name.chars().any(char::is_control) {
                    return Err(failure("Invalid device name"));
                }
                Ok(device)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let device = Device {
                    id: uuid::Uuid::now_v7().to_string(),
                    name: hostname::get()?
                        .to_string_lossy()
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(120)
                        .collect(),
                };
                io::write_json_atomic(&path, &device)
                    .map_err(|error| failure(format!("{error:#}")))?;
                Ok(device)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
}
pub(super) fn change_summary(objects: &BTreeMap<String, Option<Oid>>) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for path in objects.keys() {
        let kind = if path.starts_with("items/") {
            "item"
        } else if path.starts_with("assets/") {
            "attachment"
        } else if path.starts_with("themes/") {
            "theme file"
        } else {
            "project setting"
        };
        *counts.entry(kind).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| format!("{count} {kind}{}", if count == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn validate_file(path: &str, bytes: &[u8]) -> Result<()> {
    if path == "bluink.json" {
        crate::domain::project::config::parse_project_config(bytes)?;
    } else if path.starts_with("items/") {
        let id = path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".json"))
            .ok_or_else(|| failure("Invalid item path"))?;
        crate::domain::project::item::validate_item(&serde_json::from_slice(bytes)?, id)?;
    }
    Ok(())
}
