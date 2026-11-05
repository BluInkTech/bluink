// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Synchronous application waits, scoped to one blocking operation. No timing
//! state escapes onto the next request reusing that worker thread.
use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
pub(crate) struct Timings {
    pub(crate) admission: Duration,
    pub(crate) sqlite_queue: Duration,
    pub(crate) sqlite_begin: Duration,
    pub(crate) storage_lock: Duration,
    pub(crate) sqlite_write: Duration,
}

thread_local! {
    static CURRENT: Cell<Option<Timings>> = const { Cell::new(None) };
}

struct Scope(Option<Timings>);

impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.set(self.0);
    }
}

pub(crate) fn capture<T>(action: impl FnOnce() -> T) -> (T, Timings) {
    let _scope = Scope(CURRENT.replace(Some(Timings::default())));
    let result = action();
    (result, CURRENT.get().unwrap_or_default())
}

pub(crate) fn sqlite_write(queue: Duration, begin: Duration, hold: Duration) {
    if let Some(mut timings) = CURRENT.get() {
        timings.sqlite_queue += queue;
        timings.sqlite_begin += begin;
        timings.sqlite_write += hold;
        CURRENT.set(Some(timings));
    }
}

pub(crate) enum Kind {
    Admission,
    StorageLock,
}

pub(crate) struct Wait {
    started: Instant,
    kind: Kind,
}

impl Wait {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            started: Instant::now(),
            kind,
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        if let Some(mut timings) = CURRENT.get() {
            match self.kind {
                Kind::Admission => timings.admission += self.started.elapsed(),
                Kind::StorageLock => timings.storage_lock += self.started.elapsed(),
            }
            CURRENT.set(Some(timings));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_are_request_scoped_and_restored_after_nested_calls_and_panics() {
        let ((), measured) = capture(|| {
            let ((), inner) = capture(|| {
                sqlite_write(Duration::from_millis(2), Duration::ZERO, Duration::ZERO);
            });
            assert!(inner.sqlite_queue >= Duration::from_millis(2));
            let _wait = Wait::new(Kind::Admission);
            std::thread::sleep(Duration::from_millis(2));
        });
        assert!(measured.admission >= Duration::from_millis(2));
        assert_eq!(measured.sqlite_queue, Duration::ZERO);
        let ((), storage) = capture(|| {
            {
                let _wait = Wait::new(Kind::StorageLock);
                std::thread::sleep(Duration::from_millis(2));
            }
            sqlite_write(Duration::ZERO, Duration::ZERO, Duration::from_millis(2));
        });
        assert!(storage.storage_lock >= Duration::from_millis(2));
        assert!(storage.sqlite_write >= Duration::from_millis(2));
        assert_eq!(storage.admission, Duration::ZERO);
        assert!(CURRENT.get().is_none());
        let _ = std::panic::catch_unwind(|| capture(|| panic!("injected")));
        assert!(CURRENT.get().is_none());
        let ((), empty) = capture(|| ());
        assert_eq!(empty.admission, Duration::ZERO);
        assert_eq!(empty.storage_lock, Duration::ZERO);
        assert_eq!(empty.sqlite_write, Duration::ZERO);
    }
}
