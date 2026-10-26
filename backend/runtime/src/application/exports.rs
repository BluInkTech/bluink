// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::publication::request::ExportRequest;

use super::{Application, error::Result};
use crate::export::{jobs::Context, model::*};

impl Application {
    pub(crate) fn create_export(&self, request: ExportRequest) -> Result<JobId> {
        let _operation = self.begin_operation()?;
        self.exports.create(self.export_context()?, request)
    }

    pub(super) fn export_context(&self) -> Result<Context> {
        Ok(Context {
            root: self.root.clone(),
            index: self.state()?.index.as_ref().map(|index| index.path.clone()),
            web_fonts: self.options.web_font_folder.clone(),
            pdf_fonts: self.options.pdf_font_folder.clone(),
            docx_fonts: self.options.docx_font_folder.clone(),
            typst: self.options.typst_binary.clone(),
        })
    }

    pub(crate) fn read_export(&self, request: ReadRequest) -> Result<Status> {
        let _operation = self.begin_operation()?;
        self.exports.read(request)
    }

    pub(crate) fn start_export(&self, request: JobRequest) -> Result<JobId> {
        let _operation = self.begin_operation()?;
        self.exports.start(request)
    }

    pub(crate) fn approve_export(&self, request: ApproveRequest) -> Result<Approved> {
        let _operation = self.begin_operation()?;
        self.exports.approve(request)
    }

    pub(crate) fn cancel_export(&self, request: JobRequest) -> Result<Cancelled> {
        let _operation = self.begin_operation()?;
        Ok(self.exports.cancel(request))
    }

    pub(crate) fn preview_export(&self, request: JobRequest) -> Result<PreviewUrl> {
        let _operation = self.begin_operation()?;
        self.exports.preview(request)
    }

    #[cfg(test)]
    pub(crate) fn export_artifact(&self, id: &str, path: &str) -> Result<Option<Artifact>> {
        let _operation = self.begin_operation()?;
        self.exports.artifact(id, path)
    }

    pub(crate) fn open_export_artifact(
        &self,
        id: &str,
        path: &str,
    ) -> Result<Option<crate::export::jobs::OpenedArtifact>> {
        let _operation = self.begin_operation()?;
        self.exports.open_artifact(id, path)
    }

    pub(crate) fn export_site_zip(
        &self,
        id: &str,
    ) -> Result<Option<crate::export::jobs::OpenedArtifact>> {
        let _operation = self.begin_operation()?;
        self.exports.site_zip(id)
    }
}
