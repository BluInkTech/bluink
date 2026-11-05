// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{History, Result, failure};
use std::{sync::Arc, thread::JoinHandle};

#[derive(Default)]
pub(super) struct WorkerState {
    pub(super) owners: usize,
    worker: Option<(crossbeam_channel::Sender<()>, JoinHandle<()>)>,
}

/// One initialized Application owns one lease, regardless of how often it
/// reopens its index. An uninitialized/failed Application owns none.
pub(crate) struct HistorySession {
    store: Option<Arc<History>>,
}

impl History {
    #[cfg(test)]
    pub(crate) fn active_sessions_for_test(&self) -> usize {
        self.worker.lock().unwrap().owners
    }

    pub(crate) fn acquire_session(self: &Arc<Self>) -> Result<HistorySession> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| failure("History worker lock was poisoned"))?;
        let owners = worker
            .owners
            .checked_add(1)
            .ok_or_else(|| failure("Too many history sessions"))?;
        if worker.owners == 0 {
            *self
                .sync_cancel
                .lock()
                .map_err(|_| failure("Synchronization cancellation lock was poisoned"))? =
                Arc::default();
        }
        if worker.worker.is_none() {
            worker.worker = Some(self.spawn_worker()?);
        }
        worker.owners = owners;
        drop(worker);
        Ok(HistorySession {
            store: Some(Arc::clone(self)),
        })
    }
}

impl HistorySession {
    pub(crate) fn close(&mut self) -> Result<()> {
        let Some(store) = self.store.take() else {
            return Ok(());
        };
        let (mut worker, poisoned) = match store.worker.lock() {
            Ok(worker) => (worker, false),
            Err(error) => (error.into_inner(), true),
        };
        worker.owners = worker
            .owners
            .checked_sub(1)
            .ok_or_else(|| failure("History session ownership is inconsistent"))?;
        let result = if worker.owners == 0 {
            let joined = if let Some((sender, handle)) = worker.worker.take() {
                let _ = sender.send(());
                handle
                    .join()
                    .map_err(|_| failure("History worker panicked"))
            } else {
                Ok(())
            };
            // Hold the lifecycle mutex through final publication so a new
            // owner cannot start a worker while its predecessor is stopping.
            let checkpoint = store.stop();
            joined.and(checkpoint)
        } else {
            Ok(())
        };
        drop(worker);
        result.and_then(|()| {
            if poisoned {
                Err(failure("History worker lock was poisoned"))
            } else {
                Ok(())
            }
        })
    }
}

impl Drop for HistorySession {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::error!(operation = "history.session.drop", error = %error, "History session cleanup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_worker_outlives_each_intermediate_owner_and_restarts_after_the_last() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::versioning::test_support::create(&root, "UTC").unwrap();
        let store = History::register(&root, temporary.path().join("user").to_str()).unwrap();
        store.initialize().unwrap();
        assert!(store.worker.lock().unwrap().worker.is_none());
        let mut first = store.acquire_session().unwrap();
        let mut second = store.acquire_session().unwrap();
        let remote = super::super::transport::Remote::Local(root.join("remote.git"));
        store.cancel_sync();
        let previous_auth = store.auth(&remote).unwrap();
        let thread = store
            .worker
            .lock()
            .unwrap()
            .worker
            .as_ref()
            .unwrap()
            .1
            .thread()
            .id();
        first.close().unwrap();
        first.close().unwrap();
        {
            let worker = store.worker.lock().unwrap();
            assert_eq!(worker.owners, 1);
            assert_eq!(worker.worker.as_ref().unwrap().1.thread().id(), thread);
            assert!(!worker.worker.as_ref().unwrap().1.is_finished());
            drop(worker);
        }
        second.close().unwrap();
        assert!(store.worker.lock().unwrap().worker.is_none());
        store.cancel_sync();
        assert!(previous_auth.check_cancelled().is_err());
        assert!(store.auth(&remote).is_err());
        let next = store.acquire_session().unwrap();
        assert!(store.auth(&remote).is_ok());
        assert!(previous_auth.check_cancelled().is_err());
        assert_ne!(
            store
                .worker
                .lock()
                .unwrap()
                .worker
                .as_ref()
                .unwrap()
                .1
                .thread()
                .id(),
            thread
        );
        drop(next);
        assert!(store.worker.lock().unwrap().worker.is_none());
        drop(store);
        temporary.close().unwrap();
    }
}
