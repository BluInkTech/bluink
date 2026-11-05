// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Dependency repairs over immutable captures. Returned proposals are not durable
//! conflict items and must not be published without decision/tombstone handling,
//! complete package validation and the accepting transaction's generation checks.

use super::{
    item::{self, ItemCandidate},
    model::{Context, Fragment, Merger, Source, stable_id},
    validation::{self, Report, Snapshot, Snapshots},
};
use crate::domain::project::{
    candidate::{Dependency, NamedKind, dependencies},
    canonical_json,
    item::validate_item,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub(crate) struct Field {
    pub(crate) item: String,
    /// Object keys, never array offsets. Empty means a restored declaration item.
    pub(crate) path: Vec<String>,
}

impl Field {
    fn new(item: &str, path: &[&str]) -> Self {
        Self {
            item: item.to_owned(),
            path: path.iter().map(|key| (*key).to_owned()).collect(),
        }
    }
    fn overlaps(&self, other: &Self) -> bool {
        let shared = self.path.len().min(other.path.len());
        self.item == other.item && self.path[..shared] == other.path[..shared]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct FieldValue {
    pub(crate) field: Field,
    pub(crate) value: Fragment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Policy {
    RestoreDeclaration,
    RestoreTarget,
    PropertyConstraint,
    ArchiveTasks,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Alternative {
    pub(crate) fields: Vec<FieldValue>,
    pub(crate) source: Source,
}

/// Raw branch alternatives include absence. `provisional` is separate because a
/// preservation repair can synthesize a valid combination (new task + archived
/// parent) that neither branch contained. No absent alternative is invented away.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Group {
    pub(crate) id: String,
    pub(crate) target: String,
    pub(crate) base_epoch: String,
    pub(crate) dependencies: BTreeSet<Dependency>,
    pub(crate) policies: BTreeSet<Policy>,
    pub(crate) base: Vec<FieldValue>,
    pub(crate) alternatives: [Alternative; 2],
    pub(crate) provisional: Vec<FieldValue>,
    pub(crate) members: Vec<Members>,
}

/// Actual incoming references in the accepted provisional capture. Extra reverse
/// index candidates are filtered through their captured canonical content.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Members {
    pub(crate) dependency: Dependency,
    pub(crate) owners: BTreeSet<String>,
}

pub(super) fn members(
    source: &dyn Snapshot,
    constraints: &BTreeSet<Dependency>,
) -> Result<Vec<Members>, String> {
    constraints
        .iter()
        .map(|dependency| {
            let mut owners = BTreeSet::new();
            for owner in source.dependents(dependency)? {
                if let Some(value) = read(source, &owner)?
                    && dependencies(&value)?.contains(dependency)
                {
                    owners.insert(owner);
                }
            }
            Ok(Members {
                dependency: dependency.clone(),
                owners,
            })
        })
        .collect()
}

impl Group {
    pub(super) fn selected(&self, index: Option<usize>) -> Result<Vec<FieldValue>, String> {
        let Some(index) = index else {
            return Ok(self.provisional.clone());
        };
        let mut selected = self
            .alternatives
            .get(index)
            .ok_or("Invalid dependency alternative")?
            .fields
            .clone();
        if self.policies.contains(&Policy::ArchiveTasks) {
            let tasks: BTreeSet<_> = self
                .provisional
                .iter()
                .filter_map(|value| {
                    if value.field.path != ["content", "noteId"] {
                        return None;
                    }
                    let Fragment::Present(Value::String(parent)) = &value.value else {
                        return None;
                    };
                    self.dependencies
                        .contains(&Dependency::Item(parent.clone()))
                        .then_some(&value.field.item)
                })
                .collect();
            for (selected, provisional) in selected.iter_mut().zip(&self.provisional) {
                if selected.value == Fragment::Missing
                    && tasks.contains(&selected.field.item)
                    && (selected.field.path == ["meta", "state"]
                        || selected.field.path == ["content", "noteId"])
                {
                    selected.value.clone_from(&provisional.value);
                }
            }
        }
        Ok(selected)
    }

    pub(super) fn identity(&self) -> Result<String, String> {
        stable_id(&(
            "bluink:dependency-decision:v1",
            &self.target,
            &self.base_epoch,
            &self.dependencies,
            &self.policies,
            &self.base,
            &self.alternatives[0].fields,
            &self.alternatives[1].fields,
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RepairPlan {
    pub(crate) items: BTreeMap<String, ItemCandidate>,
    pub(crate) groups: Vec<Group>,
    pub(crate) checked: Report,
}

#[derive(Default)]
struct Unit {
    dependencies: BTreeSet<Dependency>,
    policies: BTreeSet<Policy>,
    fields: BTreeSet<Field>,
}

impl Unit {
    /// Both sets are ordered by item then path. Walk them together instead of
    /// comparing every pair of fields in a property with thousands of users.
    fn overlaps(&self, other: &Self) -> bool {
        let mut left = self.fields.iter();
        let mut right = other.fields.iter();
        let (mut a, mut b) = (left.next(), right.next());
        while let (Some(first), Some(second)) = (a, b) {
            if first.overlaps(second) {
                return true;
            }
            if first < second {
                a = left.next();
            } else {
                b = right.next();
            }
        }
        false
    }

    fn remove_covered_fields(&mut self) {
        // A prefix sorts before its descendants, so the last retained field is
        // sufficient to recognize a restored whole item covering child fields.
        let mut roots: Vec<Field> = Vec::new();
        for field in std::mem::take(&mut self.fields) {
            if roots.last().is_none_or(|root| !root.overlaps(&field)) {
                roots.push(field);
            }
        }
        self.fields.extend(roots);
    }
}

/// Overridden items update reverse references as well as reads. Delegating only
/// reads would let subsequent repairs validate against stale relationship data.
pub(crate) struct Overlay<'a> {
    source: &'a dyn Snapshot,
    items: BTreeMap<String, Value>,
    deleted: BTreeSet<String>,
    incoming: BTreeMap<Dependency, BTreeSet<String>>,
}

impl<'a> Overlay<'a> {
    pub(crate) fn new(source: &'a dyn Snapshot) -> Self {
        Self {
            source,
            items: BTreeMap::new(),
            deleted: BTreeSet::new(),
            incoming: BTreeMap::new(),
        }
    }
    pub(crate) fn put(&mut self, value: Value) -> Result<(), String> {
        let id = value["id"]
            .as_str()
            .ok_or("Repair item needs an identity")?
            .to_owned();
        validate_item(&value, &id)?;
        self.deleted.remove(&id);
        if let Some(previous) = self.items.get(&id) {
            for dependency in dependencies(previous)? {
                if let Some(owners) = self.incoming.get_mut(&dependency) {
                    owners.remove(&id);
                }
            }
        }
        for dependency in dependencies(&value)? {
            self.incoming
                .entry(dependency)
                .or_default()
                .insert(id.clone());
        }
        self.items.insert(id, value);
        Ok(())
    }
    pub(crate) fn remove(&mut self, id: &str) -> Result<(), String> {
        crate::domain::project::identity::ItemId::try_from(id)?;
        self.items.remove(id);
        for owners in self.incoming.values_mut() {
            owners.remove(id);
        }
        self.deleted.insert(id.to_owned());
        Ok(())
    }
}
impl Snapshot for Overlay<'_> {
    fn read(&self, id: &str) -> Result<Option<Value>, String> {
        if self.deleted.contains(id) {
            return Ok(None);
        }
        self.items
            .get(id)
            .map_or_else(|| self.source.read(id), |value| Ok(Some(value.clone())))
    }
    fn dependents(&self, dependency: &Dependency) -> Result<BTreeSet<String>, String> {
        let mut result = self.source.dependents(dependency)?;
        result.retain(|id| !self.items.contains_key(id) && !self.deleted.contains(id));
        if let Some(owners) = self.incoming.get(dependency) {
            result.extend(owners.iter().cloned());
        }
        Ok(result)
    }
}

/// Repair only the policies explicitly supported here. Every iteration expands
/// through captured reverse references and revalidates. A repeated unresolved
/// policy, unsupported violation or invalid item returns no partial plan.
pub(crate) fn repair(
    context: &Context,
    config: &Value,
    changed: &BTreeSet<String>,
    snapshots: Snapshots<'_>,
) -> Result<RepairPlan, String> {
    let prefer_left = Merger::new(context)?.prefer_left;
    let mut overlay = Overlay::new(snapshots.candidate);
    let mut changed = changed.clone();
    let mut units = BTreeMap::new();
    let checked = loop {
        let report = validation::validate_affected(
            config,
            &changed,
            Snapshots {
                candidate: &overlay,
                ..snapshots
            },
        )?;
        if report.violations.is_empty() {
            break report;
        }
        let mut pending = BTreeSet::new();
        for issue in &report.violations {
            let policy = policy(&issue.dependency, &issue.owner, &overlay)?;
            let key = (issue.dependency.clone(), policy);
            if !units.contains_key(&key) {
                pending.insert(key);
            }
        }
        if pending.is_empty() {
            return Err(
                "Dependency repairs did not produce a valid candidate; preserve both inputs."
                    .into(),
            );
        }
        for (dependency, policy) in pending {
            let mut unit = Unit::default();
            match policy {
                Policy::RestoreDeclaration => restore_declaration(
                    &dependency,
                    prefer_left,
                    snapshots,
                    &mut overlay,
                    &mut unit,
                )?,
                Policy::RestoreTarget => {
                    restore_target(&dependency, prefer_left, snapshots, &mut overlay, &mut unit)?;
                }
                Policy::PropertyConstraint => {
                    property::repair(&dependency, prefer_left, snapshots, &mut overlay, &mut unit)?;
                }
                Policy::ArchiveTasks => {
                    archive_tasks(&dependency, snapshots, &mut overlay, &mut unit)?;
                }
            }
            unit.dependencies.insert(dependency.clone());
            unit.policies.insert(policy);
            changed.extend(unit.fields.iter().map(|field| field.item.clone()));
            units.insert((dependency, policy), unit);
        }
    };
    let groups = combine(units.into_values().collect())
        .into_iter()
        .map(|unit| describe(context, &unit, snapshots, &overlay))
        .collect::<Result<_, _>>()?;
    let items = classify(context, snapshots, &overlay)?;
    Ok(RepairPlan {
        items,
        groups,
        checked,
    })
}

fn policy(
    dependency: &Dependency,
    owner: &str,
    candidate: &dyn Snapshot,
) -> Result<Policy, String> {
    let target = read(candidate, &dependency.target_id())?;
    match dependency {
        Dependency::Item(_)
            if target.is_none()
                && read(candidate, owner)?
                    .is_some_and(|value| value["meta"]["type"] == "publication") =>
        {
            Ok(Policy::RestoreTarget)
        }
        Dependency::Named { .. }
            if target
                .as_ref()
                .is_none_or(|value| value["meta"]["state"] != "active") =>
        {
            Ok(Policy::RestoreDeclaration)
        }
        Dependency::Named {
            kind: NamedKind::Property,
            ..
        } => Ok(Policy::PropertyConstraint),
        Dependency::Item(_)
            if target.as_ref().is_some_and(|value| {
                value["meta"]["type"] == "page" && value["meta"]["state"] == "archived"
            }) && read(candidate, owner)?
                .is_some_and(|value| value["meta"]["type"] == "task") =>
        {
            Ok(Policy::ArchiveTasks)
        }
        _ => Err(format!(
            "Unsupported dependency repair for {owner}: {dependency:?}; preserve both inputs."
        )),
    }
}

fn read(source: &dyn Snapshot, id: &str) -> Result<Option<Value>, String> {
    let value = source.read(id)?;
    if let Some(value) = &value {
        validate_item(value, id)?;
    }
    Ok(value)
}

fn owners(
    dependency: &Dependency,
    snapshots: Snapshots<'_>,
    candidate: &dyn Snapshot,
) -> Result<BTreeSet<String>, String> {
    let mut result = BTreeSet::new();
    for source in [snapshots.base, snapshots.left, snapshots.right, candidate] {
        result.extend(source.dependents(dependency)?);
    }
    Ok(result)
}

fn restore_declaration(
    dependency: &Dependency,
    prefer_left: bool,
    snapshots: Snapshots<'_>,
    overlay: &mut Overlay<'_>,
    unit: &mut Unit,
) -> Result<(), String> {
    let Dependency::Named { kind, .. } = dependency else {
        return Err("Expected a named declaration".into());
    };
    let id = dependency.target_id();
    let existing = read(overlay, &id)?;
    let mut restored = if let Some(value) = existing.clone() {
        value
    } else {
        let (first, second) = if prefer_left {
            (snapshots.left, snapshots.right)
        } else {
            (snapshots.right, snapshots.left)
        };
        let mut candidates = [
            read(first, &id)?,
            read(second, &id)?,
            read(snapshots.base, &id)?,
        ];
        let index = candidates[..2]
            .iter()
            .position(|value| {
                value
                    .as_ref()
                    .is_some_and(|value| value["meta"]["state"] == "active")
            })
            .or_else(|| candidates.iter().position(Option::is_some))
            .ok_or("No declaration survives to restore")?;
        candidates[index]
            .take()
            .ok_or("Missing restoration source")?
    };
    if restored["meta"]["type"] != kind.as_str() {
        return Err("A declaration changed type; preserve both inputs.".into());
    }
    restored["meta"]["state"] = json!("active");
    unit.fields.insert(Field::new(
        &id,
        if existing.is_some() {
            &["meta", "state"]
        } else {
            &[]
        },
    ));
    overlay.put(restored)
}

fn restore_target(
    dependency: &Dependency,
    prefer_left: bool,
    snapshots: Snapshots<'_>,
    overlay: &mut Overlay<'_>,
    unit: &mut Unit,
) -> Result<(), String> {
    let id = dependency.target_id();
    let sources = if prefer_left {
        [snapshots.left, snapshots.right, snapshots.base]
    } else {
        [snapshots.right, snapshots.left, snapshots.base]
    };
    for source in sources {
        if let Some(page) = read(source, &id)?
            && page["meta"]["type"] == "page"
        {
            unit.fields.insert(Field::new(&id, &[]));
            return overlay.put(page);
        }
    }
    Err("No publication page survives to restore; preserve both inputs.".into())
}

fn archive_tasks(
    dependency: &Dependency,
    snapshots: Snapshots<'_>,
    overlay: &mut Overlay<'_>,
    unit: &mut Unit,
) -> Result<(), String> {
    let id = dependency.target_id();
    unit.fields.insert(Field::new(&id, &["meta", "state"]));
    for owner in owners(dependency, snapshots, overlay)? {
        let Some(mut task) = read(overlay, &owner)? else {
            continue;
        };
        if task["meta"]["type"] != "task" || task["content"]["noteId"] != id {
            continue;
        }
        unit.fields.insert(Field::new(&owner, &["meta", "state"]));
        unit.fields
            .insert(Field::new(&owner, &["content", "noteId"]));
        if task["meta"]["state"] != "archived" {
            task["meta"]["state"] = json!("archived");
            overlay.put(task)?;
        }
    }
    Ok(())
}

fn assign(item: &mut Value, path: &[String], value: Option<&Value>) -> Result<(), String> {
    let (last, parents) = path
        .split_last()
        .ok_or("Field repair cannot replace a whole item")?;
    let mut owner = item;
    for key in parents {
        owner = owner.get_mut(key).ok_or("Repair field parent is missing")?;
    }
    let owner = owner
        .as_object_mut()
        .ok_or("Repair field parent is not an object")?;
    if let Some(value) = value {
        owner.insert(last.clone(), value.clone());
    } else {
        owner.remove(last);
    }
    Ok(())
}

fn combine(mut units: Vec<Unit>) -> Vec<Unit> {
    let mut groups: Vec<Unit> = Vec::new();
    while let Some(mut unit) = units.pop() {
        while let Some(index) = groups.iter().position(|other| unit.overlaps(other)) {
            let other = groups.remove(index);
            unit.dependencies.extend(other.dependencies);
            unit.policies.extend(other.policies);
            unit.fields.extend(other.fields);
        }
        unit.remove_covered_fields();
        groups.push(unit);
    }
    groups.sort_by(|left, right| left.fields.cmp(&right.fields));
    groups
}

pub(super) fn patches(
    source: &dyn Snapshot,
    fields: &BTreeSet<Field>,
) -> Result<Vec<FieldValue>, String> {
    let mut result = Vec::new();
    let mut current_id = String::new();
    let mut current = None;
    for field in fields {
        if current_id != field.item {
            current = read(source, &field.item)?;
            current_id.clone_from(&field.item);
        }
        let value = if field.path.is_empty() {
            current.as_ref().map(item::semantic).transpose()?
        } else {
            let mut value = current.as_ref();
            for key in &field.path {
                value = value.and_then(|value| value.get(key));
            }
            value.cloned()
        };
        result.push(FieldValue {
            field: field.clone(),
            value: Fragment::from(value.as_ref()),
        });
    }
    Ok(result)
}

fn describe(
    context: &Context,
    unit: &Unit,
    snapshots: Snapshots<'_>,
    candidate: &dyn Snapshot,
) -> Result<Group, String> {
    let base = patches(snapshots.base, &unit.fields)?;
    let mut alternatives = [
        Alternative {
            fields: patches(snapshots.left, &unit.fields)?,
            source: context.left.clone(),
        },
        Alternative {
            fields: patches(snapshots.right, &unit.fields)?,
            source: context.right.clone(),
        },
    ];
    // Provenance breaks ties only when the field values are identical. It does
    // not enter the decision identity, which describes the semantic conflict.
    let order_key = |alternative: &Alternative| {
        canonical_json::to_vec(&(&alternative.fields, &alternative.source))
            .map_err(|error| error.to_string())
    };
    if order_key(&alternatives[0])? > order_key(&alternatives[1])? {
        alternatives.swap(0, 1);
    }
    let mut group = Group {
        id: String::new(),
        target: context.target.clone(),
        base_epoch: context.base_epoch.clone(),
        dependencies: unit.dependencies.clone(),
        policies: unit.policies.clone(),
        base,
        alternatives,
        provisional: patches(candidate, &unit.fields)?,
        members: members(candidate, &unit.dependencies)?,
    };
    group.id = group.identity()?;
    Ok(group)
}

fn classify(
    context: &Context,
    snapshots: Snapshots<'_>,
    overlay: &Overlay<'_>,
) -> Result<BTreeMap<String, ItemCandidate>, String> {
    let mut result = BTreeMap::new();
    for (id, value) in &overlay.items {
        let fields = item::semantic(value)?;
        if read(snapshots.candidate, id)?
            .as_ref()
            .map(item::semantic)
            .transpose()?
            .as_ref()
            == Some(&fields)
        {
            continue;
        }
        let base = read(snapshots.base, id)?;
        let left = read(snapshots.left, id)?;
        let right = read(snapshots.right, id)?;
        let b = base.as_ref().map(item::semantic).transpose()?;
        let l = left.as_ref().map(item::semantic).transpose()?;
        let r = right.as_ref().map(item::semantic).transpose()?;
        let context = Context {
            target: id.clone(),
            ..context.clone()
        };
        let merger = Merger::new(&context)?;
        let candidate = item::choose_candidate(
            &merger,
            Some(fields),
            [
                (left.as_ref(), l.as_ref()),
                (right.as_ref(), r.as_ref()),
                (base.as_ref(), b.as_ref()),
            ],
        )?;
        result.insert(id.clone(), candidate);
    }
    Ok(result)
}

mod property;

#[cfg(test)]
mod tests;
