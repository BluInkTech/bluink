// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

#[cfg(test)]
use super::error::MutationError;

#[cfg(test)]
use super::item_writes::mutate_existing_item;
use crate::domain::project::snippets as canonical;

#[cfg(test)]
use canonical::{item_id, record};

#[cfg(test)]
use chrono::Utc;
use serde::Deserialize;

#[cfg(test)]
use serde_json::{Value, json};

use super::{
    Application,
    error::{Result, internal_error, internal_message, invalid_request},
};
use crate::records::{SnippetRecord, SnippetResults, SnippetUsage};
#[cfg(test)]
use std::path::Path;

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct CreateSnippetRequest {
    id: String,
    paragraph: crate::domain::project::types::ReferenceParagraph,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct UpdateSnippetRequest {
    id: String,
    revision: String,
    paragraph: crate::domain::project::types::ReferenceParagraph,
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
        let record = crate::store::commands::write_snippet(
            self.database()?.as_ref(),
            &request.id,
            serde_json::to_value(request.paragraph).map_err(internal_error)?,
            None,
        )?;
        serde_json::from_value(record).map_err(internal_error)
    }

    pub(crate) fn update_snippet(&self, request: UpdateSnippetRequest) -> Result<SnippetRecord> {
        let _operation = self.begin_operation()?;
        let record = crate::store::commands::write_snippet(
            self.database()?.as_ref(),
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
        crate::store::snapshots::read_snippet(&self.root, &request.id)
            .map_err(internal_message)?
            .map(|record| serde_json::from_value(record).map_err(internal_error))
            .transpose()
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn archive_snippet(&self, request: ArchiveSnippetRequest) -> Result<SnippetRecord> {
        let _operation = self.begin_operation()?;
        canonical::validate_snippet_id(&request.id).map_err(invalid_request)?;
        let id = canonical::item_id(&request.id).map_err(invalid_request)?;
        let item = crate::store::commands::metadata(
            self.database()?.as_ref(),
            &id,
            Some(&request.revision),
            serde_json::Map::from_iter([("state".to_owned(), serde_json::json!("archived"))]),
            true,
        )?;
        let record = canonical::record(&item).map_err(internal_message)?;
        serde_json::from_value(record).map_err(internal_error)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn query_snippets(&self, request: QuerySnippetsRequest) -> Result<SnippetResults> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                crate::store::reads::snippets::query(
                    tx,
                    request.query.as_deref().unwrap_or(""),
                    request.limit.unwrap_or(50),
                    request.offset.unwrap_or(0),
                )
            })
            .map_err(Into::into)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn snippet_usages(&self, request: ReadSnippetRequest) -> Result<Vec<SnippetUsage>> {
        let _operation = self.begin_operation()?;
        canonical::validate_snippet_id(&request.id).map_err(invalid_request)?;
        self.database()?
            .read(|tx| crate::store::reads::snippets::usages(tx, &request.id))
            .map_err(Into::into)
    }
}

#[cfg(test)]
pub(crate) fn archive_snippet(
    root: &Path,
    key: &str,
    expected_revision: &str,
) -> std::result::Result<Value, MutationError> {
    let id = item_id(key).map_err(MutationError::Validation)?;
    let result = mutate_existing_item(
        crate::store::Store::shared(root)?.as_ref(),
        &id,
        Some(expected_revision),
        &Utc::now().date_naive().to_string(),
        |_| Ok(json!({"id":id,"meta":{"state":"archived"}})),
    )?;
    record(&result["item"]).map_err(MutationError::internal)
}
#[cfg(test)]
pub(crate) fn write_snippet(
    root: &Path,
    key: &str,
    paragraph: Value,
    expected_revision: Option<&str>,
) -> std::result::Result<Value, MutationError> {
    crate::store::commands::write_snippet(
        crate::store::Store::shared(root)?.as_ref(),
        key,
        paragraph,
        expected_revision,
    )
    .map_err(Into::into)
}
