// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Workspace setups: a way of working shared as a file, without notes beyond
//! the few pages its widgets need, such as an inbox. Rust creates a setup's
//! missing properties and tags, its templates, pages and workspace together,
//! in one recoverable write.
use super::{
    assemble::AuthoringValues,
    properties::{SetupDeclarations, SetupProperty},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const FORMAT: &str = "bluink.workspace-setup";
/// The most templates one setup may bring.
const MAX_TEMPLATES: usize = 24;
/// The most pages one setup may bring.
const MAX_PAGES: usize = 8;
/// Metadata a template value may not set: the envelope belongs to the template.
const ENVELOPE_KEYS: [&str; 4] = ["type", "state", "title", "propertyKeys"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkspaceSetup {
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
    /// Pages the workspace's widgets name by key, such as an inbox.
    #[serde(default)]
    pages: Vec<SetupTemplate>,
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

fn invalid(message: impl Into<String>) -> String {
    message.into()
}

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}

/// Parse and bound the setup before the shell performs any record reads.
pub(crate) fn parse_setup(setup: &Value, title: &str) -> Result<WorkspaceSetup, String> {
    let setup: WorkspaceSetup = serde_json::from_value(setup.clone())
        .map_err(|error| invalid(format!("This is not a valid workspace setup: {error}")))?;
    setup.validate(title.trim())?;
    Ok(setup)
}

impl WorkspaceSetup {
    fn validate(&self, title: &str) -> Result<(), String> {
        if self.format != FORMAT || self.version != 1 {
            return Err(invalid(
                "This file is not a Bluink workspace setup, or it is from a newer version.",
            ));
        }
        if !text(&self.name, 80) || !text(&self.summary, 300) {
            return Err(invalid("The setup needs a name and a one-line summary."));
        }
        if !text(title, 120) {
            return Err(invalid("Enter a workspace name of at most 120 characters."));
        }
        if self.templates.len() > MAX_TEMPLATES {
            return Err(invalid(format!(
                "A setup can bring at most {MAX_TEMPLATES} templates."
            )));
        }
        if self.pages.len() > MAX_PAGES {
            return Err(invalid(format!(
                "A setup can bring at most {MAX_PAGES} pages."
            )));
        }

        Ok(())
    }

    /// Only these declarations/references are needed; no project-wide scan.
    pub(crate) fn required_records(&self) -> Result<BTreeSet<String>, String> {
        let mut ids = BTreeSet::new();
        for property in &self.properties {
            ids.insert(super::named_items::id("property", &property.key));
        }
        for template in self.templates.iter().chain(&self.pages) {
            for key in template.values.keys().chain(&template.property_keys) {
                ids.insert(super::named_items::id("property", key));
            }
            for key in &template.tags {
                ids.insert(super::named_items::id("tag", key));
            }
            super::snippets::validate_references_with(&template.document, |key| {
                ids.insert(super::snippets::item_id(key)?);
                Ok(())
            })?;
        }
        Ok(ids)
    }
}

pub(crate) struct SetupPlan {
    pub(crate) workspace: Value,
    pub(crate) related: Vec<Value>,
}

pub(crate) fn prepare_workspace_from_setup(
    setup: &WorkspaceSetup,
    title: &str,
    records: &BTreeMap<String, Value>,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<SetupPlan, String> {
    setup.validate(title.trim())?;
    let title = title.trim();

    // Declarations and references are checked and written as one change.
    let mut declarations = SetupDeclarations::prepare(records, &setup.properties, values)?;

    let templates = template_items(
        records,
        &setup.templates,
        "template",
        &mut declarations,
        timezone,
        values,
    )?;
    let pages = template_items(
        records,
        &setup.pages,
        "page",
        &mut declarations,
        timezone,
        values,
    )?;
    let tag_items = tag_items(
        records,
        templates.tags.union(&pages.tags).cloned().collect(),
        values,
    )?;
    let workspace = workspace_item(
        &setup.workspace,
        title,
        &templates.ids,
        &pages.ids,
        timezone,
        values,
    )?;

    let related: Vec<Value> = declarations
        .items
        .into_iter()
        .chain(tag_items)
        .chain(templates.items)
        .chain(pages.items)
        .collect();
    Ok(SetupPlan { workspace, related })
}

/// A setup's templates as new items, their IDs by setup key, and their tags.
struct PreparedTemplates {
    ids: HashMap<String, String>,
    items: Vec<Value>,
    tags: BTreeSet<String>,
}

/// The setup's templates or pages, as new items of `kind`, by setup key, and
/// the tags they use.
fn template_items(
    records: &BTreeMap<String, Value>,
    setup: &[SetupTemplate],
    kind: &str,
    declarations: &mut SetupDeclarations,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<PreparedTemplates, String> {
    let mut template_ids = HashMap::new();
    let mut templates = Vec::new();
    let mut tags = BTreeSet::new();
    for template in setup {
        let noun = if kind == "page" { "Page" } else { "Template" };
        let owner = format!("{noun} {}", template.title);
        if !text(&template.key, 64) || !text(&template.title, 160) {
            return Err(invalid(format!(
                "Each {} needs a key and a title.",
                noun.to_lowercase()
            )));
        }
        let id = values.next_id()?;
        if template_ids
            .insert(template.key.clone(), id.clone())
            .is_some()
        {
            return Err(invalid(format!("{noun} {} appears twice.", template.key)));
        }
        let mut meta = Map::new();
        for (key, value) in &template.values {
            if ENVELOPE_KEYS.contains(&key.as_str()) {
                return Err(invalid(format!("{owner} cannot set {key}.")));
            }
            declarations.check(records, &owner, key, Some(value))?;
            meta.insert(key.clone(), value.clone());
        }
        for key in &template.property_keys {
            declarations.check(records, &owner, key, None)?;
        }
        meta.insert("type".into(), json!(kind));
        meta.insert("state".into(), json!("active"));
        meta.insert("title".into(), json!(template.title.trim()));
        if !template.property_keys.is_empty() {
            meta.insert("propertyKeys".into(), json!(template.property_keys));
        }
        tags.extend(template.tags.iter().cloned());
        let item = super::assemble::assemble_item(
            &json!({"id": id, "meta": meta, "tags": template.tags, "content": {"document": template.document}}),
            None,
            timezone,
            values,
        )?;
        super::item::validate_item(&item, &id)?;
        let document = crate::domain::document::document_root(&item["content"]);
        crate::domain::document::links::validate_http_links(document, None)?;
        super::snippets::validate_references_with(document, |key| {
            let record = records
                .get(&super::snippets::item_id(key)?)
                .ok_or_else(|| format!("Snippet {key} does not exist."))?;
            if record["meta"]["state"] != "active" {
                return Err(format!("Snippet {key} is archived."));
            }
            Ok(())
        })?;
        templates.push(item);
    }
    Ok(PreparedTemplates {
        ids: template_ids,
        items: templates,
        tags,
    })
}

/// Declarations for the tags templates use that the project lacks.
fn tag_items(
    records: &BTreeMap<String, Value>,
    tags: BTreeSet<String>,
    values: &mut AuthoringValues<'_>,
) -> Result<Vec<Value>, String> {
    let mut tag_items = Vec::new();
    for key in tags {
        let existing = records.get(&super::named_items::id("tag", &key));
        if existing
            .as_ref()
            .is_some_and(|record| record["meta"]["state"] == "active")
        {
            continue;
        }
        super::facets::validate_content("tag", &json!({"key": key}))?;
        let id = super::named_items::id("tag", &key);
        let item = super::assemble::assemble_item(
            &json!({"id": id, "meta": {"type": "tag", "state": "active", "title": key}, "content": {"key": key}}),
            existing,
            "UTC",
            values,
        )?;
        super::item::validate_item(&item, &id)?;

        tag_items.push(item);
    }
    Ok(tag_items)
}

/// The workspace, with fresh widget IDs and templates and pages named by
/// their new IDs.
fn workspace_item(
    setup: &SetupWorkspace,
    title: &str,
    template_ids: &HashMap<String, String>,
    page_ids: &HashMap<String, String>,
    timezone: &str,
    values: &mut AuthoringValues<'_>,
) -> Result<Value, String> {
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
        // A task list names its page, such as an inbox, by its setup key.
        if widget["config"]["kind"] == "tasks"
            && let Some(key) = widget["config"]["noteId"].as_str()
        {
            let id = page_ids.get(key).cloned().ok_or_else(|| {
                invalid(format!(
                    "The workspace uses page {key}, which the setup does not include."
                ))
            })?;
            widget["config"]["noteId"] = json!(id);
        }
        widget.insert("id".into(), json!(values.next_id()?));
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
    let id = values.next_id()?;
    let workspace = super::assemble::assemble_item(
        &json!({"id": id, "meta": meta, "content": content}),
        None,
        timezone,
        values,
    )?;
    super::item::validate_item(&workspace, &id)?;
    Ok(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_setup_preparation_is_repeatable_from_supplied_records_and_identities() {
        let input: Value = serde_json::from_str(include_str!(
            "../../../resources/workspace-setups/para.json"
        ))
        .unwrap();
        let setup = parse_setup(&input, "My projects").unwrap();
        let records = super::super::config::default_user_properties()
            .into_iter()
            .map(|content| {
                let key = content["key"].as_str().unwrap();
                let id = super::super::named_items::id("property", key);
                (
                    id.clone(),
                    json!({"id":id,"meta":{"type":"property","state":"active"},"content":content}),
                )
            })
            .collect();
        let prepare = || {
            let mut ids = (1..1024).map(|i| format!("11111111-1111-7111-8111-{i:012x}"));
            let mut values = AuthoringValues {
                timestamp: "2026-11-05T12:00:00.000Z",
                ids: &mut ids,
            };
            prepare_workspace_from_setup(&setup, "My projects", &records, "UTC", &mut values)
                .unwrap()
        };
        let first = prepare();
        let second = prepare();
        assert_eq!(first.workspace, second.workspace);
        assert_eq!(first.related, second.related);
        assert_eq!(first.workspace["modifiedAt"], "2026-11-05T12:00:00.000Z");
        assert_ne!(first.related, [] as [Value; 0]);
        for item in first
            .related
            .iter()
            .chain(std::iter::once(&first.workspace))
        {
            super::super::item::validate_item(item, item["id"].as_str().unwrap()).unwrap();
        }
    }
}
