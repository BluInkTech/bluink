// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    Application,
    collections::EmptyRequest,
    error::{Result, internal_error, internal_message},
};
use std::{sync::Arc, thread::JoinHandle, time::Duration};

pub(super) struct Worker {
    stop: crossbeam_channel::Sender<()>,
    thread: JoinHandle<()>,
}

impl Application {
    pub(crate) fn start_automatic_sync(self: &Arc<Self>) -> Result<()> {
        let mut slot = self
            .automatic_sync
            .lock()
            .map_err(super::error::poison_error)?;
        if slot.is_some() {
            return Ok(());
        }
        let application = Arc::downgrade(self);
        let (stop, receiver) = crossbeam_channel::bounded(1);
        let thread = std::thread::Builder::new()
            .name("bluink-sync".into())
            .spawn(move || {
                loop {
                    if receiver.recv_timeout(Duration::from_secs(5))
                        != Err(crossbeam_channel::RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                    let Some(application) = application.upgrade() else {
                        break;
                    };
                    if application
                        .stopping
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        break;
                    }
                    match application.history.claim_automatic_sync() { Ok(true)=>{
                        // Operational failures are retained in status for the UI.
                        // Local writing and periodic checkpoints remain available.
                        let _ = application.sync_project(EmptyRequest {});
                    },Ok(false)=>{},Err(error)=>tracing::warn!(operation="sync.schedule",%error,"Automatic sync could not check the project") }
                }
            })
            .map_err(internal_error)?;
        *slot = Some(Worker { stop, thread });
        drop(slot);
        Ok(())
    }
    pub(super) fn stop_automatic_sync(&self) -> Result<()> {
        let mut slot = self
            .automatic_sync
            .lock()
            .map_err(super::error::poison_error)?;
        let worker = slot.take();
        drop(slot);
        if let Some(worker) = worker {
            let _ = worker.stop.send(());
            // The last weak upgrade can drop Application on this very worker.
            if worker.thread.thread().id() != std::thread::current().id() {
                worker
                    .thread
                    .join()
                    .map_err(|_| internal_message("Automatic sync worker panicked"))?;
            }
        }
        Ok(())
    }
}
