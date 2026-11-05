// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate};
use serde::Serialize;

use crate::domain::project::activity::{ActivityDay, ItemActivity};

/// Three years of days, enough for a year and the year before it.
const MAX_RANGE_DAYS: i64 = 3 * 366;

fn date(value: &str) -> Result<NaiveDate, String> {
    if value.len() != 10 {
        return Err("Date must use YYYY-MM-DD.".to_owned());
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| "Date must use YYYY-MM-DD.".to_owned())
}

#[derive(Debug, Default, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardSummary {
    active_days: u64,
    words_added: i64,
    words_deleted: i64,
    net_word_change: i64,
    editing_events: u64,
    tasks_completed: u64,
    current_streak: u64,
    best_streak: u64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardAnalytics {
    days: Vec<ActivityDay>,
    trend_start: String,
    summary: DashboardSummary,
}

pub(crate) fn dashboard_range(
    start_date: Option<&str>,
    end_date: &str,
) -> Result<(NaiveDate, NaiveDate), String> {
    let end = date(end_date)?;
    let start = if let Some(value) = start_date {
        date(value)?
    } else {
        end.checked_sub_signed(Duration::days(364))
            .ok_or_else(|| "Activity date range is too large.".to_owned())?
    };
    if start > end {
        return Err("startDate must be no later than endDate.".to_owned());
    }
    // The dashboard compares a year with the year before it; every day in the
    // range becomes a row in the response.
    if (end - start).num_days() >= MAX_RANGE_DAYS {
        return Err("Activity date range is too large.".to_owned());
    }
    Ok((start, end))
}

pub(crate) fn summarize_days(
    start: NaiveDate,
    end: NaiveDate,
    recorded: Vec<ActivityDay>,
) -> Result<DashboardAnalytics, String> {
    let mut recorded = recorded
        .into_iter()
        .map(|day| (day.date.clone(), day))
        .collect::<BTreeMap<_, _>>();
    let mut days = Vec::new();
    let mut summary = DashboardSummary::default();
    let mut streak = 0;
    let mut current = start;
    loop {
        let key = current.to_string();
        let day = recorded.remove(&key).unwrap_or_else(|| ActivityDay {
            date: key,
            activity: ItemActivity::default(),
            notes_touched: 0,
        });
        summary.words_added += day.activity.words_added;
        summary.words_deleted += day.activity.words_deleted;
        summary.net_word_change += day.activity.net_word_change;
        summary.editing_events += day.activity.editing_events;
        summary.tasks_completed += day.activity.tasks_completed;
        if day.activity.words_added > 0 {
            streak += 1;
            summary.active_days += 1;
        } else {
            streak = 0;
        }
        summary.best_streak = summary.best_streak.max(streak);
        days.push(day);
        if current == end {
            break;
        }
        current = current
            .succ_opt()
            .ok_or_else(|| "Activity date range is too large.".to_owned())?;
    }
    summary.current_streak = streak;
    Ok(DashboardAnalytics {
        days,
        trend_start: start.to_string(),
        summary,
    })
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardDayItem {
    pub(crate) id: String,
    pub(crate) title: String,
    #[serde(flatten)]
    pub(crate) activity: ItemActivity,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DashboardDayDetails {
    pub(crate) date: String,
    pub(crate) items: Vec<DashboardDayItem>,
}
