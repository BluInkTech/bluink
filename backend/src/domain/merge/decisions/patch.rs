// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::resolve::ResolveError;
use super::*;
use crate::domain::merge::model::Segment;

fn entry_id(value: &Value) -> Option<&str> {
    value["attrs"]["id"]
        .as_str()
        .or_else(|| value["id"].as_str())
        .or_else(|| value["pageId"].as_str())
}

fn entry_index(values: &[Value], id: &str) -> Result<usize, ResolveError> {
    let matches: Vec<_> = values
        .iter()
        .enumerate()
        .filter(|(_, value)| entry_id(value) == Some(id))
        .map(|(index, _)| index)
        .collect();
    if let [index] = matches.as_slice() {
        Ok(*index)
    } else {
        Err(ResolveError::Stale(
            "Block identity is missing or ambiguous".into(),
        ))
    }
}

fn path_allowed(path: &[Segment]) -> Result<(), ResolveError> {
    if !matches!(path.first(), Some(Segment::Field(key)) if matches!(key.as_str(), "meta" | "content" | "tags" | "pins"))
        || matches!(path, [Segment::Field(meta), Segment::Field(key), ..] if meta == "meta" && key == "type")
        || matches!(path, [Segment::Field(key)] if key == "meta")
        || path.iter().any(|part| {
            matches!(part, Segment::Field(key) if key.starts_with('$') && key != "$body" && key != "$order")
        })
    {
        return Err(ResolveError::Unsupported(
            "Decision path needs a specialized resolution policy".into(),
        ));
    }
    Ok(())
}

pub(super) fn fragment(value: &Value, path: &[Segment]) -> Result<Fragment, ResolveError> {
    path_allowed(path)?;
    let mut value = Some(value);
    for (position, segment) in path.iter().enumerate() {
        let parent =
            value.ok_or_else(|| ResolveError::Stale("Decision parent no longer exists".into()))?;
        value = match segment {
            Segment::Field(key) if key == "$order" && position + 1 == path.len() => {
                return Ok(Fragment::Present(Value::from(order(parent)?)));
            }
            Segment::Field(key) if key == "$body" && position + 1 == path.len() => {
                return Ok(Fragment::Present(
                    crate::domain::merge::document::paragraph_body(parent),
                ));
            }
            Segment::Field(key) => parent.get(key),
            Segment::Entry(id) => {
                let values = parent
                    .as_array()
                    .ok_or_else(|| ResolveError::Stale("Decision container changed".into()))?;
                Some(&values[entry_index(values, id)?])
            }
            Segment::Cell(cell) => Some(guarded_cell(parent, cell)?),
        };
    }
    Ok(Fragment::from(value))
}

pub(super) fn replace(
    value: &mut Value,
    path: &[Segment],
    replacement: &Fragment,
) -> Result<(), ResolveError> {
    path_allowed(path)?;
    patch(value, path, replacement)
}

fn patch(value: &mut Value, path: &[Segment], replacement: &Fragment) -> Result<(), ResolveError> {
    let (segment, rest) = path
        .split_first()
        .ok_or_else(|| ResolveError::Unsupported("Whole item replacement is unsupported".into()))?;
    match segment {
        Segment::Field(key) if key == "$order" && rest.is_empty() => {
            reorder(value, replacement)?;
        }
        Segment::Field(key) if key == "$body" && rest.is_empty() => {
            let Fragment::Present(next) = replacement else {
                return Err(ResolveError::Unsupported(
                    "Cannot remove paragraph body".into(),
                ));
            };
            let labels = value["attrs"].get("labels").cloned();
            if next["type"] != "paragraph" || entry_id(next) != entry_id(value) {
                return Err(ResolveError::Invalid(
                    "Paragraph resolution changed its identity or type".into(),
                ));
            }
            *value = next.clone();
            if let Some(labels) = labels {
                value
                    .get_mut("attrs")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| {
                        ResolveError::Invalid("Paragraph attributes are invalid".into())
                    })?
                    .insert("labels".into(), labels);
            }
        }
        Segment::Field(key) => {
            let fields = value
                .as_object_mut()
                .ok_or_else(|| ResolveError::Stale("Field parent changed".into()))?;
            if rest.is_empty() {
                match replacement {
                    Fragment::Missing => {
                        fields.remove(key);
                    }
                    Fragment::Present(next) => {
                        fields.insert(key.clone(), next.clone());
                    }
                }
            } else {
                patch(
                    fields
                        .get_mut(key)
                        .ok_or_else(|| ResolveError::Stale("Field parent disappeared".into()))?,
                    rest,
                    replacement,
                )?;
            }
        }
        Segment::Entry(id) => {
            let values = value
                .as_array_mut()
                .ok_or_else(|| ResolveError::Stale("Block container changed".into()))?;
            let index = entry_index(values, id)?;
            if rest.is_empty() {
                match replacement {
                    Fragment::Missing => {
                        values.remove(index);
                    }
                    Fragment::Present(next) if entry_id(next) == Some(id) => {
                        values[index] = next.clone();
                    }
                    Fragment::Present(_) => {
                        return Err(ResolveError::Invalid(
                            "Replacement changed block identity".into(),
                        ));
                    }
                }
            } else {
                patch(&mut values[index], rest, replacement)?;
            }
        }
        Segment::Cell(cell) => {
            replace_cell(value, cell, rest, replacement)?;
        }
    }
    Ok(())
}

fn reorder(value: &mut Value, replacement: &Fragment) -> Result<(), ResolveError> {
    let actual = order(value)?;
    let Fragment::Present(replacement) = replacement else {
        return Err(ResolveError::Invalid("Order cannot be absent".into()));
    };
    let selected: Vec<String> = serde_json::from_value(replacement.clone())
        .map_err(|e| ResolveError::Invalid(e.to_string()))?;
    let actual_ids: std::collections::BTreeSet<_> = actual.iter().collect();
    if selected.len() != actual.len()
        || selected.iter().collect::<std::collections::BTreeSet<_>>() != actual_ids
    {
        return Err(ResolveError::Stale(
            "Sequence membership changed; no entries were removed".into(),
        ));
    }
    let values = value
        .as_array_mut()
        .ok_or_else(|| ResolveError::Stale("Sequence container changed".into()))?;
    let mut entries: std::collections::BTreeMap<_, _> =
        actual.into_iter().zip(std::mem::take(values)).collect();
    for id in selected {
        values.push(
            entries
                .remove(&id)
                .ok_or_else(|| ResolveError::Invalid("Sequence entry disappeared".into()))?,
        );
    }
    Ok(())
}

fn guarded_cell<'a>(
    row: &'a Value,
    cell: &crate::domain::merge::model::CellLocation,
) -> Result<&'a Value, ResolveError> {
    if crate::domain::merge::model::cell_structure(row).map_err(ResolveError::Stale)?
        != cell.structure
    {
        return Err(ResolveError::Stale(
            "Table structure changed since this decision".into(),
        ));
    }
    row["content"]
        .as_array()
        .and_then(|cells| cells.get(cell.index))
        .ok_or_else(|| ResolveError::Stale("Table cell disappeared".into()))
}

fn replace_cell(
    value: &mut Value,
    cell: &crate::domain::merge::model::CellLocation,
    rest: &[Segment],
    replacement: &Fragment,
) -> Result<(), ResolveError> {
    guarded_cell(value, cell)?;
    let target = value
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .and_then(|cells| cells.get_mut(cell.index))
        .ok_or_else(|| ResolveError::Stale("Table cell disappeared".into()))?;
    if rest.is_empty() {
        let Fragment::Present(next) = replacement else {
            return Err(ResolveError::Unsupported(
                "Cells cannot be removed without changing the table structure".into(),
            ));
        };
        *target = next.clone();
        Ok(())
    } else {
        patch(target, rest, replacement)
    }
}

fn order(value: &Value) -> Result<Vec<String>, ResolveError> {
    let values = value
        .as_array()
        .ok_or_else(|| ResolveError::Stale("Sequence container changed".into()))?;
    let mut seen = std::collections::BTreeSet::new();
    values
        .iter()
        .map(|entry| {
            let id = entry_id(entry)
                .ok_or_else(|| ResolveError::Stale("Sequence entry has no identity".into()))?;
            if !seen.insert(id) {
                return Err(ResolveError::Stale("Duplicate sequence identity".into()));
            }
            Ok(id.to_owned())
        })
        .collect()
}
