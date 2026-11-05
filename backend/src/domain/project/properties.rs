// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    item::{is_canonical_date, is_canonical_instant},
    types::{ChoiceAppearance, MetadataFieldValueType},
};

#[cfg(test)]
use crate::application::error::MutationError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use serde_json::Map;
#[cfg(test)]
use std::time::Instant;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Declaration {
    value_type: MetadataFieldValueType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    /// How chosen options look; options without an entry are automatic.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    option_appearance: BTreeMap<String, ChoiceAppearance>,
    #[serde(flatten)]
    attributes: BTreeMap<String, Value>,
}

pub(crate) fn validate_content(content: &Value) -> Result<(), String> {
    ValidatedDeclaration::read(content).map(|_| ())
}

/// A parsed declaration reusable across a captured project or one live write.
pub(crate) struct ValidatedDeclaration {
    key: String,
    declaration: Declaration,
}

impl ValidatedDeclaration {
    pub(crate) fn read(content: &Value) -> Result<Self, String> {
        let key = content["key"].as_str().ok_or("Property has no key")?;
        let declaration =
            serde_json::from_value(content.clone()).map_err(|error| error.to_string())?;
        validate_declaration(key, &declaration)?;
        Ok(Self {
            key: key.to_owned(),
            declaration,
        })
    }

    pub(crate) fn validate(&self, value: &Value) -> Result<(), String> {
        validate_value(&self.key, &self.declaration, value)
    }
}

fn validate_declaration(key: &str, declaration: &Declaration) -> Result<(), String> {
    if !is_snake_case(key) || key == "summary" {
        return Err(
            "Property keys must be lowercase snake-case strings other than summary.".to_owned(),
        );
    }
    let normalized = normalize_options(
        declaration.value_type.as_str(),
        Some(&json!(declaration.options)),
    )?;
    if normalized != declaration.options {
        return Err(format!(
            "Property {key} options must be unique and only used with select types."
        ));
    }
    if let Some(option) = declaration
        .option_appearance
        .keys()
        .find(|option| !declaration.options.contains(option))
    {
        return Err(format!(
            "Property {key} has no option {option} to give an appearance."
        ));
    }
    Ok(())
}

#[cfg(test)]
impl Declaration {
    fn as_json(&self, key: &str) -> Value {
        let mut value = json!({"key": key, "valueType": self.value_type});
        if super::config::is_required_builtin_property(key) {
            value["system"] = json!(true);
        }
        if !self.options.is_empty() {
            value["options"] = json!(self.options);
        }
        if !self.option_appearance.is_empty() {
            value["optionAppearance"] = json!(self.option_appearance);
        }
        value
    }
}

#[cfg(test)]
pub(crate) fn initialize(root: &Path) -> Result<(), MutationError> {
    if !root.join(crate::store::DATABASE_NAME).is_file() {
        let config = json!({"format":"bluink-project","formatVersion":super::config::PROJECT_FORMAT_VERSION,
            "id":uuid::Uuid::now_v7().to_string(),"defaultWorkspaceId":uuid::Uuid::now_v7().to_string(),
            "analytics":{"timezone":"UTC","eventRetentionDays":14}});
        super::config::create_project_config(root, config).map_err(MutationError::internal)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn list_property_declarations(root: &Path) -> Result<Value, String> {
    let mut declarations = Vec::new();
    for record in super::named_items::list(root, "property")? {
        if record["meta"]["state"] != "active" {
            continue;
        }
        let parsed = ValidatedDeclaration::read(&record["content"])?;
        let mut value = parsed.declaration.as_json(&parsed.key);
        value["revision"] = record["revision"].clone();
        declarations.push(value);
    }
    Ok(Value::Array(declarations))
}

/// Validate declared metadata while the caller holds the project mutation gate.
#[cfg(test)]
pub(crate) fn with_validated_metadata<T>(
    root: &Path,
    meta: &Map<String, Value>,
    commit: impl FnOnce() -> Result<T, MutationError>,
) -> Result<T, MutationError> {
    let started = Instant::now();
    let mut checked = 0usize;
    for (key, value) in meta.iter().filter(|(_, value)| !value.is_null()) {
        let Some(record) = crate::store::snapshots::read_named(root, "property", key)
            .map_err(MutationError::internal)?
        else {
            continue;
        };
        if record["meta"]["state"] != "active" {
            continue;
        }
        let declaration =
            ValidatedDeclaration::read(&record["content"]).map_err(MutationError::internal)?;
        declaration
            .validate(value)
            .map_err(MutationError::Validation)?;
        checked += 1;
    }
    tracing::debug!(
        operation = "validate_item_properties",
        checked,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0
    );
    commit()
}

/// Prepare a new declaration from authoring input, without accessing storage.
pub(crate) fn new_declaration(key: &str, input: &Value) -> Result<Declaration, String> {
    let value_type: MetadataFieldValueType = serde_json::from_value(input["valueType"].clone())
        .map_err(|_| "Unsupported property type.".to_owned())?;
    let declaration = Declaration {
        value_type,
        options: normalize_options(value_type.as_str(), input.get("options"))?,
        option_appearance: BTreeMap::new(),
        attributes: BTreeMap::new(),
    };
    validate_declaration(key, &declaration)?;
    Ok(declaration)
}

/// Apply option edits to an observed declaration. Additional authored fields survive.
pub(crate) fn update_options(
    key: &str,
    mut declaration: Declaration,
    requested_type: &str,
    additions: Vec<String>,
    appearance: Option<BTreeMap<String, ChoiceAppearance>>,
    order: Option<Vec<String>>,
) -> Result<Declaration, String> {
    if declaration.value_type.as_str() != requested_type {
        return Err(format!("Property {key} does not exist."));
    }
    for option in additions {
        if !declaration.options.contains(&option) {
            declaration.options.push(option);
        }
    }
    if let Some(order) = order {
        let requested: std::collections::BTreeSet<_> = order.iter().collect();
        let existing: std::collections::BTreeSet<_> = declaration.options.iter().collect();
        if requested.len() != order.len() || requested != existing {
            return Err("Choice order must contain every option exactly once.".into());
        }
        declaration.options = order;
    }
    if let Some(appearance) = appearance {
        declaration.option_appearance = appearance;
    }
    validate_declaration(key, &declaration)?;
    Ok(declaration)
}

/// Check option-edit input before the shell loads the declaration.
pub(crate) fn option_additions(
    key: &str,
    requested_type: &str,
    additions: &Value,
) -> Result<Vec<String>, String> {
    if !is_snake_case(key) {
        return Err("Property keys must be lowercase snake-case strings.".to_owned());
    }
    if !matches!(requested_type, "select" | "multiSelect") {
        return Err("Property must support options.".to_owned());
    }
    normalize_options(requested_type, Some(additions))
}

fn normalize_options(value_type: &str, values: Option<&Value>) -> Result<Vec<String>, String> {
    if !matches!(value_type, "select" | "multiSelect") {
        return Ok(Vec::new());
    }
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    let Some(values) = values.as_array() else {
        return Err(
            "Property options must be an array of lowercase snake-case strings.".to_owned(),
        );
    };
    if !values
        .iter()
        .all(|value| value.as_str().is_some_and(is_snake_case))
    {
        return Err("Property options must be lowercase snake-case strings.".to_owned());
    }
    let options: Vec<String> = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if options
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != options.len()
    {
        return Err("Property options must be unique.".into());
    }
    Ok(options)
}

pub(crate) fn is_snake_case(value: &str) -> bool {
    let mut segments = value.split('_');
    segments.next().is_some_and(valid_segment)
        && segments.all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_segment(segment: &str) -> bool {
    segment
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// A property a workspace setup relies on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetupProperty {
    pub(crate) key: String,
    value_type: MetadataFieldValueType,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    option_appearance: BTreeMap<String, ChoiceAppearance>,
}

/// The declarations a setup's templates are checked against. A property the
/// project already declares is reused unchanged, never replaced; a missing
/// one becomes a new declaration item written with the rest of the setup.
pub(crate) struct SetupDeclarations {
    declarations: BTreeMap<String, Declaration>,
    /// New declaration items, for the caller's single write.
    pub(crate) items: Vec<Value>,
}

impl SetupDeclarations {
    /// The caller supplies declarations from one captured snapshot.
    pub(crate) fn prepare(
        records: &BTreeMap<String, Value>,
        properties: &[SetupProperty],
        values: &mut super::assemble::AuthoringValues<'_>,
    ) -> Result<Self, String> {
        let mut prepared = Self {
            declarations: BTreeMap::new(),
            items: Vec::new(),
        };
        for property in properties {
            let key = property.key.as_str();
            if super::config::is_required_builtin_property(key) {
                return Err(format!(
                    "{key} is built in, so the setup must not declare it."
                ));
            }
            if prepared.declarations.contains_key(key) {
                return Err(format!("Property {key} appears twice."));
            }
            if let Some(existing) = Self::active(records, key)? {
                prepared.declarations.insert(key.to_owned(), existing);
                continue;
            }
            let declaration = Declaration {
                value_type: property.value_type,
                options: property.options.clone(),
                option_appearance: property.option_appearance.clone(),
                attributes: BTreeMap::new(),
            };
            validate_declaration(key, &declaration)?;
            let id = super::named_items::id("property", key);
            let archived = records.get(&super::named_items::id("property", key));
            let mut content =
                serde_json::to_value(&declaration).map_err(|error| error.to_string())?;
            content["key"] = json!(key);
            let item = super::assemble::assemble_item(
                &json!({"id": id, "meta": {"type": "property", "state": "active", "title": key}, "content": content}),
                archived,
                "UTC",
                values,
            )?;
            super::item::validate_item(&item, &id)?;

            prepared.items.push(item);
            prepared.declarations.insert(key.to_owned(), declaration);
        }
        Ok(prepared)
    }

    fn active(records: &BTreeMap<String, Value>, key: &str) -> Result<Option<Declaration>, String> {
        let Some(record) = records.get(&super::named_items::id("property", key)) else {
            return Ok(None);
        };
        if record["meta"]["state"] != "active" {
            return Ok(None);
        }
        serde_json::from_value(record["content"].clone())
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Checks that `key` is declared, by the setup or the project, and that
    /// `value`, when given, suits its declaration.
    pub(crate) fn check(
        &mut self,
        records: &BTreeMap<String, Value>,
        owner: &str,
        key: &str,
        value: Option<&Value>,
    ) -> Result<(), String> {
        if !self.declarations.contains_key(key) {
            let declaration = Self::active(records, key)?.ok_or_else(|| {
                format!("{owner} uses {key}, which neither the setup nor the project declares.")
            })?;
            self.declarations.insert(key.to_owned(), declaration);
        }
        if let (Some(value), Some(declaration)) = (value, self.declarations.get(key)) {
            validate_value(key, declaration, value).map_err(|error| format!("{owner}: {error}"))?;
        }
        Ok(())
    }
}

fn validate_value(key: &str, declaration: &Declaration, value: &Value) -> Result<(), String> {
    let valid = match declaration.value_type.as_str() {
        "number" | "rating" => value.is_number(),
        "checkbox" => value.is_boolean(),
        "page" => value.as_str().is_some_and(super::identity::is_item_id),
        "date" => value.as_str().is_some_and(is_canonical_date),
        "dateTime" | "timestamp" => value.as_str().is_some_and(is_canonical_instant),
        "multiSelect" => value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string)),
        _ => value.is_string(),
    };
    if !valid {
        return Err(format!(
            "Property {} must use a {} value.",
            key,
            declaration.value_type.as_str()
        ));
    }
    if !declaration.options.is_empty() {
        let values: Vec<&str> = match declaration.value_type.as_str() {
            "select" => vec![value.as_str().unwrap_or("")],
            "multiSelect" => value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect(),
            _ => Vec::new(),
        };
        if values
            .iter()
            .any(|value| !declaration.options.iter().any(|option| option == value))
        {
            let suffix = if declaration.value_type.as_str() == "select" {
                "a declared option"
            } else {
                "declared options"
            };
            return Err(format!("Property {key} must use {suffix}."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_preparation_preserves_authored_fields_and_applies_complete_order() {
        let observed = json!({
            "key":"workflow", "valueType":"select", "options":["draft", "review"],
            "color":"blue", "display":{"compact":true},
            "optionAppearance":{"draft":{"color":"blue"}}
        });
        let declaration = serde_json::from_value(observed.clone()).unwrap();
        let additions = option_additions("workflow", "select", &json!(["ready", "draft"])).unwrap();
        let prepared = update_options(
            "workflow",
            declaration,
            "select",
            additions,
            None,
            Some(vec!["ready".into(), "review".into(), "draft".into()]),
        )
        .unwrap();
        let result = serde_json::to_value(prepared).unwrap();
        assert_eq!(result["options"], json!(["ready", "review", "draft"]));
        for field in ["key", "color", "display", "optionAppearance"] {
            assert_eq!(result[field], observed[field]);
        }
        // The input remains a reusable observation, independent of the prepared edit.
        assert_eq!(observed["options"], json!(["draft", "review"]));
    }

    #[test]
    fn option_preparation_rejects_incomplete_or_duplicate_orders() {
        for order in [vec!["draft".into()], vec!["draft".into(), "draft".into()]] {
            let declaration = new_declaration(
                "workflow",
                &json!({
                    "valueType":"select", "options":["draft", "review"]
                }),
            )
            .unwrap();
            assert_eq!(
                update_options(
                    "workflow",
                    declaration,
                    "select",
                    Vec::new(),
                    None,
                    Some(order)
                )
                .err()
                .unwrap(),
                "Choice order must contain every option exactly once."
            );
        }
    }
}
