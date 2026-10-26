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

impl Lane {
    fn next(&self) -> Result<Option<Entry>> {
        let mut queue = self
            .queue
            .lock()
            .map_err(crate::application::error::poison_error)?;
        while queue.entries.is_empty() && !queue.stopped {
            queue = self
                .changed
                .wait(queue)
                .map_err(crate::application::error::poison_error)?;
        }
        let entry = if queue.stopped {
            None
        } else {
            queue.entries.pop_front()
        };
        drop(queue);
        Ok(entry)
    }

    fn run(&self) {
        loop {
            match self.next() {
                Ok(Some(entry)) => {
                    // Rendering happens outside the queue lock. Its panic must
                    // not poison admission or permanently remove worker capacity.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(entry.work)).is_err() {
                        tracing::error!(target: "bluink.export", "Export worker recovered from a panic");
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    self.stop();
                    tracing::error!(target: "bluink.export", %error, "Export queue failed; lane stopped");
                    return;
                }
            }
        }
    }

    fn stop(&self) {
        // Recover only to close admission and reclaim resources, never to run work.
        let entries = {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.stopped = true;
            std::mem::take(&mut queue.entries)
        };
        self.changed.notify_all();
        drop(entries);
    }
}

pub(super) struct Scheduler {
    print: Arc<Lane>,
    web: Arc<Lane>,
    workers: Vec<JoinHandle<()>>,
}

impl Scheduler {
    fn new() -> Result<Self> {
        let mut scheduler = Self {
            print: Arc::default(),
            web: Arc::default(),
            workers: Vec::new(),
        };
        for (name, lane) in [
            ("print", &scheduler.print),
            ("web-1", &scheduler.web),
            ("web-2", &scheduler.web),
        ] {
            let lane = Arc::clone(lane);
            scheduler.workers.push(
                thread::Builder::new()
                    .name(format!("bluink-export-{name}"))
                    .spawn(move || lane.run())?,
            );
        }
        Ok(scheduler)
    }

    pub(super) fn global() -> Result<&'static Self> {
        static SCHEDULER: OnceLock<std::result::Result<Scheduler, String>> = OnceLock::new();
        SCHEDULER
            .get_or_init(|| Self::new().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(|error| internal_error(error.clone()))
    }

    /// Construct tracked work only after admission succeeds. A rejected closure
    /// must not drop a job lease while its caller still holds that job's lock.
    pub(super) fn enqueue<F>(
        &self,
        print: bool,
        id: String,
        prepare: impl FnOnce() -> F,
    ) -> Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        let lane = if print { &self.print } else { &self.web };
        let queued = {
            let mut queue = lane
                .queue
                .lock()
                .map_err(crate::application::error::poison_error)?;
            if queue.stopped {
                return Err(internal_error("Export scheduler has stopped."));
            }
            queue.entries.push_back(Entry {
                id,
                work: Box::new(prepare()),
            });
            queue.entries.len()
        };
        tracing::debug!(target: "bluink.export", event = "export_queued", queued,
            capacity = if print { 1_u64 } else { 2 }, print);
        lane.changed.notify_one();
        Ok(())
    }

    pub(super) fn cancel(&self, id: &str) {
        for lane in [&self.print, &self.web] {
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
        for lane in [&self.print, &self.web] {
            lane.stop();
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
    fn poisoned_admission_rejects_work_and_cleanup_releases_queued_resources() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let scheduler = Scheduler {
            print: Arc::default(),
            web: Arc::default(),
            workers: Vec::new(),
        };
        let released = Arc::new(AtomicBool::new(false));
        let lease = Lease(Arc::clone(&released));
        scheduler
            .enqueue(true, "waiting".into(), || move || drop(lease))
            .unwrap();
        let poisoned = std::panic::catch_unwind(|| {
            let _queue = scheduler.print.queue.lock().unwrap();
            panic!("injected queue corruption");
        });
        assert!(poisoned.is_err());
        let prepared = std::cell::Cell::new(false);
        assert!(
            scheduler
                .enqueue(true, "rejected".into(), || {
                    prepared.set(true);
                    || {}
                })
                .is_err()
        );
        assert!(
            !prepared.get(),
            "Rejected work must not create a tracked lease"
        );
        scheduler.print.run();
        assert!(released.load(Ordering::Acquire));
        let queue = scheduler.print.queue.lock().err().unwrap().into_inner();
        assert!(queue.stopped);
        assert!(queue.entries.is_empty());
        drop(queue);
        scheduler.web.stop();
        assert!(
            scheduler
                .enqueue(false, "stopped".into(), || || {})
                .is_err()
        );
    }

    #[test]
    fn lanes_enforce_capacity_remove_queued_work_and_recover_after_panics() {
        let scheduler = Scheduler::new().unwrap();
        let (started, events) = mpsc::channel();
        let mut releases = Vec::new();
        for (id, print) in [("print-active", true), ("web-a", false), ("web-b", false)] {
            let (release, wait) = mpsc::channel();
            releases.push(release);
            let started = started.clone();
            scheduler
                .enqueue(print, id.to_owned(), || {
                    move || {
                        started.send(id).unwrap();
                        wait.recv().unwrap();
                    }
                })
                .unwrap();
        }
        let mut observed = (0..3)
            .map(|_| events.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        observed.sort_unstable();
        assert_eq!(observed, ["print-active", "web-a", "web-b"]);
        for (id, print) in [
            ("print-cancel", true),
            ("print-next", true),
            ("web-next", false),
        ] {
            let started = started.clone();
            scheduler
                .enqueue(print, id.to_owned(), || {
                    move || {
                        started.send(id).unwrap();
                    }
                })
                .unwrap();
        }
        assert!(events.try_recv().is_err());
        scheduler.cancel("print-cancel");
        releases.remove(0).send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "print-next"
        );
        assert!(events.try_recv().is_err());
        releases.remove(0).send(()).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "web-next"
        );
        releases.remove(0).send(()).unwrap();
        scheduler
            .enqueue(true, "panic".to_owned(), || {
                || panic!("injected renderer panic")
            })
            .unwrap();
        scheduler
            .enqueue(true, "after-panic".to_owned(), || {
                move || {
                    started.send("after-panic").unwrap();
                }
            })
            .unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "after-panic"
        );
    }
}
