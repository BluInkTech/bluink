// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::project::{config, types::ViewContent};
use crate::engine::publication::{
    plan::PublicationFormat,
    theme::{self, ThemeSnapshot},
};
use serde::Deserialize;
use serde_json::json;

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use crate::{query, service};

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdateDefaultViewRequest {
    content: ViewContent,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadThemeRequest {
    theme_id: String,
    format: String,
}

impl Application {
    pub(crate) fn read_default_view(&self) -> Result<ViewContent> {
        let _operation = self.begin_operation()?;
        // Read canonical configuration, not the initialization snapshot.
        let config = config::read_project_config(&self.root)
            .map_err(internal_error)?
            .ok_or_else(|| internal_error("Project configuration is missing."))?;
        serde_json::from_value(
            config
                .get("defaultView")
                .cloned()
                .unwrap_or_else(|| json!({"query":"sort:modified-desc", "mode":"dashboard"})),
        )
        .map_err(internal_error)
    }

    pub(crate) fn update_default_view(
        &self,
        request: UpdateDefaultViewRequest,
    ) -> Result<ViewContent> {
        let _operation = self.begin_operation()?;
        let content = serde_json::to_value(&request.content).map_err(internal_error)?;
        let source = config::default_view_query(&content)?;
        let fields = service::query_fields(&self.index_path()?)?;
        let analysis = query::analyze_bluink_query_with_schema(source, &fields);
        if !analysis.valid {
            return Err(invalid_request(analysis.diagnostics.first().map_or(
                "Enter a valid fixed query for All Notes.",
                |diagnostic| diagnostic.message.as_str(),
            )));
        }
        config::write_default_view(&self.root, content)?;
        Ok(request.content)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn read_theme(&self, request: ReadThemeRequest) -> Result<ThemeSnapshot> {
        let _operation = self.begin_operation()?;
        let format = PublicationFormat::parse(&request.format)
            .map_err(|_| invalid_request("Choose PDF, HTML, or site as the theme target."))?;
        theme::capture_theme(&self.root, &request.theme_id, format).map_err(internal_error)
    }
}
