// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::Application;
use super::error::Result;
use crate::records::items::*;
use crate::store::reads;

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch takes ownership of each decoded request."
)]
impl Application {
    pub(crate) fn resolve_workspace_notes(
        &self,
        request: ResolveWorkspaceNotesRequest,
    ) -> Result<Vec<PublicItem>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::items::resolve_workspace_notes(
                    tx,
                    &request.workspace_id,
                    &request.ids,
                    request.limit.unwrap_or(50),
                    reads::timezone(tx)?,
                )
            })
            .map_err(Into::into)
    }
    pub(crate) fn read_item(&self, request: ReadItemRequest) -> Result<PublicItem> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                reads::items::read_item(tx, &request.id, request.options.content != Some(false))
            })
            .map_err(Into::into)
    }
    pub(crate) fn read_items(&self, request: ReadItemsRequest) -> Result<Vec<PublicItem>> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| reads::items::read_items(tx, &request, reads::timezone(tx)?))
            .map_err(Into::into)
    }
    pub(crate) fn query_items(&self, request: QueryItemsRequest) -> Result<QueryItemsResult> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| reads::items::query_items(tx, &request, reads::timezone(tx)?))
            .map_err(Into::into)
    }
}
