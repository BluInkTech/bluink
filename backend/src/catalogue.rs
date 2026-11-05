// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::application::error::{Result, internal_error, internal_message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::project::types::{FacetKind, FieldSource, MetadataFieldValueType};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataField {
    pub(crate) key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) revision: Option<String>,
    pub(crate) value_type: MetadataFieldValueType,
    pub(crate) source: FieldSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) system: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) options: Option<Vec<String>>,
    /// How chosen options look; options without an entry are automatic.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) option_appearance:
        Option<std::collections::BTreeMap<String, crate::domain::project::types::ChoiceAppearance>>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct FacetEntry {
    pub(crate) key: String,
    pub(crate) kind: FacetKind,
    pub(crate) usage_count: usize,
    pub(crate) inferred: bool,
}

pub(crate) fn metadata_field(declaration: &Value) -> Result<MetadataField> {
    let key = declaration["key"]
        .as_str()
        .ok_or_else(|| internal_message("Property declaration has no key."))?;
    let system = declaration["system"] == true;
    let options = if declaration["options"]
        .as_array()
        .is_some_and(|options| !options.is_empty())
    {
        Some(Vec::<String>::deserialize(&declaration["options"]).map_err(internal_error)?)
    } else {
        None
    };
    Ok(MetadataField {
        key: key.to_owned(),
        revision: declaration["revision"].as_str().map(str::to_owned),
        value_type: MetadataFieldValueType::deserialize(&declaration["valueType"])
            .map_err(internal_error)?,
        source: if system {
            FieldSource::Builtin
        } else {
            FieldSource::Registered
        },
        system: system.then_some(true),
        options,
        option_appearance: declaration
            .get("optionAppearance")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(internal_error)?,
    })
}
