// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Structured query execution, diagnostics and dynamic search capabilities.
use crate::indexing::coordinator::MainSnapshot;
use serde::{Deserialize, Serialize};
use tantivy::Index;

use super::{
    Application,
    error::{ApplicationError, Result},
};
use crate::engine::project::types::MetadataFieldValueType;
use crate::indexing::service;
use crate::query::document::{
    Comparison, ConditionDiagnostic, QueryDocument, StatePredicate, TextMode,
};
use crate::{catalogue, query};

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QueryFieldCapability {
    key: String,
    value_type: MetadataFieldValueType,
    operators: Vec<Comparison>,
    options: Vec<String>,
    can_test_presence: bool,
    can_sort: bool,
    references_items: bool,
    relative_values: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QueryCapabilities {
    version: u8,
    fields: Vec<QueryFieldCapability>,
    text_modes: Vec<TextMode>,
    states: Vec<StatePredicate>,
}

fn native_validation(
    index: &Index,
    document: &QueryDocument,
    fields: &query::schema::QuerySchema,
    scope: Option<&str>,
) -> Result<QueryValidation> {
    let diagnostics = match query::compile_query_document(index, document, fields, scope) {
        Ok(_) => Vec::new(),
        Err(ApplicationError::InvalidQuery(diagnostics)) => diagnostics,
        Err(error) => return Err(error),
    };
    Ok(QueryValidation {
        valid: diagnostics.is_empty(),
        diagnostics,
    })
}

impl Application {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "RPC dispatch owns decoded requests at the application boundary."
    )]
    pub(crate) fn validate_query(&self, request: DocumentQueryRequest) -> Result<QueryValidation> {
        let _operation = self.begin_operation()?;
        let index_path = self.index_path()?;
        let MainSnapshot {
            index,
            query_fields: fields,
            ..
        } = service::service_index(&index_path)?;
        native_validation(
            &index,
            &request.document,
            &fields,
            request.pin_scope_id.as_deref(),
        )
    }

    pub(crate) fn query_capabilities(&self) -> Result<QueryCapabilities> {
        let _operation = self.begin_operation()?;
        let path = self.index_path()?;
        let MainSnapshot {
            index,
            query_fields: fields,
            ..
        } = service::service_index(&path)?;
        let mut metadata = fields.metadata().to_vec();
        for (key, value_type) in [
            ("type", MetadataFieldValueType::Select),
            ("state", MetadataFieldValueType::Select),
            ("id", MetadataFieldValueType::Text),
            ("title", MetadataFieldValueType::Text),
            ("body", MetadataFieldValueType::Text),
            ("tag", MetadataFieldValueType::Text),
            ("label", MetadataFieldValueType::Text),
            ("link", MetadataFieldValueType::Text),
            ("reference", MetadataFieldValueType::Text),
            ("created", MetadataFieldValueType::Timestamp),
            ("modified", MetadataFieldValueType::Timestamp),
            ("completed", MetadataFieldValueType::Timestamp),
            ("published_on", MetadataFieldValueType::Date),
        ] {
            metadata.retain(|field| field.key != key);
            metadata.push(catalogue::MetadataField {
                key: key.into(),
                value_type,
                source: crate::engine::project::types::FieldSource::Builtin,
                system: None,
                options: None,
                option_appearance: None,
            });
        }
        let mut capabilities = Vec::new();
        for field in metadata {
            if !query::is_builtin_field(&field.key) && !fields.contains(&field.key) {
                continue;
            }
            let options = match field.key.as_str() {
                "type" => query::item_type_values().map(str::to_owned).collect(),
                "state" => vec!["active".into(), "archived".into()],
                _ => field.options.unwrap_or_default(),
            };
            capabilities.push(QueryFieldCapability {
                can_test_presence: query::supports_presence(&index, &field.key),
                can_sort: query::is_sort_field(&field.key, &fields),
                references_items: matches!(field.key.as_str(), "link" | "reference"),
                relative_values: if matches!(
                    field.key.as_str(),
                    "created" | "modified" | "completed"
                ) {
                    vec!["today".into(), "yesterday".into(), "last_7d".into()]
                } else {
                    Vec::new()
                },
                key: field.key,
                value_type: field.value_type,
                // The native compiler supports lexical as well as numeric/date
                // comparisons. Labels use the value type; preserve native
                // predicates previously available through text entry.
                operators: vec![
                    Comparison::Match,
                    Comparison::Gt,
                    Comparison::Gte,
                    Comparison::Lt,
                    Comparison::Lte,
                ],
                options,
            });
        }
        capabilities.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(QueryCapabilities {
            version: 1,
            fields: capabilities,
            text_modes: vec![TextMode::Terms, TextMode::Phrase],
            states: vec![StatePredicate::Archived, StatePredicate::Pinned],
        })
    }
}
