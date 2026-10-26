// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Workspace setups: a way of working shared as a file, without notes. Rust
//! creates a setup's missing properties and tags, its templates and its
//! workspace together, in one recoverable write.
use super::io::LockMode;
use super::properties::{SetupDeclarations, SetupProperty};
use super::write::MutationError;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use uuid::Uuid;

const FORMAT: &str = "bluink.workspace-setup";
/// The most templates one setup may bring.
const MAX_TEMPLATES: usize = 24;
/// Metadata a template value may not set: the envelope belongs to the template.
const ENVELOPE_KEYS: [&str; 4] = ["type", "state", "title", "propertyKeys"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceSetup {
    format: String,
    version: u8,
    name: String,
    summary: String,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "Shown by the gallery; the workspace does not store it."
    )]
    description: Option<String>,
    properties: Vec<SetupProperty>,
    templates: Vec<SetupTemplate>,
    workspace: SetupWorkspace,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetupTemplate {
    /// Names the template within the setup file only.
    key: String,
    title: String,
    #[serde(default)]
    values: Map<String, Value>,
    #[serde(default)]
    property_keys: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    document: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetupWorkspace {
    /// The suggested name; the person's chosen name replaces it.
    #[expect(dead_code, reason = "The request carries the name the person chose.")]
    title: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    icon: Option<Value>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    cover: Option<Value>,
    query: Value,
    widgets: Vec<Value>,
    #[serde(default)]
    templates: Option<SetupOffers>,
    #[serde(default)]
    calendar: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupOffers {
    offered: Vec<String>,
    default: Option<String>,
}

fn invalid(message: impl Into<String>) -> MutationError {
    MutationError::Validation(message.into())
}

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}

/// Creates a workspace named `title` from `setup`, with fresh IDs for its
/// templates, workspace and widgets. Returns the workspace and the other
/// items written with it.
pub(crate) fn create_workspace_from_setup(
    root: &Path,
    setup: &Value,
    title: &str,
) -> Result<Value, MutationError> {
    let setup: WorkspaceSetup = serde_json::from_value(setup.clone())
        .map_err(|error| invalid(format!("This is not a valid workspace setup: {error}")))?;
    if setup.format != FORMAT || setup.version != 1 {
        return Err(invalid(
            "This file is not a Bluink workspace setup, or it is from a newer version.",
        ));
    }
    if !text(&setup.name, 80) || !text(&setup.summary, 300) {
        return Err(invalid("The setup needs a name and a one-line summary."));
    }
    let title = title.trim();
    if !text(title, 120) {
        return Err(invalid("Enter a workspace name of at most 120 characters."));
    }
    if setup.templates.len() > MAX_TEMPLATES {
        return Err(invalid(format!(
            "A setup can bring at most {MAX_TEMPLATES} templates."
        )));
    }

    let _gate = super::transaction::lock(root, LockMode::Exclusive)?;
    super::transaction::recover(root).map_err(MutationError::Internal)?;
    let config = super::config::read_project_config(root)
        .map_err(MutationError::Internal)?
        .ok_or_else(|| MutationError::Internal("Project configuration is missing.".to_owned()))?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| MutationError::Internal("Project has no timezone".to_owned()))?
        .to_owned();
    // Declarations and references are checked and written as one change.
    let _relationships = super::write::lock_relationships(root)?;
    let mut declarations = SetupDeclarations::prepare(root, &setup.properties)?;

    let templates = template_items(root, &setup.templates, &mut declarations, &timezone)?;
    let tag_items = tag_items(root, templates.tags)?;
    let workspace = workspace_item(&setup.workspace, title, &templates.ids, &timezone)?;

    let related: Vec<Value> = declarations
        .items
        .into_iter()
        .chain(tag_items)
        .chain(templates.items)
        .collect();
    let mut group = related.clone();
    group.push(workspace.clone());
    super::transaction::write_group(root, group).map_err(MutationError::Internal)?;
    Ok(json!({"item": workspace, "relatedItems": related}))
}

/// A setup's templates as new items, their IDs by setup key, and their tags.
struct PreparedTemplates {
    ids: HashMap<String, String>,
    items: Vec<Value>,
    tags: BTreeSet<String>,
}

/// The setup's templates as new items, by setup key, and the tags they use.
fn template_items(
    root: &Path,
    setup: &[SetupTemplate],
    declarations: &mut SetupDeclarations,
    timezone: &str,
) -> Result<PreparedTemplates, MutationError> {
    let mut template_ids = HashMap::new();
    let mut templates = Vec::new();
    let mut tags = BTreeSet::new();
    for template in setup {
        let owner = format!("Template {}", template.title);
        if !text(&template.key, 64) || !text(&template.title, 160) {
            return Err(invalid("Each template needs a key and a title."));
        }
        let id = Uuid::now_v7().to_string();
        if template_ids
            .insert(template.key.clone(), id.clone())
            .is_some()
        {
            return Err(invalid(format!("Template {} appears twice.", template.key)));
        }
        let mut meta = Map::new();
        for (key, value) in &template.values {
            if ENVELOPE_KEYS.contains(&key.as_str()) {
                return Err(invalid(format!("{owner} cannot set {key}.")));
            }
            declarations.check(root, &owner, key, Some(value))?;
            meta.insert(key.clone(), value.clone());
        }
        for key in &template.property_keys {
            declarations.check(root, &owner, key, None)?;
        }
        meta.insert("type".into(), json!("template"));
        meta.insert("state".into(), json!("active"));
        meta.insert("title".into(), json!(template.title.trim()));
        if !template.property_keys.is_empty() {
            meta.insert("propertyKeys".into(), json!(template.property_keys));
        }
        tags.extend(template.tags.iter().cloned());
        let item = super::assemble::assemble_item(
            &json!({"id": id, "meta": meta, "tags": template.tags, "content": {"content": template.document}}),
            None,
            timezone,
        )
        .map_err(MutationError::Validation)?;
        super::item::validate_item(&item, &id).map_err(MutationError::Validation)?;
        let document = crate::engine::document::document_root(&item["content"]);
        crate::engine::document::links::validate_http_links(document, None)
            .map_err(MutationError::Validation)?;
        super::snippets::validate_page_snippet_references(
            root,
            crate::engine::publication::source::document(&item),
        )
        .map_err(MutationError::Validation)?;
        templates.push(item);
    }
    Ok(PreparedTemplates {
        ids: template_ids,
        items: templates,
        tags,
    })
}

/// Declarations for the tags templates use that the project lacks.
fn tag_items(root: &Path, tags: BTreeSet<String>) -> Result<Vec<Value>, MutationError> {
    let mut tag_items = Vec::new();
    for key in tags {
        let existing =
            super::named_items::read(root, "tag", &key).map_err(MutationError::Internal)?;
        if existing
            .as_ref()
            .is_some_and(|record| record["meta"]["state"] == "active")
        {
            continue;
        }
        super::facets::validate_content("tag", &json!({"key": key}))
            .map_err(MutationError::Validation)?;
        let id = super::named_items::id("tag", &key);
        let item = super::assemble::assemble_item(
            &json!({"id": id, "meta": {"type": "tag", "state": "active", "title": key}, "content": {"key": key}}),
            existing.as_ref(),
            "UTC",
        )
        .map_err(MutationError::Validation)?;
        super::item::validate_item(&item, &id).map_err(MutationError::Validation)?;
        super::named_items::validate_mutation(root, existing.as_ref(), &item)?;
        tag_items.push(item);
    }
    Ok(tag_items)
}

/// The workspace, with fresh widget IDs and templates named by their new IDs.
fn workspace_item(
    setup: &SetupWorkspace,
    title: &str,
    template_ids: &HashMap<String, String>,
    timezone: &str,
) -> Result<Value, MutationError> {
    let template_id = |key: &str| {
        template_ids.get(key).cloned().ok_or_else(|| {
            invalid(format!(
                "The workspace uses template {key}, which the setup does not include."
            ))
        })
    };
    let mut widgets = Vec::new();
    for widget in &setup.widgets {
        let mut widget = widget
            .as_object()
            .cloned()
            .ok_or_else(|| invalid("Each widget must be an object."))?;
        if widget.contains_key("id") {
            return Err(invalid("Widgets in a setup must not carry IDs."));
        }
        // Create buttons name templates by their setup keys.
        if widget["config"]["kind"] == "create_buttons" {
            let keys = widget["config"]["templateIds"]
                .as_array()
                .ok_or_else(|| invalid("Create buttons need a list of templates."))?;
            let ids = keys
                .iter()
                .map(|key| template_id(key.as_str().unwrap_or_default()))
                .collect::<Result<Vec<_>, _>>()?;
            widget["config"]["templateIds"] = json!(ids);
        }
        widget.insert("id".into(), json!(Uuid::now_v7().to_string()));
        widgets.push(Value::Object(widget));
    }
    let mut content = json!({"query": setup.query, "widgets": widgets});
    if let Some(offers) = &setup.templates {
        let offered = offers
            .offered
            .iter()
            .map(|key| template_id(key))
            .collect::<Result<Vec<_>, _>>()?;
        let default = offers.default.as_deref().map(template_id).transpose()?;
        content["templates"] = json!({"offered": offered, "default": default});
    }
    if let Some(calendar) = &setup.calendar {
        content["calendar"] = calendar.clone();
    }
    let mut meta = json!({"type": "workspace", "state": "active", "title": title});
    for (key, value) in [
        ("summary", setup.summary.clone().map(Value::String)),
        ("icon", setup.icon.clone()),
        ("color", setup.color.clone().map(Value::String)),
        ("cover", setup.cover.clone()),
    ] {
        if let Some(value) = value {
            meta[key] = value;
        }
    }
    let id = Uuid::now_v7().to_string();
    let workspace = super::assemble::assemble_item(
        &json!({"id": id, "meta": meta, "content": content}),
        None,
        timezone,
    )
    .map_err(MutationError::Validation)?;
    super::item::validate_item(&workspace, &id).map_err(MutationError::Validation)?;
    Ok(workspace)
}
