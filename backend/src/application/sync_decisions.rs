// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::{
    Application,
    drafts::{
        DecisionFile, DecisionPreview, DecisionRequest, ResolveDecisionRequest,
        ResolveDecisionResult,
    },
    error::{Result, internal_error, invalid_request},
};
use crate::domain::project::types::OpaqueJson;
use crate::records::items::canonical_item;

fn decision_error(error: crate::versioning::Error) -> super::error::ApplicationError {
    if error.state == "stale_decision" {
        super::error::ApplicationError::StaleDecision(error.to_string())
    } else {
        invalid_request(error)
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "These handlers consume the shared RPC requests."
)]
impl Application {
    pub(super) fn preview_file_decision(
        &self,
        request: DecisionRequest,
    ) -> Result<DecisionPreview> {
        let _operation = self.begin_operation()?;
        let preview = self
            .history
            .preview_file(&request.id)
            .map_err(decision_error)?;
        Ok(DecisionPreview {
            allowed_selections: preview.allowed,
            decision: canonical_item(preview.decision, true)?,
            current_targets: Vec::new(),
            current_files: vec![DecisionFile {
                path: preview.path,
                content: OpaqueJson(serde_json::to_value(preview.current).map_err(internal_error)?),
            }],
            expected: preview.expected,
        })
    }
    pub(super) fn resolve_file_decision(
        &self,
        request: ResolveDecisionRequest,
    ) -> Result<ResolveDecisionResult> {
        let _sync = self
            .history
            .sync_serial
            .lock()
            .map_err(super::error::poison_error)?;
        let (prepared, decision, items) = {
            let _operation = self.begin_operation()?;
            self.history
                .prepare_file_choice(
                    &request.id,
                    &request.expected,
                    request.selection,
                    &request.device,
                )
                .map_err(decision_error)?
        };
        let result = self.accept_sync(&prepared);
        self.history
            .finish_sync(result.as_ref().err().map(|e| e.to_string().into()))
            .map_err(internal_error)?;
        result?;
        Ok(ResolveDecisionResult {
            deleted_ids: Vec::new(),
            decision: canonical_item(decision, true)?,
            items: items
                .into_iter()
                .map(|item| Ok(canonical_item(item, true)?))
                .collect::<Result<_>>()?,
        })
    }
}
