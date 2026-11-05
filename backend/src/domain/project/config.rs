// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::project::identity::is_item_id;
use serde_json::{Value, json};
#[cfg(test)]
use std::path::Path;

pub(crate) const PROJECT_FORMAT_VERSION: u64 = 20;

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

pub(crate) fn default_user_properties() -> Vec<Value> {
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
    if config.get("history").is_some() {
        return Err("Hourly history settings are not part of this project format.".to_owned());
    }
    let analytics = config
        .get("analytics")
        .ok_or("Project has no analytics settings.")?;
    if analytics
        .get("timezone")
        .and_then(Value::as_str)
        .and_then(|zone| zone.parse::<chrono_tz::Tz>().ok())
        .is_none()
        || !analytics
            .get("eventRetentionDays")
            .is_some_and(positive_safe_integer)
    {
        return Err("Project has invalid analytics settings.".to_owned());
    }
    if config.get("defaultView").is_some()
        || !config
            .get("defaultWorkspaceId")
            .and_then(Value::as_str)
            .is_some_and(is_item_id)
    {
        return Err(
            "Project needs a valid defaultWorkspaceId and normal workspace records.".to_owned(),
        );
    }
    Ok(config)
}
/// Seed a fixture with its chosen project identity and explicit item set.
#[cfg(test)]
pub(crate) fn create_project_config(root: &Path, config: Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(&config).map_err(|error| error.to_string())?;
    parse_project_config(&bytes)?;
    let project_id =
        uuid::Uuid::parse_str(config["id"].as_str().ok_or("Missing project identity")?)
            .map_err(|error| error.to_string())?;
    let store = crate::store::Store::create(root, project_id).map_err(|error| error.to_string())?;
    crate::versioning::checkpoints::initialize(root).map_err(|error| error.to_string())?;
    store
        .write(move |tx| {
            crate::store::git::put_record(
                tx,
                "bluink.json",
                &config,
                &uuid::Uuid::now_v7().to_string(),
                0,
            )
        })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn read_default_workspace(root: &Path) -> Result<Value, String> {
    crate::store::Store::shared(root)
        .and_then(|store| {
            store.read(|tx| {
                let config = crate::store::project::read_config(tx)?;
                let id = config["defaultWorkspaceId"].as_str().ok_or_else(|| {
                    crate::store::Error::corrupt("Project has no default workspace")
                })?;
                crate::store::item::read(tx, id, true)?
                    .filter(|value| {
                        value["meta"]["type"] == "workspace" && value["meta"]["state"] == "active"
                    })
                    .ok_or_else(|| {
                        crate::store::Error::corrupt(
                            "The default workspace is missing or inactive.",
                        )
                    })
            })
        })
        .map_err(|error| error.to_string())
}

fn positive_safe_integer(value: &Value) -> bool {
    value
        .as_i64()
        .is_some_and(|number| (1..=9_007_199_254_740_991).contains(&number))
}

#[cfg(test)]
pub(crate) fn open_or_create_project(root: &Path, timezone: &str) -> Result<Value, String> {
    crate::store::project::open_or_create(root, timezone, &super::workspace::initial_content())
        .map(|(_, config)| config)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use std::path::Path;

    use serde_json::json;

    use super::{create_project_config, open_or_create_project, parse_project_config};
    use crate::store::snapshots::read_project_config;

    #[test]
    fn reads_portable_project_fixture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config =
            parse_project_config(&std::fs::read(root.join("bluink.json")).unwrap()).unwrap();
        assert_eq!(config["formatVersion"], super::PROJECT_FORMAT_VERSION);
    }

    #[test]
    fn rejects_invalid_project_settings() {
        let mut config = json!({
                   "format": "bluink-project",
                   "formatVersion": 20,
                   "id": "01a0e2a7-d112-7829-b7d6-5f218a1341e0",
        "defaultWorkspaceId":"00000000-0000-7000-8000-000000000101",
                   "analytics": {"timezone": "Europe/London", "eventRetentionDays": 14}
               });
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_ok());
        config["history"]["interval"] = json!("2h");
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_err());
        config.as_object_mut().unwrap().remove("history");
        config["analytics"]["eventRetentionDays"] = json!(0);
        assert!(parse_project_config(&serde_json::to_vec(&config).unwrap()).is_err());
    }

    #[test]
    fn creates_config_once_with_a_valid_default_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let config = open_or_create_project(&root, "Europe/London").unwrap();
        assert!(create_project_config(&root, config.clone()).is_err());
        let main = super::read_default_workspace(&root).unwrap();
        assert_eq!(main["id"], config["defaultWorkspaceId"]);
        assert_eq!(main["meta"]["title"], "All notes");
        let saved = read_project_config(&root).unwrap().unwrap();
        assert_eq!(saved, config);
        temporary.close().unwrap();
    }
}
