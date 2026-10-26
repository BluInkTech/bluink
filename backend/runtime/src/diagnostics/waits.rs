// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Synchronous application waits, scoped to one blocking operation. No timing
//! state escapes onto the next request reusing that worker thread.
use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
pub(crate) struct Timings {
    pub(crate) admission: Duration,
    pub(crate) readiness: Duration,
    pub(crate) storage_lock: Duration,
    pub(crate) publication: Duration,
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

pub(crate) enum Kind {
    Admission,
    Readiness,
    StorageLock,
    Publication,
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
                Kind::Readiness => timings.readiness += self.started.elapsed(),
                Kind::StorageLock => timings.storage_lock += self.started.elapsed(),
                Kind::Publication => timings.publication += self.started.elapsed(),
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
                let _wait = Wait::new(Kind::Readiness);
                std::thread::sleep(Duration::from_millis(2));
            });
            assert!(inner.readiness >= Duration::from_millis(2));
            let _wait = Wait::new(Kind::Admission);
            std::thread::sleep(Duration::from_millis(2));
        });
        assert!(measured.admission >= Duration::from_millis(2));
        assert_eq!(measured.readiness, Duration::ZERO);
        let ((), storage) = capture(|| {
            {
                let _wait = Wait::new(Kind::StorageLock);
                std::thread::sleep(Duration::from_millis(2));
            }
            let _wait = Wait::new(Kind::Publication);
            std::thread::sleep(Duration::from_millis(2));
        });
        assert!(storage.storage_lock >= Duration::from_millis(2));
        assert!(storage.publication >= Duration::from_millis(2));
        assert_eq!(storage.admission, Duration::ZERO);
        assert!(CURRENT.get().is_none());
        let _ = std::panic::catch_unwind(|| capture(|| panic!("injected")));
        assert!(CURRENT.get().is_none());
        let ((), empty) = capture(|| ());
        assert_eq!(empty.admission, Duration::ZERO);
        assert_eq!(empty.storage_lock, Duration::ZERO);
        assert_eq!(empty.publication, Duration::ZERO);
    }
}
