// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs;
use std::path::Path;

use crate::engine::project::io::{LockMode, lock_file};
use serde_json::{Value, json};

use crate::engine::project::identity::is_item_id;
use crate::engine::project::io::{create_json_atomic, write_json_atomic};
use crate::engine::project::properties::{create_property, list_property_declarations};
use crate::engine::project::write::MutationError;

pub(crate) const PROJECT_FORMAT_VERSION: u64 = 17;

pub(crate) fn open_or_create_project_with_workspace(
    root: &Path,
    timezone: &str,
    template: &Value,
) -> Result<Value, String> {
    if timezone.is_empty() {
        return Err("Project timezone must not be empty.".to_owned());
    }
    for directory in ["items", "analytics/events", ".cache"] {
        let path = root.join(directory);
        fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let _lock = lock_file(&root.join(".cache/project-init.lock"), LockMode::Exclusive)
        .map_err(|error| error.to_string())?;
    if let Some(config) = read_project_config(root)? {
        list_property_declarations(root)?;
        super::facets::list_facets(root, None)?;
        read_default_workspace(root)?;
        return Ok(config);
    }
    super::facets::initialize(root).map_err(|error| error.to_string())?;
    super::properties::initialize(root).map_err(|error| error.to_string())?;
    let declarations = list_property_declarations(root)?;
    for property in default_user_properties() {
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
                    "Default property {key} conflicts with an existing declaration."
                ));
            }
            None => {
                create_property(root, property).map_err(|error| error.to_string())?;
            }
        }
    }
    // Keep the newly allocated workspace identity across an interrupted first
    // initialization; it is a bootstrap intent, never an old-data migration.
    let intent_path = root.join(".cache/main-workspace.json");
    let workspace_id = match fs::read(&intent_path) {
        Ok(bytes) => {
            serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())?["id"]
                .as_str()
                .filter(|id| is_item_id(id))
                .ok_or("Invalid workspace bootstrap identity.")?
                .to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = uuid::Uuid::now_v7().to_string();
            create_json_atomic(&intent_path, &json!({"id":id}))
                .map_err(|error| error.to_string())?;
            id
        }
        Err(error) => return Err(error.to_string()),
    };
    if super::item::read_project_item(root, &workspace_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        let workspace = super::assemble::assemble_item(
            &json!({"id":workspace_id,"meta":{"type":"workspace","title":"All notes"},"content":super::workspace::instantiate(template)?}),
            None,
            timezone,
        )?;
        super::item::validate_item(&workspace, &workspace_id)?;
        create_json_atomic(
            &super::identity::ItemId::try_from(workspace_id.as_str())?.path(root),
            &workspace,
        )
        .map_err(|error| error.to_string())?;
    }
    let config = json!({
        "format": "bluink-project",
        "formatVersion": PROJECT_FORMAT_VERSION,
        "id": uuid::Uuid::now_v7().to_string(),
        "defaultWorkspaceId": workspace_id,
        "history": {"enabled": true, "interval": "1h", "retention": {"hourlyDays": 7}},
        "analytics": {"timezone": timezone, "eventRetentionDays": 14}
    });
    create_project_config(root, config.clone())?;
    let _ = fs::remove_file(intent_path);
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
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn create_project_config(root: &Path, config: Value) -> Result<(), String> {
    let path = root.join("bluink.json");
    let source = serde_json::to_vec(&config).map_err(|error| error.to_string())?;
    parse_project_config(&source)?;
    create_json_atomic(&path, &config).map_err(|error| error.to_string())
}

pub(crate) fn read_default_workspace(root: &Path) -> Result<Value, String> {
    read_workspace_context(root).map(|(_, workspace)| workspace)
}

pub(crate) fn read_workspace_context(root: &Path) -> Result<(String, Value), String> {
    let _relationships =
        super::write::lock_relationships(root).map_err(|error| error.to_string())?;
    let config = read_project_config(root)?.ok_or("Project configuration is missing.")?;
    let project_id = config["id"]
        .as_str()
        .ok_or("Project identity is missing.")?
        .to_owned();
    let id = config["defaultWorkspaceId"]
        .as_str()
        .ok_or("Project has no default workspace.")?;
    let item = super::item::read_project_item(root, id)
        .map_err(|error| error.to_string())?
        .ok_or("The default workspace is missing.")?;
    if item["meta"]["type"] != "workspace" || item["meta"]["state"] != "active" {
        return Err("The default must be an active workspace.".into());
    }
    Ok((project_id, item))
}

pub(crate) fn set_default_workspace(root: &Path, id: &str) -> Result<Value, MutationError> {
    if !is_item_id(id) {
        return Err(MutationError::Validation(
            "Choose an active workspace as the default.".into(),
        ));
    }
    let _relationships = super::write::lock_relationships(root).map_err(MutationError::Storage)?;
    let workspace = super::item::read_project_item(root, id)
        .map_err(MutationError::Read)?
        .filter(|item| item["meta"]["type"] == "workspace" && item["meta"]["state"] == "active")
        .ok_or_else(|| {
            MutationError::Validation("Choose an active workspace as the default.".into())
        })?;
    let mut config = read_project_config(root)
        .map_err(MutationError::Internal)?
        .ok_or_else(|| MutationError::Internal("Project configuration is missing.".to_owned()))?;
    config
        .as_object_mut()
        .ok_or_else(|| MutationError::Internal("Invalid project configuration".to_owned()))?
        .insert("defaultWorkspaceId".to_owned(), json!(id));
    write_json_atomic(&root.join("bluink.json"), &config).map_err(MutationError::Storage)?;
    Ok(workspace)
}

fn positive_safe_integer(value: &Value) -> bool {
    value
        .as_i64()
        .is_some_and(|number| (1..=9_007_199_254_740_991).contains(&number))
}

#[cfg(test)]
pub(crate) fn open_or_create_project(root: &Path, timezone: &str) -> Result<Value, String> {
    open_or_create_project_with_workspace(root, timezone, &super::workspace::initial_content())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{
        create_project_config, open_or_create_project, parse_project_config, read_project_config,
        set_default_workspace,
    };
    use crate::engine::project::properties::{create_property, list_property_declarations};

    #[test]
    fn initializes_catalogues_once_and_recovers_partial_bootstrap() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::engine::project::properties::initialize(&root).unwrap();
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
        assert!(!root.join("catalogue").exists());
        crate::engine::project::properties::remove_property(&root, "kind", || Ok(false)).unwrap();
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
        temporary.close().unwrap();
    }

    #[test]
    fn reads_portable_project_fixture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config = read_project_config(&root).unwrap().unwrap();
        assert_eq!(config["formatVersion"], 17);
    }

    #[test]
    fn existing_projects_reject_invalid_declaration_items_without_overwriting_them() {
        let root = tempfile::tempdir().unwrap();
        open_or_create_project(root.path(), "UTC").unwrap();
        let tags = super::super::named_items::path(root.path(), "property", "status").unwrap();
        std::fs::write(&tags, "invalid JSON").unwrap();
        assert!(open_or_create_project(root.path(), "UTC").is_err());
        assert_eq!(std::fs::read_to_string(tags).unwrap(), "invalid JSON");
    }

    #[test]
    fn rejects_invalid_project_settings() {
        let mut config = json!({
                   "format": "bluink-project",
                   "formatVersion": 17,
                   "id": "01a0e2a7-d112-7829-b7d6-5f218a1341e0",
        "defaultWorkspaceId":"00000000-0000-7000-8000-000000000101",
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
    fn creates_config_once_and_updates_only_default_workspace_reference() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let config = open_or_create_project(&root, "Europe/London").unwrap();
        assert!(create_project_config(&root, config.clone()).is_err());
        let main = super::read_default_workspace(&root).unwrap();
        assert_eq!(main["id"], config["defaultWorkspaceId"]);
        assert_eq!(main["meta"]["title"], "All notes");
        set_default_workspace(&root, main["id"].as_str().unwrap()).unwrap();
        let saved = read_project_config(&root).unwrap().unwrap();
        assert_eq!(saved, config);
        assert!(set_default_workspace(&root, "missing").is_err());
        temporary.close().unwrap();
    }
}
