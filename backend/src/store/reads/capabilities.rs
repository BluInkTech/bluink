// SPDX-License-Identifier: MPL-2.0
use crate::domain::project::{
    query::{Comparison, StatePredicate, TextMode},
    types::MetadataFieldValueType,
};
use crate::store::{Result, fields};
use crate::{catalogue, structured_query};
use rusqlite::Transaction;
use serde::Serialize;

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

pub(crate) fn read(tx: &Transaction<'_>) -> Result<QueryCapabilities> {
    let schema = fields::schema(tx)?;
    Ok(from_schema(&schema))
}

fn from_schema(fields: &structured_query::schema::QuerySchema) -> QueryCapabilities {
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
        ("open_tasks", MetadataFieldValueType::Number),
        ("task_progress", MetadataFieldValueType::Number),
    ] {
        metadata.retain(|field| field.key != key);
        metadata.push(catalogue::MetadataField {
            key: key.into(),
            revision: None,
            value_type,
            source: crate::domain::project::types::FieldSource::Builtin,
            system: None,
            options: None,
            option_appearance: None,
        });
    }
    let mut capabilities = Vec::new();
    for field in metadata {
        if !structured_query::is_builtin_field(&field.key) && !fields.contains(&field.key) {
            continue;
        }
        let options = match field.key.as_str() {
            "type" => structured_query::item_type_values()
                .map(str::to_owned)
                .collect(),
            "state" => vec!["active".into(), "archived".into()],
            _ => field.options.unwrap_or_default(),
        };
        capabilities.push(QueryFieldCapability {
            can_test_presence: !matches!(field.key.as_str(), "title" | "body"),
            can_sort: structured_query::is_sort_field(&field.key, fields),
            references_items: matches!(field.key.as_str(), "link" | "reference")
                || field.value_type == MetadataFieldValueType::Page,
            relative_values: if matches!(
                field.value_type,
                MetadataFieldValueType::Date
                    | MetadataFieldValueType::DateTime
                    | MetadataFieldValueType::Timestamp
            ) {
                structured_query::RELATIVE_DATES.map(str::to_owned).to_vec()
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
    QueryCapabilities {
        version: 1,
        fields: capabilities,
        text_modes: vec![TextMode::Terms, TextMode::Phrase],
        states: vec![StatePredicate::Archived, StatePredicate::Pinned],
    }
}
