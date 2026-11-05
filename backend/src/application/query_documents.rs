// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Structured query execution, diagnostics and dynamic search capabilities.
use serde::{Deserialize, Serialize};

use super::{Application, error::Result};
use crate::store::{query, reads};
use crate::structured_query::document::{ConditionDiagnostic, QueryDocument};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct DocumentQueryRequest {
    document: QueryDocument,
    pin_scope_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QueryValidation {
    valid: bool,
    diagnostics: Vec<ConditionDiagnostic>,
}

pub(crate) use crate::store::reads::capabilities::QueryCapabilities;

#[expect(
    clippy::needless_pass_by_value,
    reason = "RPC dispatch takes ownership of each decoded request."
)]
impl Application {
    pub(crate) fn validate_query(&self, request: DocumentQueryRequest) -> Result<QueryValidation> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(|tx| {
                let diagnostics = match query::compile(
                    tx,
                    &request.document,
                    request.pin_scope_id.as_deref(),
                    reads::timezone(tx)?,
                ) {
                    Ok(_) => Vec::new(),
                    Err(crate::store::Error::InvalidQuery(diagnostics)) => diagnostics,
                    Err(error) => return Err(error),
                };
                Ok(QueryValidation {
                    valid: diagnostics.is_empty(),
                    diagnostics,
                })
            })
            .map_err(Into::into)
    }
    pub(crate) fn query_capabilities(&self) -> Result<QueryCapabilities> {
        let _operation = self.begin_operation()?;
        self.database()?
            .read(reads::capabilities::read)
            .map_err(Into::into)
    }
}
