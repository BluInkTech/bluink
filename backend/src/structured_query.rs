// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod document;
pub(crate) mod schema;

#[cfg(test)]
pub(crate) mod fixtures;

use std::collections::HashSet;

pub(crate) fn is_builtin_field(field: &str) -> bool {
    matches!(
        field,
        "type"
            | "state"
            | "tag"
            | "label"
            | "link"
            | "reference"
            | "title"
            | "body"
            | "id"
            | "created"
            | "modified"
            | "completed"
            | "published_on"
            | "open_tasks"
            | "task_progress"
    )
}

pub(crate) fn is_sort_field(field: &str, property_fields: &HashSet<String>) -> bool {
    matches!(
        field,
        "created"
            | "modified"
            | "title"
            | "type"
            | "state"
            | "id"
            | "published_on"
            | "open_tasks"
            | "task_progress"
    ) || property_fields.contains(field)
}

fn is_flat_facet_key(value: &str) -> bool {
    value.split('_').enumerate().all(|(index, segment)| {
        let mut characters = segment.chars();
        matches!(characters.next(), Some(character) if character.is_ascii_lowercase() || (index > 0 && character.is_ascii_digit()))
            && characters
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    })
}

fn is_tag_key(value: &str) -> bool {
    let segments = value.split('/').collect::<Vec<_>>();
    !segments.is_empty()
        && segments.iter().enumerate().all(|(index, segment)| {
            is_flat_facet_key(segment)
                || (index > 0
                    && segment.len() <= 4
                    && !segment.is_empty()
                    && segment.chars().all(|character| character.is_ascii_digit()))
        })
}
fn absolute_date_prefix(value: &str) -> Option<String> {
    let valid = matches!(value.len(), 4 | 7 | 10)
        && value
            .as_bytes()
            .iter()
            .enumerate()
            .all(|(index, byte)| match value.len() {
                4 => byte.is_ascii_digit(),
                7 => {
                    if index == 4 {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit()
                    }
                }
                _ => {
                    if matches!(index, 4 | 7) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit()
                    }
                }
            });
    if !valid {
        return None;
    }
    let full = match value.len() {
        4 => format!("{value}-01-01"),
        7 => format!("{value}-01"),
        _ => value.to_owned(),
    };
    chrono::NaiveDate::parse_from_str(&full, "%Y-%m-%d").ok()?;
    Some(value.to_owned())
}

/// The relative dates offered beside every date field. Any `last_<n>d` or
/// `next_<n>d` up to a year also compiles; these are the common ones.
pub(crate) const RELATIVE_DATES: [&str; 7] = [
    "today",
    "yesterday",
    "tomorrow",
    "last_7d",
    "last_30d",
    "next_7d",
    "next_30d",
];

/// The days a relative date covers, from its first day up to, not including,
/// the day after its last. `last_7d` ends today; `next_7d` starts today.
pub(crate) fn relative_date_range(
    value: &str,
    today: chrono::NaiveDate,
) -> Option<(chrono::NaiveDate, chrono::NaiveDate)> {
    let day = chrono::Duration::days;
    let (start, end) = match value {
        "today" => (today, today + day(1)),
        "yesterday" => (today - day(1), today),
        "tomorrow" => (today + day(1), today + day(2)),
        _ => {
            let (direction, days) = value.split_once('_')?;
            let days: i64 = days
                .strip_suffix('d')
                .filter(|digits| !digits.starts_with('0') && digits.len() <= 3)?
                .parse()
                .ok()
                .filter(|days| (1..=366).contains(days))?;
            match direction {
                "last" => (today - day(days - 1), today + day(1)),
                "next" => (today, today + day(days)),
                _ => return None,
            }
        }
    };
    Some((start, end))
}

pub(crate) fn is_relative_date(value: &str) -> bool {
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
        .and_then(|day| relative_date_range(value, day))
        .is_some()
}

pub(crate) fn item_type_values() -> impl Iterator<Item = &'static str> {
    crate::domain::project::types::ItemType::VALUES
        .iter()
        .map(|(_, value)| *value)
        .filter(|value| *value != "conflict")
}
