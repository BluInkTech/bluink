// SPDX-License-Identifier: MPL-2.0
use super::{analytics, collections::Scope};
use crate::store::{Error, Result};
use crate::{
    domain::project::types::CalendarGranularity,
    records::{CalendarBucket, CalendarResults},
};
use rusqlite::{Transaction, params_from_iter, types::Value};

pub(crate) fn buckets(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    field: &str,
    start: &str,
    end: &str,
    granularity: CalendarGranularity,
) -> Result<CalendarResults> {
    let valid = crate::domain::project::item::is_canonical_date;
    if !valid(start) || !valid(end) || start > end {
        return Err(Error::Invalid(
            "Calendar dates must be an inclusive YYYY-MM-DD range.".into(),
        ));
    }
    let length = match granularity.as_str() {
        "day" => 10,
        "month" => 7,
        _ => 4,
    };
    let (sql, values) = match field {
        "completed" => completed(tx, scope, start, end, length)?,
        "created" => return instants(tx, scope, "created_ms", start, end, length),
        "modified" => return instants(tx, scope, "modified_ms", start, end, length),
        _ => item_dates(tx, scope, field, start, end, length)?,
    };
    let buckets = tx
        .prepare(&sql)?
        .query_map(params_from_iter(values.iter()), |row| {
            Ok(CalendarBucket {
                date: row.get(0)?,
                count: analytics::unsigned(row, 1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(CalendarResults { buckets })
}

fn completed(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    start: &str,
    end: &str,
    length: usize,
) -> Result<(String, Vec<Value>)> {
    let (filter, mut values) = analytics::selected(tx, scope)?;
    values.push(Value::Text(start.to_owned()));
    let begin = values.len();
    values.push(Value::Text(end.to_owned()));
    Ok((
        format!(
            "SELECT substr(day,1,{length}) AS bucket,count(*) FROM activity_daily WHERE tasks_completed>0 AND day>=?{begin} AND day<=?{} {filter} GROUP BY bucket ORDER BY bucket",
            values.len()
        ),
        values,
    ))
}

/// System instants are UTC; bucket them by project-timezone day so a clicked
/// day selects the same items as the `created:`/`modified:` query it adds.
fn instants(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    column: &str,
    start: &str,
    end: &str,
    length: usize,
) -> Result<CalendarResults> {
    let timezone = scope.timezone;
    let day = |text: &str| {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map_err(|error| Error::Invalid(error.to_string()))
    };
    let after = day(end)?
        .succ_opt()
        .ok_or_else(|| Error::Invalid("Date range is out of bounds.".into()))?;
    let (filter, mut values) = selection(tx, scope)?;
    values.push(Value::Integer(crate::store::query::day_start(
        timezone,
        day(start)?,
    )));
    let lower = values.len();
    values.push(Value::Integer(crate::store::query::day_start(
        timezone, after,
    )));
    let sql = format!(
        "SELECT i.{column} FROM item i WHERE {filter} AND i.{column}>=?{lower} AND i.{column}<?{}",
        values.len()
    );
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    let mut statement = tx.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        let instant = chrono::DateTime::from_timestamp_millis(row.get(0)?)
            .ok_or_else(|| Error::Invalid("Stored instant is out of range.".into()))?;
        let mut bucket = instant.with_timezone(&timezone).date_naive().to_string();
        bucket.truncate(length);
        *counts.entry(bucket).or_default() += 1;
    }
    let buckets = counts
        .into_iter()
        .map(|(date, count)| CalendarBucket { date, count })
        .collect();
    Ok(CalendarResults { buckets })
}

fn selection(tx: &Transaction<'_>, scope: &Scope<'_>) -> Result<(String, Vec<Value>)> {
    Ok(scope.selected(tx)?.map_or_else(
        || ("i.type<>'conflict'".into(), Vec::new()),
        |(sql, params)| (format!("i.id IN ({sql})"), params),
    ))
}

fn item_dates(
    tx: &Transaction<'_>,
    scope: &Scope<'_>,
    field: &str,
    start: &str,
    end: &str,
    length: usize,
) -> Result<(String, Vec<Value>)> {
    let key = field
        .strip_prefix("meta.")
        .filter(|key| {
            !key.is_empty()
                && key
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        })
        .ok_or_else(|| {
            Error::Invalid(
                "Choose a created, modified, completed, or registered date field.".into(),
            )
        })?;
    let path = format!("$.meta.{key}");
    let (filter, mut values) = selection(tx, scope)?;
    values.push(Value::Text(path));
    let at = values.len();
    values.push(Value::Text(start.to_owned()));
    let begin = values.len();
    values.push(Value::Text(end.to_owned()));
    // Authored date components are intentional: a stored timestamp's explicit
    // offset must not move its calendar date when converted to UTC projections.
    let sql = format!(
        "WITH dates AS (SELECT substr(json_extract(i.header_json,?{at}),1,10) AS day FROM item i WHERE {filter} AND json_type(i.header_json,?{at})='text') SELECT substr(day,1,{length}) AS bucket,count(*) FROM dates WHERE day>=?{begin} AND day<=?{} AND date(day,'+0 days')=day GROUP BY bucket ORDER BY bucket",
        values.len()
    );
    Ok((sql, values))
}
