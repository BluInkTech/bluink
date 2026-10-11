// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::error::{ApplicationError, Result};

pub(crate) fn timezone(value: &str) -> Result<Tz> {
    value.parse().map_err(|_| {
        ApplicationError::InvalidRequest("Choose a valid project timezone.".to_owned())
    })
}

pub(crate) fn activity_date(now: DateTime<Utc>, zone: Tz) -> String {
    now.with_timezone(&zone).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_dst_and_calendar_rollover() {
        let london = timezone("Europe/London").unwrap();
        let winter = "2026-01-01T23:30:00Z".parse().unwrap();
        let summer = "2026-07-01T23:30:00Z".parse().unwrap();
        assert_eq!(activity_date(winter, london), "2026-01-01");
        assert_eq!(activity_date(summer, london), "2026-07-02");
        assert_eq!(
            activity_date(summer, timezone("America/Los_Angeles").unwrap()),
            "2026-07-01"
        );
        assert!(timezone("invalid/zone").is_err());
    }
}
