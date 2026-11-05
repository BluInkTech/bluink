// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Relationship validation over captured candidates, shared by restore and merge.
//! This module never consults live paths or the current search index.
use crate::domain::project::{
    config,
    envelope::ItemEnvelope,
    item, named_items,
    properties::ValidatedDeclaration,
    snippets, task,
    types::{ItemState, ItemType},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NamedKind {
    Tag,
    Label,
    Property,
    Snippet,
}

impl NamedKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Label => "label",
            Self::Property => "property",
            Self::Snippet => "snippet",
        }
    }
    pub(crate) fn parse(kind: &str) -> Option<Self> {
        match kind {
            "tag" => Some(Self::Tag),
            "label" => Some(Self::Label),
            "property" => Some(Self::Property),
            "snippet" => Some(Self::Snippet),
            _ => None,
        }
    }
    pub(crate) const fn protect_archived_owners(self) -> bool {
        matches!(self, Self::Snippet)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", content = "target", rename_all = "snake_case")]
pub(crate) enum Dependency {
    Named { kind: NamedKind, key: String },
    Item(String),
}

impl Dependency {
    pub(crate) fn named(kind: NamedKind, key: &str) -> Self {
        Self::Named {
            kind,
            key: key.to_owned(),
        }
    }
    pub(crate) fn target_id(&self) -> String {
        match self {
            Self::Named { kind, key } => named_items::id(kind.as_str(), key),
            Self::Item(id) => id.clone(),
        }
    }
}

/// Identifies the affected field's owner and its constraint, so the next merge
/// layer can form dependency groups without parsing diagnostic text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Violation {
    pub(crate) owner: String,
    pub(crate) dependency: Dependency,
    pub(crate) message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Item {}: {}", self.owner, self.message)
    }
}

struct Target {
    kind: ItemType,
    state: ItemState,
}

/// Small captured summaries, not a second search index or cache of live state.
#[derive(Default)]
pub(crate) struct Catalogue {
    targets: BTreeMap<String, Target>,
    declarations: BTreeMap<String, ValidatedDeclaration>,
}

impl Catalogue {
    pub(crate) fn observe(&mut self, id: &str, value: &Value) -> Result<(), String> {
        item::validate_item(value, id)?;
        let envelope = ItemEnvelope::read(value, id)?;
        let declaration =
            if envelope.kind == ItemType::Property && envelope.state == ItemState::Active {
                let key = envelope.content["key"]
                    .as_str()
                    .ok_or("Property has no key")?;
                Some((key, ValidatedDeclaration::read(envelope.content)?))
            } else {
                None
            };
        self.observe_summary(id, envelope.kind, envelope.state, declaration)
    }

    /// Database snapshots already contain validated canonical items. Relationships
    /// need only their typed state and small property definitions, never prose.
    pub(crate) fn observe_summary(
        &mut self,
        id: &str,
        kind: ItemType,
        state: ItemState,
        declaration: Option<(&str, ValidatedDeclaration)>,
    ) -> Result<(), String> {
        if self.targets.contains_key(id) {
            return Err(format!("Duplicate candidate identity: {id}"));
        }
        if let Some((key, declaration)) = declaration {
            self.declarations.insert(key.to_owned(), declaration);
        }
        self.targets.insert(id.to_owned(), Target { kind, state });
        Ok(())
    }

    pub(crate) fn default_workspace(&self, config: &Value) -> Result<Option<Violation>, String> {
        // Both entry points validate the configuration before loading records.
        let id = config["defaultWorkspaceId"]
            .as_str()
            .ok_or("Project has no default workspace")?;
        Ok((!self.targets.get(id).is_some_and(|target| {
            target.kind == ItemType::Workspace && target.state == ItemState::Active
        }))
        .then(|| Violation {
            owner: "bluink.json".into(),
            dependency: Dependency::Item(id.to_owned()),
            message: "The default must be an active workspace.".into(),
        }))
    }

    pub(crate) fn check(&self, value: &Value) -> Result<Vec<Violation>, String> {
        let mut keys = BTreeSet::new();
        if matches!(value["meta"]["type"].as_str(), Some("page" | "template")) {
            snippets::validate_references_with(
                crate::domain::document::document_root(&value["content"]),
                |key| {
                    keys.insert(key.to_owned());
                    Ok(())
                },
            )?;
        }
        validate_relationships(
            value,
            &self.targets,
            &self.declarations,
            keys.iter().map(String::as_str),
        )
    }

    /// `value` may omit prose after canonical validation. References must have
    /// been projected from that exact captured item, before its body was dropped.
    pub(crate) fn check_captured(
        &self,
        value: &Value,
        references: &BTreeSet<Dependency>,
    ) -> Result<Vec<Violation>, String> {
        let snippets = references.iter().filter_map(|dependency| match dependency {
            Dependency::Named {
                kind: NamedKind::Snippet,
                key,
            } => Some(key.as_str()),
            _ => None,
        });
        validate_relationships(value, &self.targets, &self.declarations, snippets)
    }
}

/// Forward references whose canonical targets the relationship checker reads.
/// Free metadata/tags/labels remain valid when no declaration has ever existed.
pub(crate) fn dependencies(value: &Value) -> Result<BTreeSet<Dependency>, String> {
    let mut result = BTreeSet::new();
    for (key, value) in value["meta"]
        .as_object()
        .ok_or("Item metadata must be an object")?
    {
        if !value.is_null() {
            result.insert(Dependency::named(NamedKind::Property, key));
        }
    }
    for tag in value["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        result.insert(Dependency::named(NamedKind::Tag, tag));
    }
    for pin in value["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        result.insert(Dependency::Item(pin.to_owned()));
    }
    if value["meta"]["type"] == "task"
        && let Some(owner) = value["content"]["noteId"].as_str()
    {
        result.insert(Dependency::Item(owner.to_owned()));
    }
    result.extend(publication_targets(value).into_iter().map(Dependency::Item));
    if matches!(value["meta"]["type"].as_str(), Some("page" | "template")) {
        let document = crate::domain::document::document_root(&value["content"]);
        for label in crate::domain::document::projection::project_document(document).labels {
            result.insert(Dependency::named(NamedKind::Label, &label));
        }
        snippets::validate_references_with(document, |key| {
            result.insert(Dependency::named(NamedKind::Snippet, key));
            Ok(())
        })?;
    }
    Ok(result)
}

/// Explicit publication pages, including section introductions. Dynamic
/// collection queries intentionally do not create stored membership edges.
pub(crate) fn publication_targets(value: &Value) -> BTreeSet<String> {
    fn outline(nodes: &Value, result: &mut BTreeSet<String>) {
        for node in nodes.as_array().into_iter().flatten() {
            for key in ["pageId", "introPageId"] {
                if let Some(id) = node[key].as_str() {
                    result.insert(id.to_owned());
                }
            }
            outline(&node["children"], result);
        }
    }
    let mut result = BTreeSet::new();
    if value["meta"]["type"] == "publication" {
        if let Some(id) = value["content"]["rootPageId"].as_str() {
            result.insert(id.to_owned());
        }
        outline(&value["content"]["outline"], &mut result);
    }
    result
}

/// The source must be an immutable snapshot. Two passes retain only identities,
/// states and property declarations, never all page documents at once.
pub(crate) fn validate<'a>(
    config: &Value,
    ids: impl Iterator<Item = &'a str> + Clone,
    mut read: impl FnMut(&str) -> Result<Value, String>,
) -> Result<(), String> {
    config::parse_project_config(&serde_json::to_vec(config).map_err(|error| error.to_string())?)?;
    let mut catalogue = Catalogue::default();
    for id in ids.clone() {
        let value = read(id)?;
        catalogue.observe(id, &value)?;
    }
    if let Some(issue) = catalogue.default_workspace(config)? {
        return Err(issue.to_string());
    }
    for id in ids {
        let value = read(id)?;
        if let Some(issue) = catalogue.check(&value)?.first() {
            return Err(issue.to_string());
        }
    }
    Ok(())
}

fn validate_relationships<'a>(
    item: &Value,
    items: &BTreeMap<String, Target>,
    declarations: &BTreeMap<String, ValidatedDeclaration>,
    snippet_keys: impl Iterator<Item = &'a str>,
) -> Result<Vec<Violation>, String> {
    let owner = item["id"].as_str().ok_or("Item has no identity")?;
    let mut issues = Vec::new();
    let mut fail = |dependency, message| {
        issues.push(Violation {
            owner: owner.to_owned(),
            dependency,
            message,
        });
    };
    for (key, value) in item["meta"]
        .as_object()
        .ok_or("Item metadata must be an object")?
    {
        if !value.is_null()
            && let Some(declaration) = declarations.get(key)
            && let Err(message) = declaration.validate(value)
        {
            fail(Dependency::named(NamedKind::Property, key), message);
        }
    }
    // Existing memberships survive workspace archival. Only a newly added pin
    // requires an active workspace; restoring history must retain that policy.
    for pin in item["pins"].as_array().into_iter().flatten() {
        let id = pin.as_str().ok_or("Pins must be item IDs")?;
        if !items
            .get(id)
            .is_some_and(|item| item.kind == ItemType::Workspace)
        {
            fail(
                Dependency::Item(id.to_owned()),
                "Pins must target an existing workspace.".into(),
            );
        }
    }
    if item["meta"]["type"] == "task" {
        let parent = item["content"]["noteId"]
            .as_str()
            .and_then(|id| items.get(id))
            .map(|parent| serde_json::json!({"meta":{"type":parent.kind,"state":parent.state}}));
        if let Err(message) = task::validate_owner(item, parent.as_ref()) {
            fail(
                Dependency::Item(
                    item["content"]["noteId"]
                        .as_str()
                        .ok_or("Task needs an owner")?
                        .to_owned(),
                ),
                message,
            );
        }
    }
    for key in snippet_keys {
        let message = match items.get(&named_items::id("snippet", key)) {
            None => Some(format!("Snippet {key} does not exist.")),
            Some(snippet) if snippet.kind != ItemType::Snippet => {
                Some(format!("Snippet {key} does not reference a snippet item."))
            }
            Some(snippet) if snippet.state != ItemState::Active => {
                Some(format!("Snippet {key} is archived."))
            }
            Some(_) => None,
        };
        if let Some(message) = message {
            fail(Dependency::named(NamedKind::Snippet, key), message);
        }
    }
    for id in publication_targets(item) {
        if !items
            .get(&id)
            .is_some_and(|target| target.kind == ItemType::Page)
        {
            fail(
                Dependency::Item(id),
                "Publication pages must reference existing pages.".into(),
            );
        }
    }
    // Workspace template offers and authored note links
    // can outlive their targets today. Canonical validation checks their shape;
    // export/open preflight keeps reporting unavailable targets after a restore.
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::commands::assemble_item;
    use serde_json::json;

    fn validate(config: &Value, items: &BTreeMap<String, Value>) -> Result<(), String> {
        super::validate(config, items.keys().map(String::as_str), |id| {
            Ok(items[id].clone())
        })
    }

    fn project() -> (Value, BTreeMap<String, Value>) {
        let root = tempfile::tempdir().unwrap();
        let config = config::open_or_create_project(root.path(), "UTC").unwrap();
        let items = item::list_project_item_ids(root.path())
            .unwrap()
            .into_iter()
            .map(|id| {
                let value = crate::store::snapshots::read_project_item(root.path(), &id)
                    .unwrap()
                    .unwrap();
                (id, value)
            })
            .collect();
        (config, items)
    }

    fn page() -> Value {
        assemble_item(&json!({"meta":{"type":"page"},"content":{"document":{"type":"doc","content":[{"type":"paragraph"}]}}}), None, "UTC").unwrap()
    }

    fn insert(items: &mut BTreeMap<String, Value>, item: Value) -> String {
        let id = item["id"].as_str().unwrap().to_owned();
        items.insert(id.clone(), item);
        id
    }

    #[test]
    fn validates_the_complete_candidate_and_preserves_unknown_metadata() {
        let (config, mut items) = project();
        let mut note = page();
        note["meta"]["unknown_field"] = json!({"arbitrary":"extension"});
        let id = insert(&mut items, note);
        validate(&config, &items).unwrap();
        items.get_mut(&id).unwrap()["meta"]["priority"] = json!("not a number");
        assert!(validate(&config, &items).unwrap_err().contains("number"));
        items.get_mut(&id).unwrap()["meta"]["priority"] = json!(7);
        validate(&config, &items).unwrap();
        items.remove(config["defaultWorkspaceId"].as_str().unwrap());
        assert!(
            validate(&config, &items)
                .unwrap_err()
                .contains("active workspace")
        );
    }

    #[test]
    fn active_tasks_require_active_owners_and_archived_tasks_keep_their_owner() {
        let (config, mut items) = project();
        let id = insert(&mut items, page());
        let task = assemble_item(
            &json!({"meta":{"type":"task"},"content":{"noteId":id,"text":"Finish"}}),
            None,
            "UTC",
        )
        .unwrap();
        let task_id = insert(&mut items, task);
        validate(&config, &items).unwrap();
        items.get_mut(&id).unwrap()["meta"]["state"] = json!("archived");
        assert!(validate(&config, &items).is_err());
        items.get_mut(&task_id).unwrap()["meta"]["state"] = json!("archived");
        validate(&config, &items).unwrap();
        items.remove(&id);
        assert!(validate(&config, &items).is_err());
    }

    #[test]
    fn snippet_and_pin_checks_use_overlay_targets_instead_of_live_files() {
        let (config, mut items) = project();
        let snippet_id = named_items::id("snippet", "reference");
        let snippet = assemble_item(&json!({"id":snippet_id,"meta":{"type":"snippet"},"content":{"key":"reference","paragraph":{"type":"paragraph"}}}), None, "UTC").unwrap();
        insert(&mut items, snippet);
        let note = assemble_item(&json!({"meta":{"type":"page"},"pins":[config["defaultWorkspaceId"]],"content":{"document":{"type":"doc","content":[{"type":"embed","attrs":{"snippetId":"reference"}}]}}}), None, "UTC").unwrap();
        let note_id = insert(&mut items, note);
        validate(&config, &items).unwrap();
        items.get_mut(&snippet_id).unwrap()["meta"]["state"] = json!("archived");
        assert!(validate(&config, &items).unwrap_err().contains("archived"));
        items.get_mut(&snippet_id).unwrap()["meta"]["state"] = json!("active");
        items.get_mut(&note_id).unwrap()["pins"] = json!([snippet_id]);
        assert!(validate(&config, &items).unwrap_err().contains("workspace"));
        items.get_mut(&note_id).unwrap()["pins"] = json!([config["defaultWorkspaceId"]]);
        let mut wrong_target = page();
        wrong_target["id"] = json!(snippet_id);
        items.insert(snippet_id, wrong_target);
        assert!(
            validate(&config, &items)
                .unwrap_err()
                .contains("snippet item")
        );
    }
}
