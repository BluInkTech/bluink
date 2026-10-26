// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::catalogues::{Attributes, DeclarationStore};
use super::item::{is_canonical_date, is_canonical_instant};
use super::types::{ChoiceAppearance, MetadataFieldValueType};
use super::write::MutationError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Declaration {
    value_type: MetadataFieldValueType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    /// How chosen options look; options without an entry are automatic.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    option_appearance: BTreeMap<String, ChoiceAppearance>,
    #[serde(flatten)]
    attributes: Attributes,
}

const CATALOGUE: DeclarationStore<Declaration> = DeclarationStore {
    kind: "property",
    validate: validate_declaration,
};

pub(crate) fn validate_content(content: &Value) -> Result<(), String> {
    let key = content["key"].as_str().ok_or("Property has no key")?;
    let declaration = serde_json::from_value(content.clone()).map_err(|error| error.to_string())?;
    validate_declaration(key, &declaration)
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
            "Property {key} options must be sorted, unique, and only used with select types."
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

pub(crate) fn initialize(root: &Path) -> Result<(), MutationError> {
    CATALOGUE.initialize(root)
}

pub(crate) fn list_property_declarations(root: &Path) -> Result<Value, String> {
    CATALOGUE
        .read(root, |entries| {
            Ok(Value::Array(
                entries
                    .iter()
                    .map(|(key, value)| value.as_json(key))
                    .collect(),
            ))
        })
        .map_err(|error| error.to_string())
}

/// Validate declared metadata while the caller holds the project mutation gate.
pub(crate) fn with_validated_metadata<T>(
    root: &Path,
    meta: &Map<String, Value>,
    commit: impl FnOnce() -> Result<T, MutationError>,
) -> Result<T, MutationError> {
    let started = Instant::now();
    let mut checked = 0usize;
    for (key, value) in meta.iter().filter(|(_, value)| !value.is_null()) {
        let Some(record) =
            super::named_items::read(root, "property", key).map_err(MutationError::Internal)?
        else {
            continue;
        };
        if record["meta"]["state"] != "active" {
            continue;
        }
        let declaration: Declaration = serde_json::from_value(record["content"].clone())
            .map_err(MutationError::Serialization)?;
        validate_declaration(key, &declaration).map_err(MutationError::Internal)?;
        validate_value(key, &declaration, value).map_err(MutationError::Validation)?;
        checked += 1;
    }
    tracing::debug!(
        operation = "validate_item_properties",
        checked,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0
    );
    commit()
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The mutation consumes its input at the engine boundary."
)]
pub(crate) fn create_property(root: &Path, input: Value) -> Result<Value, MutationError> {
    let _gate = super::transaction::lock(root, super::io::LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    let key = input.get("key").and_then(Value::as_str).unwrap_or("");
    let value_type: MetadataFieldValueType = serde_json::from_value(input["valueType"].clone())
        .map_err(|_| MutationError::Validation("Unsupported property type.".to_owned()))?;
    let declaration = Declaration {
        value_type,
        options: normalize_options(value_type.as_str(), input.get("options"))
            .map_err(MutationError::Validation)?,
        option_appearance: BTreeMap::new(),
        attributes: Attributes::new(),
    };
    validate_declaration(key, &declaration).map_err(MutationError::Validation)?;
    CATALOGUE.update(root, key, |existing| {
        if existing.is_some() {
            return Err(MutationError::Validation(format!(
                "Property {key} already exists."
            )));
        }
        let result = declaration.as_json(key);
        Ok((Some(declaration), result))
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The mutation consumes its input at the engine boundary."
)]
/// Adds options to a select property and, when `appearance` is given,
/// replaces how its options look. Existing options are never renamed or
/// removed, so no note's value changes.
pub(crate) fn update_property_options(
    root: &Path,
    key: &str,
    requested_type: &str,
    additions: Value,
    appearance: Option<BTreeMap<String, ChoiceAppearance>>,
) -> Result<Value, MutationError> {
    let _gate = super::transaction::lock(root, super::io::LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    if !is_snake_case(key) {
        return Err(MutationError::Validation(
            "Property keys must be lowercase snake-case strings.".to_owned(),
        ));
    }
    if !matches!(requested_type, "select" | "multiSelect") {
        return Err(MutationError::Validation(
            "Property must support options.".to_owned(),
        ));
    }
    let additional =
        normalize_options(requested_type, Some(&additions)).map_err(MutationError::Validation)?;
    CATALOGUE.update(root, key, |existing| {
        let mut declaration = existing
            .filter(|entry| entry.value_type.as_str() == requested_type)
            .ok_or_else(|| MutationError::NotFound(format!("Property {key} does not exist.")))?;
        declaration.options.extend(additional);
        declaration.options.sort();
        declaration.options.dedup();
        if let Some(appearance) = appearance {
            declaration.option_appearance = appearance;
        }
        validate_declaration(key, &declaration).map_err(MutationError::Validation)?;
        let result = declaration.as_json(key);
        Ok((Some(declaration), result))
    })
}

pub(crate) fn remove_property(
    root: &Path,
    key: &str,
    used: impl FnOnce() -> Result<bool, MutationError>,
) -> Result<(), MutationError> {
    let _gate = super::transaction::lock(root, super::io::LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    if !is_snake_case(key) {
        return Err(MutationError::Validation(
            "Property keys must be lowercase snake-case strings.".to_owned(),
        ));
    }
    if super::config::is_required_builtin_property(key) {
        return Err(MutationError::Validation(format!(
            "Built-in property {key} is required and cannot be deleted."
        )));
    }
    super::usage::remove_if_unused(
        root,
        super::usage::Usage::Property(key),
        || Ok(()),
        || used().map(usize::from),
        |()| {
            CATALOGUE.update(root, key, |existing| {
                existing.ok_or_else(|| {
                    MutationError::NotFound(format!("Property {key} does not exist."))
                })?;
                Ok((None, ()))
            })
        },
    )
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
    let mut options: Vec<String> = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    options.sort();
    options.dedup();
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
    key: String,
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
    /// The caller holds the project mutation gate.
    pub(crate) fn prepare(
        root: &Path,
        properties: &[SetupProperty],
    ) -> Result<Self, MutationError> {
        let mut prepared = Self {
            declarations: BTreeMap::new(),
            items: Vec::new(),
        };
        for property in properties {
            let key = property.key.as_str();
            if super::config::is_required_builtin_property(key) {
                return Err(MutationError::Validation(format!(
                    "{key} is built in, so the setup must not declare it."
                )));
            }
            if prepared.declarations.contains_key(key) {
                return Err(MutationError::Validation(format!(
                    "Property {key} appears twice."
                )));
            }
            if let Some(existing) = Self::active(root, key)? {
                prepared.declarations.insert(key.to_owned(), existing);
                continue;
            }
            let declaration = Declaration {
                value_type: property.value_type,
                options: property.options.clone(),
                option_appearance: property.option_appearance.clone(),
                attributes: Attributes::new(),
            };
            validate_declaration(key, &declaration).map_err(MutationError::Validation)?;
            let id = super::named_items::id("property", key);
            let archived =
                super::named_items::read(root, "property", key).map_err(MutationError::Internal)?;
            let mut content =
                serde_json::to_value(&declaration).map_err(MutationError::Serialization)?;
            content["key"] = json!(key);
            let item = super::assemble::assemble_item(
                &json!({"id": id, "meta": {"type": "property", "state": "active", "title": key}, "content": content}),
                archived.as_ref(),
                "UTC",
            )
            .map_err(MutationError::Validation)?;
            super::item::validate_item(&item, &id).map_err(MutationError::Validation)?;
            super::named_items::validate_mutation(root, archived.as_ref(), &item)?;
            prepared.items.push(item);
            prepared.declarations.insert(key.to_owned(), declaration);
        }
        Ok(prepared)
    }

    fn active(root: &Path, key: &str) -> Result<Option<Declaration>, MutationError> {
        let Some(record) =
            super::named_items::read(root, "property", key).map_err(MutationError::Internal)?
        else {
            return Ok(None);
        };
        if record["meta"]["state"] != "active" {
            return Ok(None);
        }
        serde_json::from_value(record["content"].clone())
            .map(Some)
            .map_err(MutationError::Serialization)
    }

    /// Checks that `key` is declared, by the setup or the project, and that
    /// `value`, when given, suits its declaration.
    pub(crate) fn check(
        &mut self,
        root: &Path,
        owner: &str,
        key: &str,
        value: Option<&Value>,
    ) -> Result<(), MutationError> {
        if !self.declarations.contains_key(key) {
            let declaration = Self::active(root, key)?.ok_or_else(|| {
                MutationError::Validation(format!(
                    "{owner} uses {key}, which neither the setup nor the project declares."
                ))
            })?;
            self.declarations.insert(key.to_owned(), declaration);
        }
        if let (Some(value), Some(declaration)) = (value, self.declarations.get(key)) {
            validate_value(key, declaration, value)
                .map_err(|error| MutationError::Validation(format!("{owner}: {error}")))?;
        }
        Ok(())
    }
}

fn validate_value(key: &str, declaration: &Declaration, value: &Value) -> Result<(), String> {
    let valid = match declaration.value_type.as_str() {
        "number" | "rating" => value.is_number(),
        "checkbox" => value.is_boolean(),
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
    fn mutations_preserve_entry_attributes_and_validate_values() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        create_property(
            root.path(),
            json!({"key":"workflow","valueType":"select","options":["review","draft","draft"]}),
        )
        .unwrap();
        create_property(root.path(), json!({"key":"due_date","valueType":"date"})).unwrap();
        let path = super::super::named_items::path(root.path(), "property", "workflow").unwrap();
        let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        document["content"]["color"] = json!("blue");
        document["content"]["display"] = json!({"note": "kept by other editors"});
        std::fs::write(&path, document.to_string()).unwrap();
        let updated = update_property_options(
            root.path(),
            "workflow",
            "select",
            json!(["ready", "draft"]),
            None,
        )
        .unwrap();
        assert_eq!(updated["options"], json!(["draft", "ready", "review"]));
        let stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["content"]["color"], "blue");
        assert_eq!(stored["content"]["display"], document["content"]["display"]);
        assert!(stored["content"].get("metadata").is_none());
        let mut meta = Map::from_iter([
            ("workflow".to_owned(), json!("ready")),
            ("due_date".to_owned(), json!("2026-10-22")),
        ]);
        with_validated_metadata(root.path(), &meta, || Ok(())).unwrap();
        meta.insert("workflow".into(), json!("invalid"));
        assert!(
            with_validated_metadata::<()>(root.path(), &meta, || panic!("Invalid value committed"))
                .is_err()
        );
        meta.insert("workflow".into(), json!("draft"));
        meta.insert("due_date".into(), json!("2026-02-30"));
        assert!(with_validated_metadata(root.path(), &meta, || Ok(())).is_err());
        assert!(
            create_property(root.path(), json!({"key":"workflow","valueType":"text"})).is_err()
        );
        assert!(create_property(root.path(), json!({"key":"summary","valueType":"text"})).is_err());
        assert!(update_property_options(root.path(), "workflow", "text", json!([]), None).is_err());
        assert!(remove_property(root.path(), "workflow", || Ok(true)).is_err());
        remove_property(root.path(), "workflow", || Ok(false)).unwrap();
        assert!(matches!(
            remove_property(root.path(), "workflow", || Ok(false)),
            Err(MutationError::NotFound(_))
        ));
        assert_eq!(
            list_property_declarations(root.path())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn options_keep_their_appearance_until_it_is_replaced() {
        use super::super::types::ChoiceColor;
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        create_property(
            root.path(),
            json!({"key":"workflow","valueType":"select","options":["draft","review"]}),
        )
        .unwrap();
        let colours = |pairs: &[(&str, ChoiceColor)]| {
            pairs
                .iter()
                .map(|(option, color)| ((*option).to_owned(), ChoiceAppearance { color: *color }))
                .collect::<BTreeMap<_, _>>()
        };
        // New options and their colours arrive together.
        let updated = update_property_options(
            root.path(),
            "workflow",
            "select",
            json!(["final"]),
            Some(colours(&[
                ("final", ChoiceColor::Green),
                ("review", ChoiceColor::Amber),
            ])),
        )
        .unwrap();
        assert_eq!(updated["options"], json!(["draft", "final", "review"]));
        assert_eq!(
            updated["optionAppearance"],
            json!({"final": {"color": "green"}, "review": {"color": "amber"}})
        );
        // Adding options alone leaves colours as they were.
        let kept =
            update_property_options(root.path(), "workflow", "select", json!(["ready"]), None)
                .unwrap();
        assert_eq!(kept["optionAppearance"], updated["optionAppearance"]);
        // A new map replaces the old; an empty one makes every option automatic.
        let replaced = update_property_options(
            root.path(),
            "workflow",
            "select",
            json!([]),
            Some(colours(&[("draft", ChoiceColor::Neutral)])),
        )
        .unwrap();
        assert_eq!(
            replaced["optionAppearance"],
            json!({"draft": {"color": "neutral"}})
        );
        let cleared = update_property_options(
            root.path(),
            "workflow",
            "select",
            json!([]),
            Some(BTreeMap::new()),
        )
        .unwrap();
        assert!(cleared.get("optionAppearance").is_none());
        // Only existing options can have a colour, and a refused update changes nothing.
        assert!(
            update_property_options(
                root.path(),
                "workflow",
                "select",
                json!([]),
                Some(colours(&[("missing", ChoiceColor::Blue)])),
            )
            .is_err()
        );
        let listed = list_property_declarations(root.path()).unwrap();
        let workflow = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == "workflow")
            .unwrap();
        assert!(workflow.get("optionAppearance").is_none());
    }

    #[test]
    fn required_properties_are_protected_but_default_kind_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        super::super::config::open_or_create_project(root.path(), "UTC").unwrap();
        assert!(remove_property(root.path(), "status", || Ok(false)).is_err());
        remove_property(root.path(), "kind", || Ok(false)).unwrap();
    }

    #[test]
    fn invalid_property_blocks_its_own_values_without_overwriting_the_source() {
        let root = tempfile::tempdir().unwrap();
        initialize(root.path()).unwrap();
        create_property(root.path(), json!({"key":"broken","valueType":"text"})).unwrap();
        let file = super::super::named_items::path(root.path(), "property", "broken").unwrap();
        std::fs::write(&file, "invalid JSON").unwrap();
        let meta = Map::from_iter([("broken".to_owned(), json!("value"))]);
        assert!(
            with_validated_metadata::<()>(root.path(), &meta, || panic!(
                "Invalid property committed"
            ))
            .is_err()
        );
        with_validated_metadata(root.path(), &Map::new(), || Ok(())).unwrap();
        assert!(create_property(root.path(), json!({"key":"broken","valueType":"text"})).is_err());
        create_property(root.path(), json!({"key":"other","valueType":"text"})).unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "invalid JSON");
    }
}
