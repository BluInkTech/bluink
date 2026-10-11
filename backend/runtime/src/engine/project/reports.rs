// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use chrono::{Duration, NaiveDate};
use serde::Serialize;
use serde_json::{Value, json};

use crate::engine::project::activity::{
    ActivityDay, ItemActivity, activity_days, read_daily_activity,
};
use crate::engine::project::item::read_project_item;

#[derive(Debug)]
pub(crate) enum ReportError {
    InvalidInput(String),
    Storage(String),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::Storage(message) => output.write_str(message),
        }
    }
}

impl std::error::Error for ReportError {}

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

/// The application supplies today's project-local date; core reporting has no
/// implicit machine clock or timezone dependency.
pub(crate) fn dashboard_analytics(
    root: &Path,
    start_date: Option<&str>,
    end_date: &str,
    selected: Option<&HashSet<String>>,
) -> Result<DashboardAnalytics, ReportError> {
    let end = date(end_date).map_err(ReportError::InvalidInput)?;
    let start = if let Some(value) = start_date {
        date(value).map_err(ReportError::InvalidInput)?
    } else {
        end.checked_sub_signed(Duration::days(364)).ok_or_else(|| {
            ReportError::InvalidInput("Activity date range is too large.".to_owned())
        })?
    };
    if start > end {
        return Err(ReportError::InvalidInput(
            "startDate must be no later than endDate.".to_owned(),
        ));
    }
    let recorded = activity_days(root, &start.to_string(), &end.to_string(), selected)
        .map_err(ReportError::Storage)?
        .into_iter()
        .map(|day| (day.date.clone(), day))
        .collect::<BTreeMap<_, _>>();
    let mut days = Vec::new();
    let mut summary = DashboardSummary::default();
    let mut streak = 0;
    let mut current = start;
    loop {
        let key = current.to_string();
        let day = recorded.get(&key).cloned().unwrap_or_else(|| ActivityDay {
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
        current = current.succ_opt().ok_or_else(|| {
            ReportError::InvalidInput("Activity date range is too large.".to_owned())
        })?;
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
    id: String,
    title: String,
    #[serde(flatten)]
    activity: ItemActivity,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct DashboardDayDetails {
    date: String,
    items: Vec<DashboardDayItem>,
}

pub(crate) fn dashboard_day_details(
    root: &Path,
    date_value: &str,
    selected: Option<&HashSet<String>>,
) -> Result<DashboardDayDetails, ReportError> {
    date(date_value).map_err(ReportError::InvalidInput)?;
    let day = read_daily_activity(root, date_value).map_err(ReportError::Storage)?;
    let mut items = Vec::new();
    if let Some(day) = day {
        for (id, activity) in day.items {
            if selected.is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            let item = read_project_item(root, &id)
                .map_err(ReportError::Storage)?
                .ok_or_else(|| ReportError::Storage(format!("Item {id} not found")))?;
            if item["meta"]["type"] != "page"
                || (activity.editing_events == 0 && activity.tasks_completed == 0)
            {
                continue;
            }
            items.push(DashboardDayItem {
                id,
                title: item["meta"]["title"]
                    .as_str()
                    .unwrap_or("Untitled")
                    .to_owned(),
                activity,
            });
        }
    }
    Ok(DashboardDayDetails {
        date: date_value.to_owned(),
        items,
    })
}

pub(crate) fn completed_calendar_buckets(
    root: &Path,
    start_date: &str,
    end_date: &str,
    granularity: &str,
    selected: Option<&HashSet<String>>,
) -> Result<Value, String> {
    let start = date(start_date)?;
    let end = date(end_date)?;
    if start > end || !matches!(granularity, "day" | "month" | "year") {
        return Err("Choose a valid calendar date range and granularity.".to_owned());
    }
    let mut buckets = BTreeMap::<String, usize>::new();
    let mut current = start;
    loop {
        let key = current.to_string();
        if let Some(day) = read_daily_activity(root, &key)? {
            let count = day
                .items
                .iter()
                .filter(|(id, activity)| {
                    selected.is_none_or(|ids| ids.contains(*id)) && activity.tasks_completed > 0
                })
                .count();
            if count > 0 {
                let bucket = match granularity {
                    "day" => &key[..10],
                    "month" => &key[..7],
                    _ => &key[..4],
                };
                *buckets.entry(bucket.to_owned()).or_default() += count;
            }
        }
        if current == end {
            break;
        }
        current = current
            .succ_opt()
            .ok_or("Calendar date range is too large.")?;
    }
    Ok(
        json!({"buckets": buckets.into_iter().map(|(date, count)| json!({"date":date,"count":count})).collect::<Vec<_>>() }),
    )
}
