// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Discover and validate affected item relationships in immutable snapshots.
//! This is a candidate check, not publication authorization: dependency repairs,
//! decision tombstones, theme packages and transaction generation checks belong
//! to the accepting coordinator. No query refresh, file scan or write occurs here.

use crate::domain::project::{
    candidate::{Catalogue, Dependency, NamedKind, Violation, dependencies},
    config,
    identity::ItemId,
    item,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// All reads and reverse lookups must describe the SAME immutable snapshot.
/// Reverse lookup may return extra candidates, but must include every use,
/// including archived owners. A missing/incomplete index is an error, never an
/// empty result or permission to fall back to scanning canonical files.
pub(crate) trait Snapshot {
    fn read(&self, id: &str) -> Result<Option<Value>, String>;
    fn dependents(&self, dependency: &Dependency) -> Result<BTreeSet<String>, String>;
}

#[derive(Clone, Copy)]
pub(crate) struct Snapshots<'a> {
    pub(crate) base: &'a dyn Snapshot,
    pub(crate) left: &'a dyn Snapshot,
    pub(crate) right: &'a dyn Snapshot,
    pub(crate) candidate: &'a dyn Snapshot,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) affected: BTreeSet<String>,
    pub(crate) constraints: BTreeSet<Dependency>,
    pub(crate) violations: Vec<Violation>,
}

const BASE: usize = 0;
const CANDIDATE: usize = 3;

struct Capture<'a> {
    sources: [&'a dyn Snapshot; 4],
    records: [BTreeMap<String, Option<Value>>; 4],
    catalogue: Catalogue,
    references: BTreeMap<String, BTreeSet<Dependency>>,
}

impl<'a> Capture<'a> {
    fn new(snapshots: Snapshots<'a>) -> Self {
        Self {
            sources: [
                snapshots.base,
                snapshots.left,
                snapshots.right,
                snapshots.candidate,
            ],
            records: std::array::from_fn(|_| BTreeMap::new()),
            catalogue: Catalogue::default(),
            references: BTreeMap::new(),
        }
    }

    fn read(&mut self, source: usize, id: &str) -> Result<Option<Value>, String> {
        ItemId::try_from(id)?;
        if let Some(value) = self.records[source].get(id) {
            return Ok(value.clone());
        }
        let value = self.sources[source].read(id)?;
        if let Some(value) = &value {
            if source == CANDIDATE {
                self.catalogue.observe(id, value)?;
                self.references.insert(id.to_owned(), dependencies(value)?);
            } else {
                item::validate_item(value, id)?;
            }
        }
        let value = value.as_ref().map(summary);
        self.records[source].insert(id.to_owned(), value.clone());
        Ok(value)
    }

    fn discover(&mut self, changed: &BTreeSet<String>) -> Result<Report, String> {
        let mut pending = changed.clone();
        let mut report = Report {
            affected: BTreeSet::new(),
            constraints: BTreeSet::new(),
            violations: Vec::new(),
        };
        while let Some(id) = pending.pop_first() {
            if !report.affected.insert(id.clone()) {
                continue;
            }
            let records = (0..self.sources.len())
                .map(|side| self.read(side, &id))
                .collect::<Result<Vec<_>, _>>()?;
            for dependency in changed_constraints(&id, &records) {
                if !report.constraints.insert(dependency.clone()) {
                    continue;
                }
                for source in self.sources {
                    let candidates = source.dependents(&dependency)?;
                    for candidate in candidates {
                        ItemId::try_from(candidate.as_str())?;
                        if !report.affected.contains(&candidate) {
                            pending.insert(candidate);
                        }
                    }
                }
            }
        }
        Ok(report)
    }
}

/// `changed` is the complete item-ID union from the captured base/branch/candidate
/// differences, including deletions and any subsequent repairs. It is supplied by
/// the source adapter, never by a browser or the current live index. Unchanged
/// snapshots must already be valid; this incremental check is not a project audit.
pub(crate) fn validate_affected(
    config: &Value,
    changed: &BTreeSet<String>,
    snapshots: Snapshots<'_>,
) -> Result<Report, String> {
    // Reject an invalid configuration before asking any snapshot for records.
    config::parse_project_config(&serde_json::to_vec(config).map_err(|error| error.to_string())?)?;
    let mut capture = Capture::new(snapshots);
    let mut report = capture.discover(changed)?;
    let mut required = report.affected.clone();
    required.insert(
        config["defaultWorkspaceId"]
            .as_str()
            .ok_or("Project has no default workspace")?
            .to_owned(),
    );
    for id in &report.affected {
        capture.read(CANDIDATE, id)?;
        if let Some(outgoing) = capture.references.get(id) {
            required.extend(outgoing.iter().map(Dependency::target_id));
        }
    }
    for id in required {
        capture.read(CANDIDATE, &id)?;
    }
    if let Some(issue) = capture.catalogue.default_workspace(config)? {
        report.violations.push(issue);
    }
    for id in &report.affected {
        if let Some(value) = capture.read(CANDIDATE, id)? {
            let references = capture
                .references
                .get(id)
                .ok_or("Captured item references are missing")?;
            report
                .violations
                .extend(capture.catalogue.check_captured(&value, references)?);
            check_new_pins(&mut capture, &value, &mut report.violations)?;
        }
    }
    check_removed_declarations(&mut capture, &mut report)?;
    report.violations.sort();
    report
        .violations
        .dedup_by(|a, b| a.owner == b.owner && a.dependency == b.dependency);
    Ok(report)
}

/// Only a constraint change fans out to its users. Editing a paragraph, a tag's
/// colour or a snippet's text must not expand to every owner of its dependencies.
fn changed_constraints(id: &str, records: &[Option<Value>]) -> BTreeSet<Dependency> {
    let mut result = BTreeSet::new();
    for value in records.iter().flatten() {
        let kind = value["meta"]["type"].as_str().unwrap_or_default();
        let dependency = if let Some(kind) = NamedKind::parse(kind) {
            Dependency::named(kind, value["content"]["key"].as_str().unwrap_or_default())
        } else if matches!(kind, "page" | "workspace") {
            Dependency::Item(id.to_owned())
        } else {
            continue;
        };
        let base = records
            .first()
            .and_then(Option::as_ref)
            .map(|value| signature(&dependency, value));
        if records
            .iter()
            .skip(1)
            .any(|value| value.as_ref().map(|value| signature(&dependency, value)) != base)
        {
            result.insert(dependency);
        }
    }
    result
}

fn signature(dependency: &Dependency, value: &Value) -> Value {
    let mut signature = json!({"type":value["meta"]["type"],"state":value["meta"]["state"]});
    if matches!(
        dependency,
        Dependency::Named {
            kind: NamedKind::Property,
            ..
        }
    ) {
        signature["valueType"] = value["content"]["valueType"].clone();
        signature["options"] = value["content"]
            .get("options")
            .cloned()
            .unwrap_or_else(|| json!([]));
    }
    signature
}

fn check_removed_declarations(
    capture: &mut Capture<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let mut unavailable = BTreeSet::new();
    for dependency in &report.constraints {
        let Dependency::Named { kind, key } = dependency else {
            continue;
        };
        let target = dependency.target_id();
        if capture.read(CANDIDATE, &target)?.is_some_and(|value| {
            value["meta"]["type"] == kind.as_str() && value["meta"]["state"] == "active"
        }) {
            continue;
        }
        if *kind == NamedKind::Property && config::is_required_builtin_property(key) {
            report.violations.push(Violation {
                owner: target,
                dependency: dependency.clone(),
                message: format!("Built-in property {key} is required and cannot be deleted."),
            });
        }
        unavailable.insert(dependency.clone());
    }
    // Visit each affected owner's actual references, not every owner for every
    // changed declaration. Large independent groups must not form a cross product.
    for id in &report.affected {
        let Some(value) = capture.read(CANDIDATE, id)? else {
            continue;
        };
        let outgoing = capture
            .references
            .get(id)
            .ok_or("Captured item references are missing")?;
        for dependency in outgoing.intersection(&unavailable) {
            // Membership is already in the captured forward references. Reuse
            // the canonical usage scope without projecting the document again
            // for every removed label or snippet.
            if let Dependency::Named { kind, key } = dependency
                && (kind.protect_archived_owners() || value["meta"]["state"] == "active")
            {
                report.violations.push(Violation {
                    owner: id.clone(),
                    dependency: dependency.clone(),
                    message: format!(
                        "The {} {key} is still used and needs an active declaration.",
                        kind.as_str()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Retain the data needed for constraints and owner checks, never page prose,
/// snippet paragraphs, workspace layouts or derived analytics from every side.
fn summary(value: &Value) -> Value {
    let kind = value["meta"]["type"].as_str().unwrap_or_default();
    let content = match kind {
        "property" | "publication" => value["content"].clone(),
        "tag" | "label" | "snippet" => json!({"key":value["content"]["key"]}),
        "task" => json!({"noteId":value["content"]["noteId"]}),
        _ => Value::Null,
    };
    json!({"id":value["id"],"meta":value["meta"],"pins":value["pins"],"content":content})
}

fn check_new_pins(
    capture: &mut Capture<'_>,
    value: &Value,
    issues: &mut Vec<Violation>,
) -> Result<(), String> {
    let id = value["id"].as_str().ok_or("Item has no identity")?;
    let base = capture.read(BASE, id)?;
    for pin in value["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if has_pin(base.as_ref(), pin) {
            continue;
        }
        let target = capture.read(CANDIDATE, pin)?;
        if target.as_ref().is_some_and(|value| {
            value["meta"]["type"] == "workspace" && value["meta"]["state"] == "active"
        }) {
            continue;
        }
        // A valid branch may have added a pin and subsequently archived its
        // workspace. Preserve that committed membership; only a newly combined
        // pin/archival pair needs a decision. Snapshots cannot reveal edit order.
        let mut inherited = false;
        if target
            .as_ref()
            .is_some_and(|value| value["meta"]["type"] == "workspace")
        {
            for branch in 1..CANDIDATE {
                if has_pin(capture.read(branch, id)?.as_ref(), pin)
                    && capture.read(branch, pin)?.is_some_and(|value| {
                        value["meta"]["type"] == "workspace" && value["meta"]["state"] == "archived"
                    })
                {
                    inherited = true;
                    break;
                }
            }
        }
        if inherited {
            continue;
        }
        issues.push(Violation {
            owner: id.to_owned(),
            dependency: Dependency::Item(pin.to_owned()),
            message: "New pins can only target an active workspace.".into(),
        });
    }
    Ok(())
}

fn has_pin(value: Option<&Value>, pin: &str) -> bool {
    value
        .and_then(|value| value["pins"].as_array())
        .is_some_and(|pins| pins.iter().any(|value| value == pin))
}

#[cfg(test)]
mod tests;
