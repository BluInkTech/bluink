// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Durable transaction notifications, independent of browser or process lifetime.

use crate::store::changes::{ReadChangesRequest, ReadChangesResult};
impl super::Application {
    pub(crate) fn renew_editor_base(
        &self,
        request: RenewEditorBaseRequest,
    ) -> super::error::Result<EditorBaseLease> {
        let _operation = self.begin_operation()?;
        for id in [&request.id, &request.revision, &request.lease_id] {
            crate::domain::project::identity::ItemId::try_from(id.as_str())
                .map_err(super::error::invalid_request)?;
        }
        let retained = self.database()?.write(move |tx| {
            crate::store::maintenance::renew_editor_base(
                tx,
                &request.lease_id,
                &request.id,
                &request.revision,
                chrono::Utc::now().timestamp_millis(),
            )
        })?;
        Ok(EditorBaseLease { retained })
    }

    pub(crate) fn read_changes(
        &self,
        request: ReadChangesRequest,
    ) -> super::error::Result<ReadChangesResult> {
        let _operation = self.begin_operation()?;
        let cursor = request.cursor;
        self.database()?
            .read_changes(cursor.as_ref())
            .map_err(Into::into)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct RenewEditorBaseRequest {
    lease_id: String,
    id: String,
    revision: String,
}

#[derive(Debug, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct EditorBaseLease {
    retained: bool,
}
