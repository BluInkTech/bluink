// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    Application,
    error::{Result, internal_message, invalid_request},
};
use crate::domain::publication::request::ExportRequest;
use crate::export_jobs::{jobs::Context, model::*};
use serde::Deserialize;

/// The page whose stored export issues to read.
#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct PageIssuesRequest {
    id: String,
}

impl Application {
    /// What stops a page exporting on its own, as recorded when it was last
    /// saved, without reading the page when the index can vouch for it.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn page_export_issues(
        &self,
        request: PageIssuesRequest,
    ) -> Result<Vec<crate::domain::publication::preflight::FactIssue>> {
        let _operation = self.begin_operation()?;
        if !crate::domain::project::identity::is_item_id(&request.id) {
            return Err(invalid_request("Choose a saved page."));
        }
        crate::store::reads::document_graph::page_export_issues(&self.root, &request.id)
            .map_err(internal_message)?
            .ok_or_else(|| invalid_request(format!("Page {} not found.", request.id)))
    }

    pub(crate) fn create_export(&self, request: ExportRequest) -> Result<JobId> {
        let _operation = self.begin_operation()?;
        self.exports
            .create(self.export_context(), request)
            .map_err(Into::into)
    }

    pub(super) fn export_context(&self) -> Context {
        Context {
            root: self.root.clone(),
            web_fonts: self.options.web_font_folder.clone(),
            pdf_fonts: self.options.pdf_font_folder.clone(),
            docx_fonts: self.options.docx_font_folder.clone(),
            typst: self.options.typst_binary.clone(),
        }
    }

    pub(crate) fn read_export(&self, request: ReadRequest) -> Result<Status> {
        let _operation = self.begin_operation()?;
        self.exports.read(request).map_err(Into::into)
    }

    pub(crate) fn start_export(&self, request: JobRequest) -> Result<JobId> {
        let _operation = self.begin_operation()?;
        self.exports.start(request).map_err(Into::into)
    }

    pub(crate) fn approve_export(&self, request: ApproveRequest) -> Result<Approved> {
        let _operation = self.begin_operation()?;
        self.exports.approve(request).map_err(Into::into)
    }

    pub(crate) fn cancel_export(&self, request: JobRequest) -> Result<Cancelled> {
        let _operation = self.begin_operation()?;
        Ok(self.exports.cancel(request))
    }

    pub(crate) fn preview_export(&self, request: JobRequest) -> Result<PreviewUrl> {
        let _operation = self.begin_operation()?;
        self.exports.preview(request).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn export_artifact(&self, id: &str, path: &str) -> Result<Option<Artifact>> {
        let _operation = self.begin_operation()?;
        self.exports.artifact(id, path).map_err(Into::into)
    }

    pub(crate) fn open_export_artifact(
        &self,
        id: &str,
        path: &str,
    ) -> Result<Option<crate::export_jobs::jobs::OpenedArtifact>> {
        let _operation = self.begin_operation()?;
        self.exports.open_artifact(id, path).map_err(Into::into)
    }

    pub(crate) fn export_site_zip(
        &self,
        id: &str,
    ) -> Result<Option<crate::export_jobs::jobs::OpenedArtifact>> {
        let _operation = self.begin_operation()?;
        self.exports.site_zip(id).map_err(Into::into)
    }
}
