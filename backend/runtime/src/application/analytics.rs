// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;

use crate::engine::project::reports::{self, DashboardAnalytics, DashboardDayDetails, ReportError};
use serde::Deserialize;

use super::Application;
use super::error::{ApplicationError, Result};

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct AnalyticsRequest {
    start_date: Option<String>,
    end_date: Option<String>,
    query: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct DayDetailsRequest {
    date: String,
    query: Option<String>,
}

impl From<ReportError> for ApplicationError {
    fn from(error: ReportError) -> Self {
        match error {
            ReportError::InvalidInput(message) => Self::InvalidRequest(message),
            ReportError::Storage(message) => Self::Internal(message),
        }
    }
}

impl Application {
    fn report_membership(&self, query: Option<&str>) -> Result<Option<HashSet<String>>> {
        if query.is_none_or(str::is_empty) {
            return Ok(None);
        }
        crate::reports::selected_ids(&self.index_path()?, query)
    }

    pub(crate) fn dashboard_analytics(
        &self,
        request: AnalyticsRequest,
    ) -> Result<DashboardAnalytics> {
        let _operation = self.begin_operation()?;
        let selected = self.report_membership(request.query.as_deref())?;
        let end = request.end_date.unwrap_or(self.activity_date()?);
        reports::dashboard_analytics(
            &self.root,
            request.start_date.as_deref(),
            &end,
            selected.as_ref(),
        )
        .map_err(Into::into)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn dashboard_day_details(
        &self,
        request: DayDetailsRequest,
    ) -> Result<DashboardDayDetails> {
        let _operation = self.begin_operation()?;
        let selected = self.report_membership(request.query.as_deref())?;
        reports::dashboard_day_details(&self.root, &request.date, selected.as_ref())
            .map_err(Into::into)
    }
}
