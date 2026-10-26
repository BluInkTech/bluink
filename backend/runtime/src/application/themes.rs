// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Application, error::Result};
use crate::engine::publication::{
    plan::PublicationFormat,
    request::{ExportRequest, SourceKind},
    theme_format::Definition,
    theme_preview,
    theme_store::{self, Record, Summary, Validation},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(
    test,
    derive(ts_rs::TS),
    ts(rename = "ReadPublicationThemePackageRequest")
)]
pub(crate) struct ReadRequest {
    theme_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "CreatePublicationThemeRequest"))]
pub(crate) struct CreateRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(
    test,
    derive(ts_rs::TS),
    ts(rename = "DuplicatePublicationThemeRequest")
)]
pub(crate) struct DuplicateRequest {
    theme_id: String,
    revision: String,
    name: String,
    #[cfg_attr(test, ts(optional))]
    definition: Option<Box<Definition>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "UpdatePublicationThemeRequest"))]
pub(crate) struct UpdateRequest {
    theme_id: String,
    revision: String,
    definition: Box<Definition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "DeletePublicationThemeRequest"))]
pub(crate) struct DeleteRequest {
    theme_id: String,
    revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PreviewPublicationThemeRequest"))]
pub(crate) struct PreviewRequest {
    theme_id: String,
    revision: String,
    definition: Box<Definition>,
    format: PublicationFormat,
    start: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PublicationThemeDeleted"))]
pub(crate) struct Deleted {
    deleted: bool,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch consumes each decoded request at the application boundary."
)]
impl Application {
    pub(crate) fn list_themes(&self, _: super::queries::EmptyRequest) -> Result<Vec<Summary>> {
        let _operation = self.begin_operation()?;
        let candidates = crate::document_graph::publication_candidates(&self.root)
            .map_err(super::error::internal_error)?;
        Ok(theme_store::list(&self.root, candidates)?)
    }
    pub(crate) fn read_theme_package(&self, request: ReadRequest) -> Result<Record> {
        let _operation = self.begin_operation()?;
        let candidates = crate::document_graph::publication_candidates(&self.root)
            .map_err(super::error::internal_error)?;
        Ok(theme_store::read(
            &self.root,
            &request.theme_id,
            candidates,
        )?)
    }
    pub(crate) fn create_theme(&self, request: CreateRequest) -> Result<Record> {
        let _operation = self.begin_operation()?;
        Ok(theme_store::create(&self.root, &request.name)?)
    }
    pub(crate) fn duplicate_theme(&self, request: DuplicateRequest) -> Result<Record> {
        let _operation = self.begin_operation()?;
        Ok(theme_store::duplicate(
            &self.root,
            &request.theme_id,
            &request.revision,
            &request.name,
            request.definition.as_deref(),
        )?)
    }
    pub(crate) fn update_theme(&self, request: UpdateRequest) -> Result<Record> {
        let _operation = self.begin_operation()?;
        Ok(theme_store::update(
            &self.root,
            &request.theme_id,
            &request.revision,
            &request.definition,
        )?)
    }
    pub(crate) fn validate_theme(&self, request: UpdateRequest) -> Result<Validation> {
        let _operation = self.begin_operation()?;
        Ok(theme_store::validate(
            &self.root,
            &request.theme_id,
            &request.revision,
            &request.definition,
        )?)
    }
    pub(crate) fn delete_theme(&self, request: DeleteRequest) -> Result<Deleted> {
        let _operation = self.begin_operation()?;
        theme_store::delete(&self.root, &request.theme_id, &request.revision)?;
        Ok(Deleted { deleted: true })
    }
    pub(crate) fn preview_theme(
        &self,
        request: PreviewRequest,
    ) -> Result<crate::export::model::JobId> {
        let _operation = self.begin_operation()?;
        let source = theme_preview::capture(
            &self.root,
            &request.theme_id,
            &request.revision,
            &request.definition,
            request.format,
        )?;
        let export = ExportRequest {
            source: SourceKind::Publication,
            id: theme_preview::PUBLICATION_ID.into(),
            format: request.format,
            start: request.start,
            page: None,
        };
        self.exports
            .create_theme_preview(self.export_context()?, export, source)
    }
}
