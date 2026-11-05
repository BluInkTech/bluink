// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::resolve::ResolveError;
use super::*;
use crate::domain::merge::{groups, item, model::Segment, validation::Snapshot};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn targets(
    group: &Group,
    current: &dyn Snapshot,
) -> Result<BTreeSet<String>, ResolveError> {
    if groups::members(current, &group.dependencies).map_err(ResolveError::Invalid)?
        != group.members
    {
        return Err(ResolveError::Stale(
            "Dependency membership changed since the decision was formed".into(),
        ));
    }
    let mut targets: BTreeSet<_> = group
        .provisional
        .iter()
        .map(|value| value.field.item.clone())
        .collect();
    for member in &group.members {
        targets.insert(member.dependency.target_id());
        targets.extend(member.owners.iter().cloned());
    }
    Ok(targets)
}

pub(super) fn choose(
    group: &Group,
    selection: Selection,
    current: &dyn Snapshot,
) -> Result<BTreeMap<String, Value>, ResolveError> {
    let selected = group
        .selected(match selection {
            Selection::Provisional => None,
            Selection::First => Some(0),
            Selection::Second => Some(1),
            Selection::Both => {
                return Err(ResolveError::Unsupported(
                    "Keep both is only available for document blocks".into(),
                ));
            }
        })
        .map_err(ResolveError::Invalid)?;
    let mut changes = BTreeMap::new();
    let mut position = 0;
    while position < selected.len() {
        let id = &selected[position].field.item;
        let before = current
            .read(id)
            .map_err(ResolveError::Invalid)?
            .ok_or_else(|| ResolveError::Stale("Dependency target disappeared".into()))?;
        let mut after = before.clone();
        while position < selected.len() && selected[position].field.item == *id {
            let choice = &selected[position];
            let provisional = &group.provisional[position];
            if !field_allowed(group, &choice.field, &before) {
                return Err(ResolveError::Unsupported(
                    "Dependency field is outside its declared repair policy".into(),
                ));
            }
            if choice.field.path.is_empty() {
                whole_declaration(&mut after, &provisional.value, &choice.value)?;
            } else {
                let path: Vec<_> = choice
                    .field
                    .path
                    .iter()
                    .cloned()
                    .map(Segment::Field)
                    .collect();
                if super::patch::fragment(&before, &path)? != provisional.value {
                    return Err(ResolveError::Stale(
                        "A dependency field changed; refresh its preview".into(),
                    ));
                }
                super::patch::replace(&mut after, &path, &choice.value)?;
            }
            position += 1;
        }
        if before != after {
            changes.insert(id.clone(), after);
        }
    }
    Ok(changes)
}

fn field_allowed(group: &Group, field: &groups::Field, item: &Value) -> bool {
    group
        .dependencies
        .iter()
        .any(|dependency| dependency_field_allowed(group, dependency, field, item))
}

fn dependency_field_allowed(
    group: &Group,
    dependency: &crate::domain::project::candidate::Dependency,
    field: &groups::Field,
    item: &Value,
) -> bool {
    use crate::domain::project::candidate::{Dependency, NamedKind};
    use groups::Policy;
    match dependency {
        Dependency::Named { kind, key } => {
            let declaration =
                field.item == dependency.target_id() && item["meta"]["type"] == kind.as_str();
            if group.policies.contains(&Policy::RestoreDeclaration)
                && declaration
                && (field.path.is_empty() || field.path == ["meta", "state"])
            {
                return true;
            }
            if !group.policies.contains(&Policy::PropertyConstraint) || *kind != NamedKind::Property
            {
                return false;
            }
            let owner_value = field.path == ["meta", key.as_str()];
            let schema_field = field.path == ["meta", "state"]
                || matches!(field.path.as_slice(), [content, key] if content == "content" && matches!(key.as_str(), "valueType" | "options" | "optionAppearance"));
            owner_value || (declaration && schema_field)
        }
        Dependency::Item(parent) => {
            if group.policies.contains(&Policy::RestoreTarget)
                && field.item == *parent
                && item["meta"]["type"] == "page"
                && field.path.is_empty()
            {
                return true;
            }
            if !group.policies.contains(&Policy::ArchiveTasks) {
                return false;
            }
            let page_state = field.item == *parent
                && item["meta"]["type"] == "page"
                && field.path == ["meta", "state"];
            let task_field = item["meta"]["type"] == "task"
                && item["content"]["noteId"] == *parent
                && (field.path == ["meta", "state"] || field.path == ["content", "noteId"]);
            page_state || task_field
        }
    }
}

fn whole_declaration(
    current: &mut Value,
    provisional: &Fragment,
    choice: &Fragment,
) -> Result<(), ResolveError> {
    if current["meta"]["type"] != "page"
        && !crate::domain::project::named_items::is_named(
            current["meta"]["type"].as_str().unwrap_or_default(),
        )
    {
        return Err(ResolveError::Unsupported(
            "Whole-item dependency choice must be a declaration".into(),
        ));
    }
    if Fragment::Present(item::semantic(current).map_err(ResolveError::Invalid)?) != *provisional {
        return Err(ResolveError::Stale(
            "Restored declaration changed since the decision".into(),
        ));
    }
    if choice == provisional {
        return Ok(());
    }
    let Fragment::Present(fields) = choice else {
        return Err(ResolveError::Unsupported(
            "Declaration deletion requires resolving its surviving uses; no files were changed"
                .into(),
        ));
    };
    if fields["id"] != current["id"] || fields["meta"]["type"] != current["meta"]["type"] {
        return Err(ResolveError::Invalid(
            "Declaration identity cannot change".into(),
        ));
    }
    let fields = fields
        .as_object()
        .ok_or_else(|| ResolveError::Invalid("Invalid declaration alternative".into()))?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| ResolveError::Invalid("Invalid declaration envelope".into()))?;
    object.retain(|key, _| {
        matches!(
            key.as_str(),
            "revision" | "createdAt" | "modifiedAt" | "analytics"
        )
    });
    object.extend(fields.clone());
    Ok(())
}
