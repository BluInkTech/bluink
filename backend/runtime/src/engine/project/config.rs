// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs::{self, OpenOptions};
use std::path::Path;

use fs4::fs_std::FileExt;
use serde_json::{Value, json};

use crate::engine::project::io::{create_json_atomic, write_json_atomic};
use crate::engine::project::item::is_item_id;
use crate::engine::project::properties::{create_property, list_property_declarations};
use crate::engine::project::write::MutationError;

pub(crate) const PROJECT_FORMAT_VERSION: u64 = 15;

pub(crate) fn open_or_create_project(root: &Path, timezone: &str) -> Result<Value, String> {
    if timezone.is_empty() {
        return Err("Project timezone must not be empty.".to_owned());
    }
    for directory in [
        "items",
        "analytics/events",
        "facets",
        "properties",
        ".cache",
    ] {
        let path = root.join(directory);
        fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let lock_path = root.join(".cache/project-init.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| format!("{}: {error}", lock_path.display()))?;
    lock.lock_exclusive()
        .map_err(|error| format!("{}: {error}", lock_path.display()))?;
    if let Some(config) = read_project_config(root)? {
        return Ok(config);
    }
    for property in default_user_properties() {
        let declarations = list_property_declarations(root)?;
        let key = property["key"]
            .as_str()
            .ok_or("Default property has no key")?;
        match declarations
            .as_array()
            .and_then(|values| values.iter().find(|value| value["key"] == key))
        {
            Some(existing)
                if existing["key"] == property["key"]
                    && existing["valueType"] == property["valueType"]
                    && existing["options"] == property["options"] => {}
            Some(_) => {
                return Err(format!(
                    "Default property {key} conflicts with an existing marker."
                ));
            }
            None => {
                create_property(root, property).map_err(|error| error.to_string())?;
            }
        }
    }
    let config = json!({
        "format": "bluink-project",
        "formatVersion": PROJECT_FORMAT_VERSION,
        "id": uuid::Uuid::now_v7().to_string(),
        "history": {"enabled": true, "interval": "1h", "retention": {"hourlyDays": 7}},
        "analytics": {"timezone": timezone, "eventRetentionDays": 14}
    });
    create_project_config(root, config.clone())?;
    Ok(config)
}

/// Project defaults that Bluink requires for built-in behavior. This is kept
/// separate from the set seeded into a new project: `kind` is a useful default
/// declaration, but remains an ordinary project property that can be removed.
const REQUIRED_BUILTIN_PROPERTY_KEYS: &[&str] = &[
    "status",
    "priority",
    "start_date",
    "due_date",
    "journal_date",
    "completed_at",
    "progress",
    "lang",
    "color",
    "image",
    "word_target",
];

#[must_use]
pub(crate) fn is_required_builtin_property(key: &str) -> bool {
    REQUIRED_BUILTIN_PROPERTY_KEYS.contains(&key)
}

fn default_user_properties() -> Vec<Value> {
    vec![
        json!({"key":"kind","valueType":"select","options":["article","brief","culture","history","person","place","science","technology"]}),
        json!({"key":"status","valueType":"select","options":["draft","final","review"]}),
        json!({"key":"priority","valueType":"number"}),
        json!({"key":"start_date","valueType":"date"}),
        json!({"key":"due_date","valueType":"date"}),
        json!({"key":"journal_date","valueType":"date"}),
        json!({"key":"completed_at","valueType":"timestamp"}),
        json!({"key":"progress","valueType":"number"}),
        json!({"key":"lang","valueType":"language"}),
        json!({"key":"color","valueType":"color"}),
        json!({"key":"image","valueType":"image"}),
        json!({"key":"word_target","valueType":"number"}),
    ]
}

pub(crate) fn read_project_config(root: &Path) -> Result<Option<Value>, String> {
    let path = root.join("bluink.json");
    match fs::read(&path) {
        Ok(source) => parse_project_config(&source)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub(crate) fn parse_project_config(source: &[u8]) -> Result<Value, String> {
    let config: Value = serde_json::from_slice(source).map_err(|error| error.to_string())?;
    if config.get("format").and_then(Value::as_str) != Some("bluink-project")
        || config.get("formatVersion").and_then(Value::as_u64) != Some(PROJECT_FORMAT_VERSION)
    {
        return Err("Not a supported Bluink file project.".to_owned());
    }
    if !config
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(is_item_id)
    {
        return Err("Project has an invalid ID.".to_owned());
    }
    let history = config
        .get("history")
        .ok_or("Project has no history settings.")?;
    if !history.get("enabled").is_some_and(Value::is_boolean)
        || !matches!(
            history.get("interval").and_then(Value::as_str),
            Some("1h" | "4h" | "1d")
        )
        || !history
            .get("retention")
            .and_then(|retention| retention.get("hourlyDays"))
            .is_some_and(positive_safe_integer)
    {
        return Err("Project has invalid history settings.".to_owned());
    }
    let analytics = config
        .get("analytics")
        .ok_or("Project has no analytics settings.")?;
    if analytics
        .get("timezone")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || !analytics
            .get("eventRetentionDays")
            .is_some_and(positive_safe_integer)
    {
        return Err("Project has invalid analytics settings.".to_owned());
    }
    if config
        .get("defaultView")
        .is_some_and(|view| !view.is_object())
    {
        return Err("Project has an invalid default view.".to_owned());
    }
    Ok(config)
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn create_project_config(root: &Path, config: Value) -> Result<(), String> {
    let path = root.join("bluink.json");
    let source = serde_json::to_vec(&config).map_err(|error| error.to_string())?;
    parse_project_config(&source)?;
    create_json_atomic(&path, &config)
}

pub(crate) fn default_view_query(view: &Value) -> Result<&str, MutationError> {
    if !view.is_object() {
        return Err(MutationError::Expected(
            "Default view must be an object.".to_owned(),
        ));
    }
    view["query"]
        .as_str()
        .ok_or_else(|| MutationError::Expected("Enter a fixed query for All Notes.".to_owned()))
}

pub(crate) fn write_default_view(root: &Path, view: Value) -> Result<(), MutationError> {
    default_view_query(&view)?;
    let mut config = read_project_config(root)
        .map_err(MutationError::Unexpected)?
        .ok_or_else(|| MutationError::Unexpected("Project configuration is missing.".to_owned()))?;
    config
        .as_object_mut()
        .ok_or_else(|| MutationError::Unexpected("Invalid project configuration".to_owned()))?
        .insert("defaultView".to_owned(), view);
    write_json_atomic(&root.join("bluink.json"), &config).map_err(MutationError::Unexpected)
}

fn positive_safe_integer(value: &Value) -> bool {
    value
        .as_i64()
        .is_some_and(|number| (1..=9_007_199_254_740_991).contains(&number))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{
        create_project_config, open_or_create_project, parse_project_config, read_project_config,
        write_default_view,
    };
    use crate::engine::project::properties::{create_property, list_property_declarations};

    #[test]
    fn initializes_a_project_once_and_recovers_partial_default_markers() {
        let root =
            std::env::temp_dir().join(format!("bluink-core-project-open-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("properties")).unwrap();
        create_property(
            &root,
            json!({"key":"status","valueType":"select","options":["draft","final","review"]}),
        )
        .unwrap();
        let config = open_or_create_project(&root, "Europe/London").unwrap();
        assert_eq!(config["analytics"]["timezone"], "Europe/London");
        assert_eq!(
            list_property_declarations(&root)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            12
        );
        assert!(root.join("analytics/events").is_dir());
        assert!(root.join("items").is_dir());
        assert!(root.join("facets").is_dir());
        std::fs::remove_file(root.join("properties/status.select.property")).unwrap();
        let reopened = open_or_create_project(&root, "UTC").unwrap();
        assert_eq!(reopened, config);
        assert_eq!(
            list_property_declarations(&root)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            11
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_portable_project_fixture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config = read_project_config(&root).unwrap().unwrap();
        assert_eq!(config["formatVersion"], 15);
    }

    #[test]
    fn rejects_invalid_project_settings() {
        let mut config = json!({
            "format": "bluink-project",
            "formatVersion": 15,
            "id": "01a0e2a7-d112-7829-b7d6-5f218a1341e0",
            "history": {"enabled": true, "interval": "1h", "retention": {"hourlyDays": 7}},
            "analytics": {"timezone": "Europe/London", "eventRetentionDays": 14}
        });
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_ok());
        config["history"]["interval"] = json!("2h");
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_err());
        config["history"]["interval"] = json!("1h");
        config["analytics"]["eventRetentionDays"] = json!(0);
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_err());
    }

    #[test]
    fn creates_config_once_and_updates_only_default_view() {
        let root = std::env::temp_dir().join(format!("bluink-core-config-{}", std::process::id()));
        let config = json!({
            "format": "bluink-project",
            "formatVersion": 15,
            "id": "01a0e2a7-d112-7829-b7d6-5f218a1341e0",
            "history": {"enabled": true, "interval": "1h", "retention": {"hourlyDays": 7}},
            "analytics": {"timezone": "Europe/London", "eventRetentionDays": 14}
        });
        create_project_config(&root, config.clone()).unwrap();
        assert!(create_project_config(&root, config.clone()).is_err());
        write_default_view(
            &root,
            json!({"query": "sort:modified-desc", "mode": "dashboard"}),
        )
        .unwrap();
        let saved = read_project_config(&root).unwrap().unwrap();
        assert_eq!(saved["history"], config["history"]);
        assert_eq!(saved["defaultView"]["mode"], "dashboard");
        assert!(write_default_view(&root, json!("invalid")).is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
