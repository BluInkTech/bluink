// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023Ã¢â‚¬â€œ2026 BluInk
use super::{Result, failure};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};

#[derive(Default)]
pub(super) struct Schedule {
    pub(super) generation: Option<i64>,
    pub(super) first: Option<Instant>,
    pub(super) last: Option<Instant>,
    pub(super) error: Option<String>,
    pub(super) clients: BTreeMap<String, (Instant, bool)>,
}

pub(crate) struct History {
    pub(super) root: PathBuf,
    pub(super) database: Mutex<Option<Arc<crate::store::Store>>>,
    pub(super) schedule: Mutex<Schedule>,
    pub(super) user_directory: PathBuf,
    pub(super) serial: Mutex<()>,
    pub(super) worker: Mutex<super::session::WorkerState>,
    pub(crate) sync_serial: Mutex<()>,
    pub(super) sync_state: Mutex<super::sync::Progress>,
    pub(super) sync_cancel: Mutex<Arc<std::sync::atomic::AtomicBool>>,
}

static STORES: OnceLock<Mutex<Vec<Weak<History>>>> = OnceLock::new();

impl History {
    pub(crate) fn client_activity(
        &self,
        request: &super::model::HistoryClientActivityRequest,
    ) -> Result<super::model::HistoryClientActivity> {
        crate::domain::project::identity::ItemId::try_from(request.client_id.as_str())?;
        let mut dirty = self
            .schedule
            .lock()
            .map_err(|_| failure("History state lock was poisoned"))?;
        dirty
            .clients
            .retain(|_, (seen, _)| seen.elapsed() < std::time::Duration::from_mins(2));
        dirty
            .clients
            .insert(request.client_id.clone(), (Instant::now(), request.visible));
        drop(dirty);
        Ok(super::model::HistoryClientActivity { accepted: true })
    }
    pub(crate) fn register(root: &Path, user_directory: Option<&str>) -> Result<Arc<Self>> {
        let mut stores = STORES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| failure("History registry lock was poisoned"))?;
        stores.retain(|entry| entry.strong_count() > 0);
        if let Some(existing) = stores
            .iter()
            .filter_map(Weak::upgrade)
            .find(|store| store.root == root)
        {
            return Ok(existing);
        }
        let user_directory = user_directory
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(|path| PathBuf::from(path).join(".bluink"))
            })
            .ok_or_else(|| failure("Choose a user data directory for local history"))?;
        let store = Arc::new(Self {
            root: root.to_owned(),
            database: Mutex::default(),
            user_directory,
            serial: Mutex::new(()),
            worker: Mutex::default(),
            sync_serial: Mutex::new(()),
            sync_state: Mutex::default(),
            sync_cancel: Mutex::default(),
            schedule: Mutex::default(),
        });
        stores.push(Arc::downgrade(&store));
        drop(stores);
        Ok(store)
    }
}
