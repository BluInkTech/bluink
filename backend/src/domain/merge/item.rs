// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    model::{Context, Decision, DecisionKind, Location, Merger},
    value::{Agreement, set, unchanged},
};
use crate::domain::project::item::validate_item;
use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ItemCandidate {
    Deleted,
    /// Reuse the corresponding input bytes; no new revision or timestamps.
    Existing(Value),
    /// Semantic fields only: a future accepting transaction assigns the envelope
    /// and derived analytics. This value is intentionally not a writable item.
    Changed {
        fields: Value,
        created_at: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ItemMerge {
    pub(crate) candidate: ItemCandidate,
    pub(crate) decisions: Vec<Decision>,
    pub(crate) followups: Vec<super::decisions::Proposal>,
}

/// Assign an envelope only to a semantic change, retaining exact input records
/// when the merge selected one. This does not write or publish anything.
pub(crate) fn materialize(
    candidate: ItemCandidate,
    before: Option<&Value>,
    timezone: &str,
    values: &mut crate::domain::project::assemble::AuthoringValues<'_>,
) -> Result<Option<Value>, String> {
    match candidate {
        ItemCandidate::Deleted => Ok(None),
        ItemCandidate::Existing(value) => Ok(Some(value)),
        ItemCandidate::Changed { fields, created_at } => {
            let mut value =
                crate::domain::project::assemble::assemble_item(&fields, before, timezone, values)?;
            value["meta"] = fields["meta"].clone();
            value["createdAt"] = Value::String(created_at);
            for key in ["tags", "pins"] {
                if fields.get(key).is_none() {
                    value
                        .as_object_mut()
                        .ok_or("Invalid merged envelope")?
                        .remove(key);
                }
            }
            validate_item(
                &value,
                value["id"].as_str().ok_or("Merged item has no identity")?,
            )?;
            Ok(Some(value))
        }
    }
}

/// Inputs are canonical snapshots supplied by a trusted caller, never browser
/// base content. Validation here is structural, not cross-item integrity: the
/// caller must validate the complete project candidate through an overlay.
pub(crate) fn merge_item(
    context: &Context,
    base: Option<&Value>,
    left: Option<&Value>,
    right: Option<&Value>,
) -> Result<ItemMerge, String> {
    let mut merge = Merger::new(context)?;
    for item in [base, left, right].into_iter().flatten() {
        validate_item(item, &context.target)?;
    }
    if [base, left, right]
        .into_iter()
        .flatten()
        .any(|item| item["meta"]["type"] == "conflict")
    {
        let (record, followups) = super::decisions::merge_records(context, base, left, right)?;
        return Ok(ItemMerge {
            candidate: ItemCandidate::Existing(record),
            decisions: Vec::new(),
            followups,
        });
    }
    let b = base.map(semantic).transpose()?;
    let l = left.map(semantic).transpose()?;
    let r = right.map(semantic).transpose()?;
    let path = Location::default();
    let fields = if let Agreement::Chosen(value) = unchanged(b.as_ref(), l.as_ref(), r.as_ref()) {
        value.cloned()
    } else if let (Some(l), Some(r)) = (&l, &r) {
        if l["meta"]["type"] != r["meta"]["type"]
            || b.as_ref()
                .is_some_and(|b| b["meta"]["type"] != l["meta"]["type"])
        {
            return Err("Item type changes require a dependency decision before merging.".into());
        }
        Some(merge.item_fields(&path, b.as_ref(), l, r)?)
    } else {
        merge.conflict(
            &path,
            DecisionKind::DeleteEdit,
            b.as_ref(),
            l.as_ref(),
            r.as_ref(),
        )?
    };
    let candidate = choose_candidate(
        &merge,
        fields,
        [(left, l.as_ref()), (right, r.as_ref()), (base, b.as_ref())],
    )?;
    Ok(ItemMerge {
        candidate,
        decisions: merge.decisions,
        followups: Vec::new(),
    })
}

pub(super) fn choose_candidate(
    merge: &Merger<'_>,
    fields: Option<Value>,
    mut inputs: [(Option<&Value>, Option<&Value>); 3],
) -> Result<ItemCandidate, String> {
    let Some(fields) = fields else {
        return Ok(ItemCandidate::Deleted);
    };
    if !merge.prefer_left {
        inputs.swap(0, 1);
    }
    for (original, semantic) in inputs {
        if semantic == Some(&fields)
            && let Some(original) = original
        {
            return Ok(ItemCandidate::Existing(original.clone()));
        }
    }
    let template = inputs[0]
        .0
        .or(inputs[1].0)
        .ok_or("Merged item has no surviving envelope")?;
    let mut candidate = template.clone();
    let object = candidate.as_object_mut().ok_or("Item must be an object")?;
    object.retain(|key, _| VOLATILE.contains(&key.as_str()));
    object.extend(
        fields
            .as_object()
            .ok_or("Merged item must be an object")?
            .clone(),
    );
    validate_item(&candidate, &merge.context.target)
        .map_err(|error| format!("Merged candidate is invalid; preserve both inputs: {error}"))?;
    let created_at = inputs[2].0.unwrap_or(template)["createdAt"]
        .as_str()
        .ok_or("Missing creation time")?
        .to_owned();
    Ok(ItemCandidate::Changed { fields, created_at })
}

const VOLATILE: &[&str] = &["revision", "createdAt", "modifiedAt", "analytics"];

pub(crate) fn semantic(item: &Value) -> Result<Value, String> {
    let mut item = item.clone();
    let object = item.as_object_mut().ok_or("Item must be an object")?;
    for key in VOLATILE {
        object.remove(*key);
    }
    normalize_set(&mut item, "tags")?;
    normalize_set(&mut item, "pins")?;
    match item["meta"]["type"].as_str() {
        Some("page" | "template") => {
            normalize_set(&mut item["content"], "ignoredFindings")?;
            normalize_labels(&mut item["content"]["document"])?;
        }
        Some("snippet") => normalize_labels(&mut item["content"]["paragraph"])?,
        _ => {}
    }
    Ok(item)
}

fn normalize_set(value: &mut Value, key: &str) -> Result<(), String> {
    if let Some(existing) = value.get(key) {
        let result = set(None, Some(existing), None)?;
        let object = value.as_object_mut().ok_or("Set owner must be an object")?;
        if let Some(result) = result {
            object.insert(key.to_owned(), result);
        } else {
            object.remove(key);
        }
    }
    Ok(())
}

fn normalize_labels(node: &mut Value) -> Result<(), String> {
    if node["type"] == "paragraph"
        && let Some(attrs) = node.get_mut("attrs")
    {
        normalize_set(attrs, "labels")?;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            normalize_labels(child)?;
        }
    }
    Ok(())
}

impl Merger<'_> {
    fn item_fields(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        let kind = left["meta"]["type"].as_str().ok_or("Item has no type")?;
        self.object(path, base, left, right, |merge, child, key, b, l, r| {
            if matches!(key, "tags" | "pins") {
                return set(b, l, r);
            }
            if let Agreement::Chosen(value) = unchanged(b, l, r) {
                return Ok(value.cloned());
            }
            match (key, l, r) {
                ("meta", Some(l), Some(r)) => merge.fields(child, b, l, r).map(Some),
                ("content", Some(l), Some(r)) => merge.content(kind, child, b, l, r).map(Some),
                _ => merge.atomic(child, b, l, r),
            }
        })
    }

    fn content(
        &mut self,
        kind: &str,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        self.object(path, base, left, right, |merge, child, key, b, l, r| {
            if matches!((kind, key), ("page" | "template", "ignoredFindings")) {
                return set(b, l, r);
            }
            if let Agreement::Chosen(value) = unchanged(b, l, r) {
                return Ok(value.cloned());
            }
            match (kind, key, l, r) {
                ("property", "options", Some(l), Some(r)) => merge
                    .sequence(child, b, l, r, Value::as_str, |_, _, _, left, _| {
                        Ok(left.clone())
                    })
                    .map(Some),
                ("page" | "template", "document", Some(l), Some(r))
                | ("snippet", "paragraph", Some(l), Some(r)) => {
                    merge.document(child, b, l, r).map(Some)
                }
                ("workspace", "widgets", Some(l), Some(r)) => merge
                    .sequence(child, b, l, r, |v| v.get("id")?.as_str(), Self::fields)
                    .map(Some),
                ("publication", "outline", Some(l), Some(r)) => merge
                    .sequence(child, b, l, r, outline_id, Self::outline_node)
                    .map(Some),
                _ => merge.atomic(child, b, l, r),
            }
        })
    }

    fn outline_node(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        if left["kind"] != right["kind"] {
            return self
                .atomic(path, base, Some(left), Some(right))?
                .ok_or_else(|| "Outline entry disappeared".into());
        }
        self.object(path, base, left, right, |merge, child, key, b, l, r| {
            if let Agreement::Chosen(value) = unchanged(b, l, r) {
                return Ok(value.cloned());
            }
            match (key, l, r) {
                ("children", Some(l), Some(r)) => merge
                    .sequence(child, b, l, r, outline_id, Self::outline_node)
                    .map(Some),
                _ => merge.atomic(child, b, l, r),
            }
        })
    }
}

fn outline_id(value: &Value) -> Option<&str> {
    value.get("id").or_else(|| value.get("pageId"))?.as_str()
}
