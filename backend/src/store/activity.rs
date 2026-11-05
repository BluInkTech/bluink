// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Device-local writing totals commit with the revision that produced them.
use rusqlite::{Transaction, params};
use serde_json::Value;

use super::{Error, Result, item};

pub(crate) fn record_write(
    transaction: &Transaction<'_>,
    previous: Option<&Value>,
    next: &Value,
    day: &str,
) -> Result<()> {
    let events = crate::domain::project::activity::events_for_item_write(previous, next, day)
        .map_err(Error::Invalid)?;
    for event in events {
        let source = item::identity(
            transaction,
            event.source_item_id.as_deref().unwrap_or(&event.item_id),
            None,
        )?;
        let page = item::identity(transaction, &event.item_id, None)?;
        let instant = chrono::DateTime::parse_from_rfc3339(&event.occurred_at)
            .map_err(|error| Error::Invalid(error.to_string()))?
            .timestamp_millis();
        let inserted=transaction.execute(
            "INSERT INTO activity_event(source_id,revision_uuid,kind,page_id,occurred_ms,day,words_added,words_deleted)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source_id,revision_uuid,kind) DO NOTHING",
            params![source,event.revision,event.kind,page,instant,event.date,event.words_added,event.words_deleted],
        )?;
        if inserted == 0 {
            continue;
        }
        transaction.execute(
            "INSERT INTO activity_daily(day,page_id,words_added,words_deleted,editing_events,tasks_completed)
             VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(day,page_id) DO UPDATE SET
             words_added=words_added+excluded.words_added,words_deleted=words_deleted+excluded.words_deleted,
             editing_events=editing_events+excluded.editing_events,tasks_completed=tasks_completed+excluded.tasks_completed",
            params![event.date,page,event.words_added,event.words_deleted,i64::from(event.kind=="content"),i64::from(event.kind=="task_completed")],
        )?;
    }
    Ok(())
}

/// Detail expiry never removes accumulated daily totals. Only canonical revision
/// acceptance can create events, so expired events cannot be imported again.
pub(crate) fn prune(transaction: &Transaction<'_>, before_ms: i64) -> Result<usize> {
    Ok(transaction.execute(
        "DELETE FROM activity_event WHERE (source_id,revision_uuid,kind) IN
         (SELECT source_id,revision_uuid,kind FROM activity_event WHERE occurred_ms<?1 ORDER BY occurred_ms LIMIT 256)",
        [before_ms],
    )?)
}
