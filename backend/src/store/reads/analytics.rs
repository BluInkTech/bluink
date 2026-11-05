// SPDX-License-Identifier: MPL-2.0
use super::collections::Scope;
use crate::domain::project::{
    activity::{ActivityDay, ItemActivity},
    reports::{self, DashboardAnalytics, DashboardDayDetails, DashboardDayItem},
};
use crate::store::{Error, Result};
use rusqlite::{Transaction, params_from_iter, types::Value};

pub(crate) fn dashboard(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    start: Option<&str>,
    end: &str,
) -> Result<DashboardAnalytics> {
    let (start, end) = reports::dashboard_range(start, end).map_err(Error::Invalid)?;
    let (filter, mut values) = selected(tx, scope)?;
    values.push(Value::Text(start.to_string()));
    let begin = values.len();
    values.push(Value::Text(end.to_string()));
    let sql = format!(
        "SELECT day,sum(words_added),sum(words_deleted),sum(editing_events),sum(tasks_completed),sum(editing_events>0) FROM activity_daily WHERE day>=?{begin} AND day<=?{} {filter} GROUP BY day ORDER BY day",
        values.len()
    );
    let days = tx
        .prepare(&sql)?
        .query_map(params_from_iter(values.iter()), |row| {
            let activity = activity(row, 1)?;
            Ok(ActivityDay {
                date: row.get(0)?,
                activity,
                notes_touched: unsigned(row, 5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    reports::summarize_days(start, end, days).map_err(Error::Invalid)
}

pub(crate) fn day_details(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    date: &str,
) -> Result<DashboardDayDetails> {
    if !crate::domain::project::item::is_canonical_date(date) {
        return Err(Error::Invalid("Date must use YYYY-MM-DD.".into()));
    }
    let (filter, mut values) = selected(tx, scope)?;
    values.push(Value::Text(date.to_owned()));
    let sql = format!(
        "SELECT i.uuid,i.title,a.words_added,a.words_deleted,a.editing_events,a.tasks_completed FROM activity_daily a JOIN item i ON i.id=a.page_id WHERE a.day=?{} AND i.type='page' AND (a.editing_events>0 OR a.tasks_completed>0) {filter} ORDER BY i.uuid",
        values.len()
    );
    let items = tx
        .prepare(&sql)?
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(DashboardDayItem {
                id: row.get(0)?,
                title: row.get(1)?,
                activity: activity(row, 2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(DashboardDayDetails {
        date: date.to_owned(),
        items,
    })
}

pub(super) fn selected(tx: &Transaction<'_>, scope: &Scope<'_>) -> Result<(String, Vec<Value>)> {
    Ok(scope.selected(tx)?.map_or_else(
        || (String::new(), Vec::new()),
        |(sql, values)| (format!(" AND page_id IN ({sql})"), values),
    ))
}

fn activity(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<ItemActivity> {
    let words_added = row.get::<_, i64>(at)?;
    let words_deleted = row.get::<_, i64>(at + 1)?;
    Ok(ItemActivity {
        words_added,
        words_deleted,
        net_word_change: words_added - words_deleted,
        editing_events: unsigned(row, at + 2)?,
        tasks_completed: unsigned(row, at + 3)?,
    })
}

pub(super) fn unsigned(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(at)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            at,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
