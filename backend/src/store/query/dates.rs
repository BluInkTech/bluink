// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Compiler, compile::comparison};
use crate::domain::project::query::Comparison;
use crate::domain::project::types::MetadataFieldValueType as Type;
use crate::store::{Error, Result, fields};
use chrono::{Datelike, Offset, TimeZone};

impl Compiler {
    pub(super) fn date(
        &mut self,
        column: &str,
        kind: Type,
        operator: Comparison,
        input: &str,
    ) -> Result<String> {
        if input.contains('T') {
            let value = self.bind(fields::time_value(kind, input)?);
            return Ok(format!("{column}{}{value}", comparison(operator)));
        }
        let today = chrono::Utc::now()
            .with_timezone(&self.timezone)
            .date_naive();
        // A relative date is a run of whole days; it compares like a single
        // day, by its first day, except that a match covers all of them.
        let (start, end, day_precision) = if let Some((start, end)) =
            crate::structured_query::relative_date_range(input, today)
        {
            (start, end, true)
        } else {
            let start = match input.len() {
                4 => format!("{input}-01-01"),
                7 => format!("{input}-01"),
                10 => input.to_owned(),
                _ => return Err(Error::Invalid("Enter a valid date.".into())),
            };
            let start = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d")
                .map_err(|error| Error::Invalid(error.to_string()))?;
            let end = match input.len() {
                4 => chrono::NaiveDate::from_ymd_opt(start.year() + 1, 1, 1),
                7 => start.checked_add_months(chrono::Months::new(1)),
                _ => start.succ_opt(),
            }
            .ok_or_else(|| Error::Invalid("Date range is out of bounds.".into()))?;
            (start, end, input.len() == 10)
        };
        let number = |date: chrono::NaiveDate| -> Result<i64> {
            if kind == Type::Date {
                return fields::time_value(kind, &date.to_string());
            }
            Ok(day_start(self.timezone, date))
        };
        let (lower, upper) = (number(start)?, number(end)?);
        let lower = self.bind(lower);
        if operator == Comparison::Match {
            let upper = self.bind(upper);
            Ok(format!("({column}>={lower} AND {column}<{upper})"))
        } else {
            // The existing timestamp predicates compare a calendar prefix to
            // complete canonical timestamps. Every instant on its boundary
            // follows the shorter prefix, including midnight itself.
            let operator = if kind != Type::Date || !day_precision {
                match operator {
                    Comparison::Gt => Comparison::Gte,
                    Comparison::Lte => Comparison::Lt,
                    other => other,
                }
            } else {
                operator
            };
            Ok(format!("{column}{}{lower}", comparison(operator)))
        }
    }
}

/// The first instant of a calendar day in the project timezone. Where a DST
/// change skips midnight, the day starts when the clock jumps: midnight under
/// the offset in effect just before the gap.
pub(crate) fn day_start(timezone: chrono_tz::Tz, date: chrono::NaiveDate) -> i64 {
    let local = date.and_time(chrono::NaiveTime::MIN);
    if let Some(start) = timezone.from_local_datetime(&local).earliest() {
        return start.timestamp_millis();
    }
    let before = timezone
        .offset_from_utc_datetime(&(local - chrono::Duration::days(1)))
        .fix()
        .local_minus_utc();
    local.and_utc().timestamp_millis() - i64::from(before) * 1000
}

#[cfg(test)]
mod tests {
    use super::day_start;

    #[test]
    fn a_day_whose_midnight_is_skipped_starts_when_the_clock_jumps() {
        let day = |text| chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
        // Beirut skips 00:00–01:00 on 29 March 2026; 01:00 +03:00 is 22:00Z.
        assert_eq!(
            day_start(chrono_tz::Asia::Beirut, day("2026-03-29")),
            1_774_735_200_000
        );
        assert_eq!(
            day_start(chrono_tz::UTC, day("2026-10-31")),
            1_793_404_800_000
        );
    }
}
