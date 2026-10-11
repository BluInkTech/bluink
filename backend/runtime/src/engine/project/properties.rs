// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::path::Path;

use fs4::fs_std::FileExt;
use serde_json::{Map, Value, json};

use crate::engine::project::io::{create_json_atomic, write_json_atomic};
use crate::engine::project::item::{is_canonical_date, is_canonical_instant};
use crate::engine::project::write::{MutationError, lock_relationships};

#[derive(Debug)]
struct Declaration {
    key: String,
    value_type: &'static str,
    options: Vec<String>,
}

impl Declaration {
    fn as_json(&self) -> Value {
        let mut value = json!({"key": self.key, "valueType": self.value_type});
        if crate::engine::project::config::is_required_builtin_property(&self.key) {
            value["system"] = json!(true);
        }
        if !self.options.is_empty() {
            value["options"] = json!(self.options);
        }
        value
    }

    fn marker(&self) -> Value {
        let mut value = json!({"format": "bluink.property", "formatVersion": 1});
        if !self.options.is_empty() {
            value["options"] = json!(self.options);
        }
        value
    }

    fn path(&self, root: &Path) -> Result<std::path::PathBuf, String> {
        let token = type_token(self.value_type).ok_or("Unsupported property type")?;
        Ok(root
            .join("properties")
            .join(format!("{}.{token}.property", self.key)))
    }
}

/// Read version-one property markers. The filename is the declaration's
/// identity; unsupported filenames are not declarations.
pub(crate) fn list_property_declarations(root: &Path) -> Result<Value, String> {
    with_declarations_lock(root, || {
        Ok(Value::Array(
            read_declarations(root)?
                .iter()
                .map(Declaration::as_json)
                .collect(),
        ))
    })
}

/// Keep the declaration snapshot stable until the canonical item is durable.
pub(crate) fn with_validated_metadata<T>(
    root: &Path,
    meta: &Map<String, Value>,
    commit: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    with_declarations_lock(root, || {
        for declaration in read_declarations(root)? {
            let Some(value) = meta.get(&declaration.key).filter(|value| !value.is_null()) else {
                continue;
            };
            validate_value(&declaration, value)?;
        }
        commit()
    })
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn create_property(root: &Path, input: Value) -> Result<Value, MutationError> {
    let key = input.get("key").and_then(Value::as_str).unwrap_or("");
    if !is_snake_case(key) {
        return Err(MutationError::Expected(
            "Property keys must be lowercase snake-case strings.".to_owned(),
        ));
    }
    if key == "summary" {
        return Err(MutationError::Expected(
            "Summary is a built-in page property and cannot be redefined.".to_owned(),
        ));
    }
    let requested_type = input.get("valueType").and_then(Value::as_str).unwrap_or("");
    let Some(token) = type_token(requested_type) else {
        return Err(MutationError::Expected(format!(
            "Unsupported property type {requested_type}."
        )));
    };
    let declaration = Declaration {
        key: key.to_owned(),
        value_type: value_type(token)
            .ok_or_else(|| MutationError::Expected("Unsupported property type".to_owned()))?,
        options: normalize_options(requested_type, input.get("options"))
            .map_err(MutationError::Expected)?,
    };
    let _lock = declarations_lock(root).map_err(MutationError::Unexpected)?;
    if read_declarations(root)
        .map_err(MutationError::Unexpected)?
        .iter()
        .any(|existing| existing.key == key)
    {
        return Err(MutationError::Expected(format!(
            "Property {key} already exists."
        )));
    }
    create_json_atomic(
        &declaration.path(root).map_err(MutationError::Unexpected)?,
        &declaration.marker(),
    )
    .map_err(MutationError::Unexpected)?;
    Ok(declaration.as_json())
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn add_property_options(
    root: &Path,
    key: &str,
    requested_type: &str,
    additions: Value,
) -> Result<Value, MutationError> {
    if !is_snake_case(key) {
        return Err(MutationError::Expected(
            "Property keys must be lowercase snake-case strings.".to_owned(),
        ));
    }
    if type_token(requested_type).is_none() {
        return Err(MutationError::Expected(format!(
            "Unsupported property type {requested_type}."
        )));
    }
    let _lock = declarations_lock(root).map_err(MutationError::Unexpected)?;
    let mut declaration = read_declarations(root)
        .map_err(MutationError::Unexpected)?
        .into_iter()
        .find(|declaration| declaration.key == key && declaration.value_type == requested_type)
        .ok_or_else(|| MutationError::Expected(format!("Property {key} does not exist.")))?;
    if !matches!(requested_type, "select" | "multiSelect") {
        return Err(MutationError::Expected(format!(
            "Property {key} does not support options."
        )));
    }
    let mut values: Vec<Value> = declaration
        .options
        .iter()
        .map(|option| json!(option))
        .collect();
    let additional = additions.as_array().ok_or_else(|| {
        MutationError::Expected(
            "Property options must be an array of lowercase snake-case strings.".to_owned(),
        )
    })?;
    values.extend(additional.iter().cloned());
    declaration.options = normalize_options(requested_type, Some(&Value::Array(values)))
        .map_err(MutationError::Expected)?;
    let mut marker = declaration.marker();
    marker["options"] = json!(declaration.options);
    write_json_atomic(
        &declaration.path(root).map_err(MutationError::Unexpected)?,
        &marker,
    )
    .map_err(MutationError::Unexpected)?;
    let mut result = declaration.as_json();
    result["options"] = json!(declaration.options);
    Ok(result)
}

pub(crate) fn remove_property(root: &Path, key: &str) -> Result<(), MutationError> {
    if !is_snake_case(key) {
        return Err(MutationError::Expected(
            "Property keys must be lowercase snake-case strings.".to_owned(),
        ));
    }
    if crate::engine::project::config::is_required_builtin_property(key) {
        return Err(MutationError::Expected(format!(
            "Built-in property {key} is required and cannot be deleted."
        )));
    }
    let _relationships = lock_relationships(root).map_err(MutationError::Unexpected)?;
    if crate::engine::project::usage::property_used(root, key).map_err(MutationError::Unexpected)? {
        return Err(MutationError::Expected(format!(
            "Property {key} is used by one or more items."
        )));
    }
    let _lock = declarations_lock(root).map_err(MutationError::Unexpected)?;
    let declaration = read_declarations(root)
        .map_err(MutationError::Unexpected)?
        .into_iter()
        .find(|declaration| declaration.key == key)
        .ok_or_else(|| MutationError::Expected(format!("Property {key} does not exist.")))?;
    let path = declaration.path(root).map_err(MutationError::Unexpected)?;
    fs::remove_file(&path)
        .map_err(|error| MutationError::Unexpected(format!("{}: {error}", path.display())))
}

fn with_declarations_lock<T>(
    root: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _lock = declarations_lock(root)?;
    operation()
}

fn declarations_lock(root: &Path) -> Result<fs::File, String> {
    let directory = root.join(".cache");
    fs::create_dir_all(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join("property-declarations.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    lock.lock_exclusive()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(lock)
}

fn read_declarations(root: &Path) -> Result<Vec<Declaration>, String> {
    let directory = root.join("properties");
    let entries =
        fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut declarations = Vec::new();
    let mut keys = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((key, token)) = name
            .strip_suffix(".property")
            .and_then(|stem| stem.rsplit_once('.'))
        else {
            continue;
        };
        if !is_snake_case(key)
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            continue;
        }
        let Some(value_type) = value_type(token) else {
            continue;
        };
        let path = entry.path();
        let marker: Value = serde_json::from_slice(
            &fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        if marker.get("format").and_then(Value::as_str) != Some("bluink.property")
            || marker.get("formatVersion").and_then(Value::as_u64) != Some(1)
        {
            return Err(format!("{name} is not a supported Bluink property marker."));
        }
        let options = if matches!(value_type, "select" | "multiSelect") {
            match marker.get("options") {
                None => Vec::new(),
                Some(Value::Array(values))
                    if values
                        .iter()
                        .all(|value| value.as_str().is_some_and(is_snake_case)) =>
                {
                    let mut options: Vec<String> = values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    options.sort();
                    options.dedup();
                    options
                }
                _ => {
                    return Err(
                        "Property options must be an array of lowercase snake-case strings."
                            .to_owned(),
                    );
                }
            }
        } else {
            Vec::new()
        };
        if !keys.insert(key.to_owned()) {
            return Err(format!("Property {key} is declared more than once."));
        }
        declarations.push(Declaration {
            key: key.to_owned(),
            value_type,
            options,
        });
    }
    declarations.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(declarations)
}

fn value_type(token: &str) -> Option<&'static str> {
    Some(match token {
        "string" => "text",
        "number" => "number",
        "checkbox" => "checkbox",
        "date" => "date",
        "datetime" => "dateTime",
        "timestamp" => "timestamp",
        "select" => "select",
        "multi_select" => "multiSelect",
        "language" => "language",
        "color" => "color",
        "url" => "url",
        "email" => "email",
        "tel" => "tel",
        "rating" => "rating",
        "image" => "image",
        _ => return None,
    })
}

fn type_token(value_type: &str) -> Option<&'static str> {
    Some(match value_type {
        "text" => "string",
        "number" => "number",
        "checkbox" => "checkbox",
        "date" => "date",
        "dateTime" => "datetime",
        "timestamp" => "timestamp",
        "select" => "select",
        "multiSelect" => "multi_select",
        "language" => "language",
        "color" => "color",
        "url" => "url",
        "email" => "email",
        "tel" => "tel",
        "rating" => "rating",
        "image" => "image",
        _ => return None,
    })
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

fn validate_value(declaration: &Declaration, value: &Value) -> Result<(), String> {
    let valid = match declaration.value_type {
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
            declaration.key, declaration.value_type
        ));
    }
    if !declaration.options.is_empty() {
        let values: Vec<&str> = match declaration.value_type {
            "select" => vec![value.as_str().unwrap_or("")],
            "multiSelect" => value
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect(),
            _ => Vec::new(),
        };
        if values
            .iter()
            .any(|value| !declaration.options.iter().any(|option| option == value))
        {
            let suffix = if declaration.value_type == "select" {
                "a declared option"
            } else {
                "declared options"
            };
            return Err(format!("Property {} must use {suffix}.", declaration.key));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        add_property_options, create_property, list_property_declarations, remove_property,
        with_validated_metadata,
    };
    use serde_json::{Map, json};

    #[test]
    fn validates_markers_and_item_metadata() {
        let root = std::env::temp_dir().join(format!("bluink-property-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("properties")).unwrap();
        std::fs::write(root.join("properties/workflow.select.property"), br#"{"format":"bluink.property","formatVersion":1,"options":["review","draft","review"]}"#).unwrap();
        std::fs::write(
            root.join("properties/due_date.date.property"),
            br#"{"format":"bluink.property","formatVersion":1}"#,
        )
        .unwrap();
        let declarations = list_property_declarations(&root).unwrap();
        assert_eq!(
            declarations,
            json!([
                {"key":"due_date","valueType":"date","system":true},
                {"key":"workflow","valueType":"select","options":["draft","review"]}
            ])
        );
        let mut meta = Map::new();
        meta.insert("workflow".to_owned(), json!("draft"));
        meta.insert("due_date".to_owned(), json!("2026-09-28"));
        with_validated_metadata(&root, &meta, || Ok(())).unwrap();
        meta.insert("workflow".to_owned(), json!("other"));
        assert!(
            with_validated_metadata(&root, &meta, || Ok(()))
                .unwrap_err()
                .contains("declared option")
        );
        meta.insert("workflow".to_owned(), json!("review"));
        meta.insert("due_date".to_owned(), json!("2026-02-30"));
        assert!(
            with_validated_metadata(&root, &meta, || Ok(()))
                .unwrap_err()
                .contains("date value")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owns_marker_creation_update_and_removal() {
        let root =
            std::env::temp_dir().join(format!("bluink-property-writes-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("properties")).unwrap();
        std::fs::create_dir_all(root.join("items")).unwrap();
        assert!(create_property(&root, json!({"key":"Not valid","valueType":"select"})).is_err());
        assert!(create_property(&root, json!({"key":"summary","valueType":"text"})).is_err());
        let created = create_property(
            &root,
            json!({
                "key":"workflow", "valueType":"select", "options":["review","draft","review"]
            }),
        )
        .unwrap();
        assert_eq!(
            created,
            json!({"key":"workflow","valueType":"select","options":["draft","review"]})
        );
        assert!(
            create_property(&root, json!({"key":"workflow","valueType":"text"}))
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        let updated =
            add_property_options(&root, "workflow", "select", json!(["in_progress", "draft"]))
                .unwrap();
        assert_eq!(
            updated["options"],
            json!(["draft", "in_progress", "review"])
        );
        assert_eq!(
            std::fs::read_to_string(root.join("properties/workflow.select.property")).unwrap(),
            r#"{"format":"bluink.property","formatVersion":1,"options":["draft","in_progress","review"]}"#
        );
        assert_eq!(list_property_declarations(&root).unwrap(), json!([updated]));
        assert!(add_property_options(&root, "workflow", "select", json!(["bad option"])).is_err());
        assert!(add_property_options(&root, "workflow", "text", json!(["draft"])).is_err());
        remove_property(&root, "workflow").unwrap();
        assert_eq!(list_property_declarations(&root).unwrap(), json!([]));
        assert!(
            remove_property(&root, "workflow")
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protects_required_properties_but_allows_removing_default_kind() {
        let root = std::env::temp_dir().join(format!(
            "bluink-default-properties-{}",
            uuid::Uuid::now_v7()
        ));
        crate::engine::project::config::open_or_create_project(&root, "UTC").unwrap();
        let fields = list_property_declarations(&root).unwrap();
        let kind = fields
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "kind")
            .unwrap();
        assert_eq!(kind.get("system"), None);
        let status = fields
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "status")
            .unwrap();
        assert_eq!(status["system"], json!(true));
        assert!(
            remove_property(&root, "status")
                .unwrap_err()
                .to_string()
                .contains("required and cannot be deleted")
        );
        remove_property(&root, "kind").unwrap();
        create_property(&root, json!({"key":"custom_field","valueType":"text"})).unwrap();
        remove_property(&root, "custom_field").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
