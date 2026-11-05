// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::model::{DecisionKind, Location, Merger};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) enum Agreement<'a> {
    Diverged,
    Chosen(Option<&'a Value>),
}

/// An agreed deletion is distinct from divergent values.
pub(super) fn unchanged<'a>(
    base: Option<&'a Value>,
    left: Option<&'a Value>,
    right: Option<&'a Value>,
) -> Agreement<'a> {
    if left == right || right == base {
        Agreement::Chosen(left)
    } else if left == base {
        Agreement::Chosen(right)
    } else {
        Agreement::Diverged
    }
}

impl Merger<'_> {
    pub(super) fn atomic(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: Option<&Value>,
        right: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        if let Agreement::Chosen(value) = unchanged(base, left, right) {
            return Ok(value.cloned());
        }
        self.conflict(path, DecisionKind::Field, base, left, right)
    }

    /// Fields are atomic unless their owning schema explicitly supplies a policy.
    /// In particular this never recursively invents semantics for arbitrary arrays.
    pub(super) fn fields(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        self.object(path, base, left, right, |merge, child, _, b, l, r| {
            merge.atomic(child, b, l, r)
        })
    }

    pub(super) fn object(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
        mut field: impl FnMut(
            &mut Self,
            &Location,
            &str,
            Option<&Value>,
            Option<&Value>,
            Option<&Value>,
        ) -> Result<Option<Value>, String>,
    ) -> Result<Value, String> {
        let left = left.as_object().ok_or("Expected a left object")?;
        let right = right.as_object().ok_or("Expected a right object")?;
        let base = base.and_then(Value::as_object);
        let keys: BTreeSet<_> = base
            .into_iter()
            .flat_map(Map::keys)
            .chain(left.keys())
            .chain(right.keys())
            .collect();
        let mut result = Map::new();
        for key in keys {
            if let Some(value) = field(
                self,
                &path.field(key),
                key,
                base.and_then(|v| v.get(key)),
                left.get(key),
                right.get(key),
            )? {
                result.insert(key.clone(), value);
            }
        }
        Ok(Value::Object(result))
    }
}

/// Tags, pins, labels and ignored findings are string sets. Use their
/// natural ordering, matching canonical validation (JSON escaping is not order).
pub(super) fn set(
    base: Option<&Value>,
    left: Option<&Value>,
    right: Option<&Value>,
) -> Result<Option<Value>, String> {
    let members = |value: Option<&Value>| -> Result<BTreeSet<String>, String> {
        let mut result = BTreeSet::new();
        if let Some(value) = value {
            for member in value.as_array().ok_or("Set field must be an array")? {
                result.insert(
                    member
                        .as_str()
                        .ok_or("Set members must be strings")?
                        .to_owned(),
                );
            }
        }
        Ok(result)
    };
    let base = members(base)?;
    let left_members = members(left)?;
    let right_members = members(right)?;
    let mut result = BTreeSet::new();
    for value in left_members.iter().chain(&right_members) {
        if !base.contains(value) || (left_members.contains(value) && right_members.contains(value))
        {
            result.insert(value.clone());
        }
    }
    if result.is_empty() && (left.is_none() || right.is_none()) {
        return Ok(None);
    }
    Ok(Some(Value::Array(
        result.into_iter().map(Value::String).collect(),
    )))
}
