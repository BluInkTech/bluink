// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::project::snippets as canonical;
use serde::Deserialize;

use super::Application;
use super::error::{Result, internal_error, invalid_request};
use crate::snippets::{SnippetRecord, SnippetResults, SnippetUsage};

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreateSnippetRequest {
    id: String,
    paragraph: crate::engine::project::types::ReferenceParagraph,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdateSnippetRequest {
    id: String,
    revision: String,
    paragraph: crate::engine::project::types::ReferenceParagraph,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadSnippetRequest {
    id: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ArchiveSnippetRequest {
    id: String,
    revision: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct QuerySnippetsRequest {
    query: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
}

impl Application {
    pub(crate) fn create_snippet(&self, request: CreateSnippetRequest) -> Result<SnippetRecord> {
        let _operation = self.begin_operation()?;
        let record = canonical::write_snippet(
            &self.root,
            &request.id,
            serde_json::to_value(request.paragraph).map_err(internal_error)?,
            None,
        )?;
        serde_json::from_value(record).map_err(internal_error)
    }

    pub(crate) fn update_snippet(&self, request: UpdateSnippetRequest) -> Result<SnippetRecord> {
        let _operation = self.begin_operation()?;
        let record = canonical::write_snippet(
            &self.root,
            &request.id,
            serde_json::to_value(request.paragraph).map_err(internal_error)?,
            Some(&request.revision),
        )?;
        serde_json::from_value(record).map_err(internal_error)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn read_snippet(
        &self,
        request: ReadSnippetRequest,
    ) -> Result<Option<SnippetRecord>> {
        let _operation = self.begin_operation()?;
        canonical::validate_snippet_id(&request.id).map_err(invalid_request)?;
        canonical::read_snippet(&self.root, &request.id)
            .map_err(internal_error)?
            .map(|record| serde_json::from_value(record).map_err(internal_error))
            .transpose()
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn archive_snippet(&self, request: ArchiveSnippetRequest) -> Result<SnippetRecord> {
        let _operation = self.begin_operation()?;
        let record = canonical::archive_snippet_with_usage_check(
            &self.root,
            &request.id,
            &request.revision,
            || crate::document_graph::verified_snippet_usages(&self.root, &request.id),
        )?;
        serde_json::from_value(record).map_err(internal_error)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn query_snippets(&self, request: QuerySnippetsRequest) -> Result<SnippetResults> {
        let _operation = self.begin_operation()?;
        crate::snippets::query(
            &self.snippets,
            &self.root,
            request.query.as_deref().unwrap_or(""),
            request.limit.unwrap_or(50),
            request.offset.unwrap_or(0),
        )
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn snippet_usages(&self, request: ReadSnippetRequest) -> Result<Vec<SnippetUsage>> {
        let _operation = self.begin_operation()?;
        canonical::validate_snippet_id(&request.id).map_err(invalid_request)?;
        crate::document_graph::snippet_usages(&self.root, &request.id)
            .map_err(internal_error)?
            .into_iter()
            .map(|usage| serde_json::from_value(usage).map_err(internal_error))
            .collect()
    }
}
