// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::project::reports::{DashboardAnalytics, DashboardDayDetails};
use crate::store::reads;
use serde::Deserialize;

use super::Application;
use super::error::Result;
use crate::structured_query::document::QueryDocument;

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct AnalyticsRequest {
    start_date: Option<String>,
    end_date: Option<String>,
    query: Option<QueryDocument>,
    pin_scope_id: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct DayDetailsRequest {
    date: String,
    query: Option<QueryDocument>,
    pin_scope_id: Option<String>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch takes ownership of each decoded request."
)]
impl Application {
    pub(crate) fn dashboard_analytics(
        &self,
        request: AnalyticsRequest,
    ) -> Result<DashboardAnalytics> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                let timezone = reads::timezone(tx)?;
                let end = request.end_date.clone().unwrap_or_else(|| {
                    chrono::Utc::now()
                        .with_timezone(&timezone)
                        .format("%Y-%m-%d")
                        .to_string()
                });
                reads::analytics::dashboard(
                    tx,
                    &reads::collections::Scope {
                        document: request.query.as_ref(),
                        pin_scope_id: request.pin_scope_id.as_deref(),
                        timezone,
                        limit: 1,
                        offset: 0,
                    },
                    request.start_date.as_deref(),
                    &end,
                )
            })
            .map_err(Into::into)
    }
    pub(crate) fn dashboard_day_details(
        &self,
        request: DayDetailsRequest,
    ) -> Result<DashboardDayDetails> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::analytics::day_details(
                    tx,
                    &reads::collections::Scope {
                        document: request.query.as_ref(),
                        pin_scope_id: request.pin_scope_id.as_deref(),
                        timezone: reads::timezone(tx)?,
                        limit: 1,
                        offset: 0,
                    },
                    &request.date,
                )
            })
            .map_err(Into::into)
    }
}
