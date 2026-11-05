// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::coordinator::Schedule;
use std::time::{Duration, Instant};

impl Schedule {
    pub(super) fn checkpoint_due(&self, now: Instant) -> bool {
        if self.first.is_none() {
            return false;
        }
        let hidden = !self.clients.is_empty()
            && self.clients.values().all(|(seen, visible)| {
                now.duration_since(*seen) >= Duration::from_mins(2)
                    || (!visible && now.duration_since(*seen) >= Duration::from_secs(30))
            });
        hidden
            || self
                .first
                .is_some_and(|first| now.duration_since(first) >= Duration::from_mins(15))
            || self
                .last
                .is_some_and(|last| now.duration_since(last) >= Duration::from_mins(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(now: Instant) -> Schedule {
        Schedule {
            first: Some(now),
            last: Some(now),
            clients: [("client".into(), (now, true))].into(),
            ..Schedule::default()
        }
    }

    #[test]
    fn continuous_edits_do_not_extend_the_first_pending_deadline() {
        let start = Instant::now();
        let mut dirty = pending(start);
        let before = start + Duration::from_secs(899);
        dirty.last = Some(before);
        dirty.clients.insert("client".into(), (before, true));
        assert!(!dirty.checkpoint_due(before));
        assert!(dirty.checkpoint_due(start + Duration::from_mins(15)));
        dirty.first = None;
        assert!(!dirty.checkpoint_due(start + Duration::from_hours(1)));
    }

    #[test]
    fn quiet_hidden_and_expired_clients_trigger_early_checkpoints() {
        let start = Instant::now();
        let mut dirty = pending(start);
        dirty.clients.clear();
        assert!(!dirty.checkpoint_due(start + Duration::from_secs(299)));
        assert!(dirty.checkpoint_due(start + Duration::from_mins(5)));
        dirty.clients.insert("hidden".into(), (start, false));
        assert!(!dirty.checkpoint_due(start + Duration::from_secs(29)));
        assert!(dirty.checkpoint_due(start + Duration::from_secs(30)));
        dirty.clients.insert("visible".into(), (start, true));
        assert!(!dirty.checkpoint_due(start + Duration::from_secs(30)));
        assert!(dirty.checkpoint_due(start + Duration::from_mins(2)));
    }
}
