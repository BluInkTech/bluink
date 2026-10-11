// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Process-wide rendering capacity, independent of project/application state.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::application::error::{Result, internal_error};

struct Entry {
    id: String,
    work: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct Queue {
    entries: VecDeque<Entry>,
    stopped: bool,
}

#[derive(Default)]
struct Lane {
    queue: Mutex<Queue>,
    changed: Condvar,
}

pub(super) struct Scheduler {
    pdf: Arc<Lane>,
    web: Arc<Lane>,
    workers: Vec<JoinHandle<()>>,
}

impl Scheduler {
    fn new() -> Result<Self> {
        let mut scheduler = Self {
            pdf: Arc::default(),
            web: Arc::default(),
            workers: Vec::new(),
        };
        for (name, lane) in [
            ("pdf", &scheduler.pdf),
            ("web-1", &scheduler.web),
            ("web-2", &scheduler.web),
        ] {
            let lane = Arc::clone(lane);
            scheduler.workers.push(thread::Builder::new().name(format!("bluink-export-{name}")).spawn(move || {
                loop {
                    let entry = {
                        let mut queue = lane.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        while queue.entries.is_empty() && !queue.stopped {
                            queue = lane.changed.wait(queue).unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        if queue.stopped { return; }
                        queue.entries.pop_front()
                    };
                    if let Some(entry) = entry {
                        // A renderer panic must not permanently remove a capacity slot.
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(entry.work)).is_err() {
                            tracing::error!(target: "bluink.export", "Export worker recovered from a panic");
                        }
                    }
                }
            })?);
        }
        Ok(scheduler)
    }

    pub(super) fn global() -> Result<&'static Self> {
        static SCHEDULER: OnceLock<std::result::Result<Scheduler, String>> = OnceLock::new();
        SCHEDULER
            .get_or_init(|| Self::new().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(internal_error)
    }

    pub(super) fn enqueue(&self, pdf: bool, id: String, work: impl FnOnce() + Send + 'static) {
        let lane = if pdf { &self.pdf } else { &self.web };
        let queued = {
            let mut queue = lane
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.entries.push_back(Entry {
                id,
                work: Box::new(work),
            });
            queue.entries.len()
        };
        tracing::debug!(target: "bluink.export", event = "export_queued", queued,
            capacity = if pdf { 1_u64 } else { 2 }, pdf);
        lane.changed.notify_one();
    }

    pub(super) fn cancel(&self, id: &str) {
        for lane in [&self.pdf, &self.web] {
            let removed = {
                let mut queue = lane
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                queue
                    .entries
                    .iter()
                    .position(|entry| entry.id == id)
                    .and_then(|index| queue.entries.remove(index))
            };
            // Dropping work can lock its job: never do so while holding the queue.
            drop(removed);
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        for lane in [&self.pdf, &self.web] {
            let entries = {
                let mut queue = lane
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                queue.stopped = true;
                std::mem::take(&mut queue.entries)
            };
            lane.changed.notify_all();
            drop(entries);
        }
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                tracing::error!(target: "bluink.export", "Export worker did not stop cleanly");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn lanes_enforce_capacity_remove_queued_work_and_recover_after_panics() {
        let scheduler = Scheduler::new().unwrap();
        let (started, events) = mpsc::channel();
        let mut releases = Vec::new();
        for (id, pdf) in [("pdf-active", true), ("web-a", false), ("web-b", false)] {
            let (release, wait) = mpsc::channel();
            releases.push(release);
            let started = started.clone();
            scheduler.enqueue(pdf, id.to_owned(), move || {
                started.send(id).unwrap();
                wait.recv().unwrap();
            });
        }
        let mut observed = (0..3)
            .map(|_| events.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        observed.sort_unstable();
        assert_eq!(observed, ["pdf-active", "web-a", "web-b"]);
        for (id, pdf) in [
            ("pdf-cancel", true),
            ("pdf-next", true),
            ("web-next", false),
        ] {
            let started = started.clone();
            scheduler.enqueue(pdf, id.to_owned(), move || {
                started.send(id).unwrap();
            });
        }
        assert!(events.try_recv().is_err());
        scheduler.cancel("pdf-cancel");
        releases.remove(0).send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "pdf-next"
        );
        assert!(events.try_recv().is_err());
        releases.remove(0).send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "web-next"
        );
        releases.remove(0).send(()).unwrap();
        scheduler.enqueue(true, "panic".to_owned(), || {
            panic!("injected renderer panic")
        });
        scheduler.enqueue(true, "after-panic".to_owned(), move || {
            started.send("after-panic").unwrap();
        });
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "after-panic"
        );
    }
}
