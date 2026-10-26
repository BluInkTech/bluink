// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::sync::{Condvar, Mutex};

use super::state::{Service, ServiceHandle};

impl Service {
    fn shutdown(&mut self) -> Result<(), String> {
        let stopped = self.handle.coordinator.stop(&self.handle.sender);
        let joined = self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "Search worker panicked during shutdown".to_owned())
        });
        joined.and(stopped)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // Explicit shutdown reports errors. This covers an interrupted handoff
        // from initialization to its reservation without detaching a worker.
        if self.worker.is_some()
            && let Err(error) = self.shutdown()
        {
            tracing::error!(error = %error, "Search service cleanup failed");
        }
    }
}

enum Phase {
    Starting,
    Running(Service),
    Stopping,
    Stopped(Result<(), String>),
    Failed(String),
}

/// One index owns its reservation through initialization and worker shutdown.
/// Waits and joins never hold the process-wide registry lock.
pub(crate) struct ServiceSlot {
    pub(crate) project_root: String,
    phase: Mutex<Phase>,
    changed: Condvar,
}

impl ServiceSlot {
    pub(crate) const fn new(project_root: String) -> Self {
        Self {
            project_root,
            phase: Mutex::new(Phase::Starting),
            changed: Condvar::new(),
        }
    }

    pub(crate) fn finish_start(&self, result: Result<Service, String>) -> Result<(), String> {
        let mut phase = match self.phase.lock() {
            Ok(phase) => phase,
            Err(error) => {
                let message = "Search lifecycle lock was poisoned.".to_owned();
                let mut phase = error.into_inner();
                *phase = Phase::Stopping;
                drop(phase);
                // An unpublished service still owns a worker. Drop it outside
                // the phase lock so its shutdown cannot block other waiters.
                drop(result);
                let mut phase = self
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *phase = Phase::Failed(message.clone());
                drop(phase);
                self.changed.notify_all();
                return Err(message);
            }
        };
        *phase = match result {
            Ok(service) => Phase::Running(service),
            Err(error) => Phase::Failed(error),
        };
        drop(phase);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn handle(&self) -> Result<ServiceHandle, String> {
        let mut phase = self.phase.lock().map_err(|error| error.to_string())?;
        let result = loop {
            match &*phase {
                Phase::Starting => {
                    phase = self
                        .changed
                        .wait(phase)
                        .map_err(|error| error.to_string())?;
                }
                Phase::Running(service) => break Ok(service.handle.clone()),
                Phase::Failed(error) => break Err(error.clone()),
                Phase::Stopping | Phase::Stopped(_) => {
                    break Err("Search index service is shutting down".to_owned());
                }
            }
        };
        drop(phase);
        result
    }

    pub(crate) fn shutdown(&self) -> Result<(), String> {
        let (mut phase, cleanup) =
            crate::application::error::recover_for_cleanup(self.phase.lock(), "Search lifecycle");
        let mut cleanup = cleanup.map_err(|error| error.to_string());
        let mut service = loop {
            match &*phase {
                Phase::Starting | Phase::Stopping => {
                    let (next, waited) = crate::application::error::recover_for_cleanup(
                        self.changed.wait(phase),
                        "Search lifecycle",
                    );
                    phase = next;
                    cleanup = cleanup.and_then(|()| waited.map_err(|error| error.to_string()));
                }
                Phase::Running(_) => {
                    if let Phase::Running(service) = std::mem::replace(&mut *phase, Phase::Stopping)
                    {
                        break service;
                    }
                }
                // Failed initialization owns no worker and needs no cleanup.
                Phase::Failed(_) => return cleanup,
                Phase::Stopped(result) => return cleanup.and_then(|()| result.clone()),
            }
        };
        drop(phase);

        // Always join, including when poisoned coordinator state rejects stop.
        let result = service.shutdown();
        let (mut phase, finished) =
            crate::application::error::recover_for_cleanup(self.phase.lock(), "Search lifecycle");
        let result = cleanup
            .and(result)
            .and_then(|()| finished.map_err(|error| error.to_string()));
        *phase = Phase::Stopped(result.clone());
        drop(phase);
        self.changed.notify_all();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::coordinator::IndexCoordinator;
    use super::super::state::Work;
    use super::*;
    use crate::query::schema::QuerySchema;
    use std::sync::Arc;

    fn service(
        worker: Option<std::thread::JoinHandle<()>>,
        sender: crossbeam_channel::Sender<Work>,
    ) -> Service {
        Service {
            worker,
            handle: ServiceHandle {
                project_root: "test".into(),
                sender,
                coordinator: Arc::new(IndexCoordinator::new(QuerySchema::default(), false)),
                schema_refresh: Arc::new(Mutex::new(())),
            },
        }
    }

    fn poison(slot: &ServiceSlot) {
        std::thread::scope(|scope| {
            assert!(
                scope
                    .spawn(|| {
                        let _phase = slot.phase.lock().unwrap();
                        panic!("Injected lifecycle failure");
                    })
                    .join()
                    .is_err()
            );
        });
    }

    #[test]
    fn poisoned_running_slot_joins_worker_and_reports_failure_to_all_stoppers() {
        let slot = ServiceSlot::new("test".into());
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (finished, completion) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || {
            assert!(matches!(receiver.recv().unwrap(), Work::Shutdown));
            finished.send(()).unwrap();
        });
        slot.finish_start(Ok(service(Some(worker), sender)))
            .unwrap();
        poison(&slot);
        assert!(slot.handle().is_err());
        std::thread::scope(|scope| {
            let first = scope.spawn(|| slot.shutdown());
            let second = scope.spawn(|| slot.shutdown());
            assert!(first.join().unwrap().unwrap_err().contains("poisoned"));
            assert!(second.join().unwrap().unwrap_err().contains("poisoned"));
        });
        completion.try_recv().unwrap();
        assert!(slot.shutdown().is_err());
    }

    #[test]
    fn poisoned_startup_reclaims_unpublished_worker_and_releases_shutdown_waiter() {
        let slot = ServiceSlot::new("test".into());
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (finished, completion) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || {
            assert!(matches!(receiver.recv().unwrap(), Work::Shutdown));
            finished.send(()).unwrap();
        });
        poison(&slot);
        std::thread::scope(|scope| {
            let stopping = scope.spawn(|| slot.shutdown());
            assert!(
                slot.finish_start(Ok(service(Some(worker), sender)))
                    .is_err()
            );
            assert!(stopping.join().unwrap().is_err());
        });
        completion.try_recv().unwrap();
        assert!(slot.handle().is_err());
    }

    #[test]
    fn concurrent_waiters_observe_the_same_initialized_service() {
        let slot = ServiceSlot::new("test".into());
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let service = service(None, sender);
        let expected = Arc::clone(&service.handle.coordinator);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| slot.handle().unwrap());
            let second = scope.spawn(|| slot.handle().unwrap());
            slot.finish_start(Ok(service)).unwrap();
            assert!(Arc::ptr_eq(&first.join().unwrap().coordinator, &expected));
            assert!(Arc::ptr_eq(&second.join().unwrap().coordinator, &expected));
        });
        slot.shutdown().unwrap();
    }

    #[test]
    fn shutdown_waits_for_startup_and_joins_the_worker() {
        let slot = ServiceSlot::new("test".into());
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (finished, completion) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || {
            assert!(matches!(receiver.recv().unwrap(), Work::Shutdown));
            finished.send(()).unwrap();
        });
        std::thread::scope(|scope| {
            let stopping = scope.spawn(|| slot.shutdown());
            slot.finish_start(Ok(service(Some(worker), sender)))
                .unwrap();
            stopping.join().unwrap().unwrap();
        });
        completion.try_recv().unwrap();
        assert!(slot.handle().is_err());
        slot.shutdown().unwrap();
    }

    #[test]
    fn failed_start_wakes_readers_and_shutdown_without_a_worker() {
        let slot = ServiceSlot::new("test".into());
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| slot.handle().err().unwrap());
            let stopping = scope.spawn(|| slot.shutdown());
            slot.finish_start(Err("Schema unavailable".into())).unwrap();
            assert_eq!(reader.join().unwrap(), "Schema unavailable");
            stopping.join().unwrap().unwrap();
        });
    }
}
