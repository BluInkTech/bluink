// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::domain::project::properties::ValidatedDeclaration;

/// Option removal is a preservation exception to picking one branch's schema:
/// keep options still used by surviving values. Never invent an undeclared
/// option, restore unused removed options, or change the property's value type.
fn restore_options(
    key: &str,
    id: &str,
    snapshots: Snapshots<'_>,
    overlay: &mut Overlay<'_>,
    owners: &BTreeSet<String>,
    unit: &mut Unit,
    prefer_left: bool,
) -> Result<bool, String> {
    let Some(mut current) = read(overlay, id)? else {
        return Ok(false);
    };
    let Some(left) = read(snapshots.left, id)? else {
        return Ok(false);
    };
    let Some(right) = read(snapshots.right, id)? else {
        return Ok(false);
    };
    let value_type = &current["content"]["valueType"];
    if !matches!(value_type.as_str(), Some("select" | "multiSelect"))
        || [&left, &right].iter().any(|value| {
            value["meta"]["state"] != "active" || value["content"]["valueType"] != *value_type
        })
    {
        return Ok(false);
    }
    let options = |value: &Value| -> BTreeSet<String> {
        value["content"]["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let mut allowed = options(&left);
    allowed.extend(options(&right));
    let original = options(&current);
    let mut retained = original.clone();
    let mut fields = BTreeSet::new();
    for owner in owners {
        let Some(item) = read(overlay, owner)? else {
            continue;
        };
        let Some(value) = item["meta"].get(key) else {
            continue;
        };
        let choices = match value {
            Value::String(value) => vec![value.as_str()],
            Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for choice in choices {
            if allowed.contains(choice) {
                retained.insert(choice.to_owned());
            }
        }
        fields.insert(Field::new(owner, &["meta", key]));
    }
    if retained == original {
        return Ok(false);
    }
    // Let normal group selection handle any other incompatibility. The shared
    // declaration validator remains authoritative for type/shape/value checks.
    // Keep the winning order, appending restored used keys in branch order.
    let mut ordered = Vec::new();
    let branches = if prefer_left {
        [&left, &right]
    } else {
        [&right, &left]
    };
    for source in std::iter::once(&current).chain(branches) {
        for option in source["content"]["options"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(key) = option.as_str()
                && retained.contains(key)
                && !ordered.iter().any(|existing: &String| existing == key)
            {
                ordered.push(key.to_owned());
            }
        }
    }
    current["content"]["options"] = json!(ordered);
    let declaration = ValidatedDeclaration::read(&current["content"])?;
    for owner in owners {
        if let Some(item) = read(overlay, owner)?
            && let Some(value) = item["meta"].get(key)
            && declaration.validate(value).is_err()
        {
            return Ok(false);
        }
    }
    unit.fields.extend(fields);
    for key in ["valueType", "options", "optionAppearance"] {
        unit.fields.insert(Field::new(id, &["content", key]));
    }
    unit.fields.insert(Field::new(id, &["meta", "state"]));
    overlay.put(current)?;
    Ok(true)
}

pub(super) fn repair(
    dependency: &Dependency,
    prefer_left: bool,
    snapshots: Snapshots<'_>,
    overlay: &mut Overlay<'_>,
    unit: &mut Unit,
) -> Result<(), String> {
    let Dependency::Named {
        kind: NamedKind::Property,
        key,
    } = dependency
    else {
        return Err("Expected a property constraint".into());
    };
    let id = dependency.target_id();
    let (preferred, other) = if prefer_left {
        (snapshots.left, snapshots.right)
    } else {
        (snapshots.right, snapshots.left)
    };
    // A declaration restored to preserve surviving values must stay available.
    // Prefer the surviving active branch when the normal winner deleted it.
    let selected = if read(preferred, &id)?.is_some_and(|value| value["meta"]["state"] == "active")
    {
        preferred
    } else if read(other, &id)?.is_some_and(|value| value["meta"]["state"] == "active") {
        other
    } else {
        return Err("Neither branch has an active property constraint to restore.".into());
    };
    let owners = owners(dependency, snapshots, overlay)?;
    if restore_options(key, &id, snapshots, overlay, &owners, unit, prefer_left)? {
        return Ok(());
    }
    let definition = read(selected, &id)?.ok_or("Property definition is missing")?;
    let mut current = read(overlay, &id)?.ok_or("Property candidate is missing")?;
    for key in ["valueType", "options", "optionAppearance"] {
        let field = Field::new(&id, &["content", key]);
        assign(&mut current, &field.path, definition["content"].get(key))?;
        unit.fields.insert(field);
    }
    unit.fields.insert(Field::new(&id, &["meta", "state"]));
    overlay.put(current)?;
    for owner in owners {
        let Some(mut current) = read(overlay, &owner)? else {
            continue;
        };
        let selected = read(selected, &owner)?;
        // Include unchanged and newly introduced users, but verify reverse-index
        // candidates rather than treating a false positive as a property owner.
        let mut used = current["meta"].get(key).is_some();
        for source in [snapshots.base, snapshots.left, snapshots.right] {
            used |= read(source, &owner)?.is_some_and(|value| value["meta"].get(key).is_some());
        }
        if !used {
            continue;
        }
        let field = Field::new(&owner, &["meta", key]);
        let selected_value = selected.as_ref().and_then(|value| value["meta"].get(key));
        let changed = current["meta"].get(key) != selected_value;
        assign(&mut current, &field.path, selected_value)?;
        unit.fields.insert(field);
        if changed {
            overlay.put(current)?;
        }
    }
    Ok(())
}
