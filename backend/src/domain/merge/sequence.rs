// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Merge identity and content separately from position. A longest increasing
//! subsequence identifies stable base anchors without an N×N text-diff matrix.
//! Changed runs contribute ordering constraints; incompatible moves produce a
//! position decision rather than silently dropping an entry or one side's order.

use super::model::{DecisionKind, Location, Merger};
use super::value::{Agreement, unchanged};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Entries<'a> = (Vec<String>, BTreeMap<String, &'a Value>);

fn entries(value: Option<&Value>, key: fn(&Value) -> Option<&str>) -> Result<Entries<'_>, String> {
    let mut order = Vec::new();
    let mut values = BTreeMap::new();
    if let Some(value) = value {
        for entry in value
            .as_array()
            .ok_or("Identity sequence must be an array")?
        {
            let id = key(entry)
                .filter(|id| !id.is_empty())
                .ok_or("Sequence entry needs an identity")?;
            if values.insert(id.to_owned(), entry).is_some() {
                return Err(format!("Duplicate sequence identity: {id}"));
            }
            order.push(id.to_owned());
        }
    }
    Ok((order, values))
}

impl Merger<'_> {
    pub(super) fn sequence(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
        key: fn(&Value) -> Option<&str>,
        merge_entry: impl Fn(
            &mut Self,
            &Location,
            Option<&Value>,
            &Value,
            &Value,
        ) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let (base_order, base) = entries(base, key)?;
        let (left_order, left) = entries(Some(left), key)?;
        let (right_order, right) = entries(Some(right), key)?;
        let left_moved = moved(&base_order, &left_order);
        let right_moved = moved(&base_order, &right_order);
        let keys: BTreeSet<_> = base.keys().chain(left.keys()).chain(right.keys()).collect();
        let mut merged = BTreeMap::new();
        for id in keys {
            let mut b = base.get(id).copied();
            let l = left.get(id).copied();
            let r = right.get(id).copied();
            let location = path.entry(id);
            // Both sides moved this block here from elsewhere: merge from its base.
            let moved = if b.is_none() && l.is_some() && r.is_some() {
                self.moved_into(id)
            } else {
                None
            };
            if let Some(moved) = &moved {
                b = Some(moved.base());
            }
            // Position is authored intent too: deleting a block while the other
            // side moves it must not silently erase the moved block.
            let deleted_moved = b.is_some()
                && ((l.is_none() && right_moved.contains(id))
                    || (r.is_none() && left_moved.contains(id)));
            let value = if deleted_moved {
                self.conflict(&location, DecisionKind::DeleteEdit, b, l, r)?
            } else if let Agreement::Chosen(value) = unchanged(b, l, r) {
                match value.cloned() {
                    Some(value) if !self.moved.is_empty() => self.adopt(&location, value)?,
                    value => value,
                }
            } else if let (Some(l), Some(r)) = (l, r) {
                Some(merge_entry(self, &location, b, l, r)?)
            } else {
                self.conflict(&location, DecisionKind::DeleteEdit, b, l, r)?
            };
            if let Some(value) = value {
                merged.insert(id.clone(), value);
            }
        }
        let retain = |order: Vec<String>| {
            order
                .into_iter()
                .filter(|id| merged.contains_key(id))
                .collect::<Vec<_>>()
        };
        let base_order = retain(base_order);
        let left_order = retain(left_order);
        let right_order = retain(right_order);
        let order = self.order(path, &base_order, &left_order, &right_order)?;
        order
            .into_iter()
            .map(|id| {
                merged
                    .remove(&id)
                    .ok_or_else(|| format!("Merged sequence lost identity {id}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    }

    fn order(
        &mut self,
        path: &Location,
        base: &[String],
        left: &[String],
        right: &[String],
    ) -> Result<Vec<String>, String> {
        let (preferred, other) = if self.prefer_left {
            (left, right)
        } else {
            (right, left)
        };
        let all: BTreeSet<_> = base.iter().chain(left).chain(right).cloned().collect();
        let left_moved = moved(base, left);
        let right_moved = moved(base, right);
        let moved: BTreeSet<_> = left_moved.union(&right_moved).cloned().collect();
        let mut edges: BTreeMap<String, BTreeSet<String>> =
            all.iter().map(|id| (id.clone(), BTreeSet::new())).collect();
        let stable: Vec<_> = base
            .iter()
            .filter(|id| !moved.contains(*id))
            .cloned()
            .collect();
        add_chain(&mut edges, &stable);
        add_changes(&mut edges, base, left, &left_moved);
        add_changes(&mut edges, base, right, &right_moved);
        let competing = left_moved
            .intersection(&right_moved)
            .any(|id| anchors(left, id, &moved) != anchors(right, id, &moved));
        if !competing && let Some(order) = topological(&edges, preferred, other, base) {
            return Ok(order);
        }
        let b = Value::from(base.to_vec());
        let l = Value::from(left.to_vec());
        let r = Value::from(right.to_vec());
        self.conflict(
            &path.field("$order"),
            DecisionKind::Position,
            Some(&b),
            Some(&l),
            Some(&r),
        )?;
        // A position choice cannot remove an entry whose content survived. Put
        // entries absent from the preferred side beside their surviving anchors.
        Ok(complete_order(preferred, other, base))
    }
}

fn anchors<'a>(
    order: &'a [String],
    id: &str,
    moved: &BTreeSet<String>,
) -> (Option<&'a str>, Option<&'a str>) {
    let index = order
        .iter()
        .position(|value| value == id)
        .unwrap_or(order.len());
    let before = order[..index]
        .iter()
        .rev()
        .find(|id| !moved.contains(*id))
        .map(String::as_str);
    let after = order
        .iter()
        .skip(index + 1)
        .find(|id| !moved.contains(*id))
        .map(String::as_str);
    (before, after)
}

fn moved(base: &[String], side: &[String]) -> BTreeSet<String> {
    let positions: BTreeMap<_, _> = base
        .iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let existing: Vec<_> = side
        .iter()
        .filter_map(|id| positions.get(id).map(|index| (id, *index)))
        .collect();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; existing.len()];
    for (index, (_, position)) in existing.iter().enumerate() {
        let slot = tails.partition_point(|tail| existing[*tail].1 < *position);
        if slot > 0 {
            previous[index] = Some(tails[slot - 1]);
        }
        if slot == tails.len() {
            tails.push(index);
        } else {
            tails[slot] = index;
        }
    }
    let mut moved: BTreeSet<_> = existing.iter().map(|(id, _)| (*id).clone()).collect();
    let mut cursor = tails.last().copied();
    while let Some(index) = cursor {
        moved.remove(existing[index].0);
        cursor = previous[index];
    }
    moved
}

fn add_chain(edges: &mut BTreeMap<String, BTreeSet<String>>, order: &[String]) {
    for pair in order.windows(2) {
        if let Some(next) = edges.get_mut(&pair[0]) {
            next.insert(pair[1].clone());
        }
    }
}

fn add_changes(
    edges: &mut BTreeMap<String, BTreeSet<String>>,
    base: &[String],
    side: &[String],
    moved: &BTreeSet<String>,
) {
    let base: BTreeSet<_> = base.iter().collect();
    let changed = |id: &String| moved.contains(id) || !base.contains(id);
    for pair in side.windows(2) {
        if changed(&pair[0]) || changed(&pair[1]) {
            add_chain(edges, pair);
        }
    }
}

fn topological(
    edges: &BTreeMap<String, BTreeSet<String>>,
    preferred: &[String],
    other: &[String],
    base: &[String],
) -> Option<Vec<String>> {
    let mut rank = BTreeMap::new();
    for (index, id) in preferred.iter().chain(other).chain(base).enumerate() {
        rank.entry(id).or_insert(index);
    }
    let mut incoming: BTreeMap<_, usize> = edges.keys().map(|id| (id, 0)).collect();
    for next in edges.values().flatten() {
        *incoming.get_mut(next)? += 1;
    }
    let mut ready = BTreeSet::new();
    for (id, count) in &incoming {
        if *count == 0 {
            ready.insert((*rank.get(id)?, *id));
        }
    }
    let mut result = Vec::new();
    while let Some((_, id)) = ready.pop_first() {
        result.push(id.clone());
        for next in edges.get(id)? {
            let count = incoming.get_mut(next)?;
            *count = count.checked_sub(1)?;
            if *count == 0 {
                ready.insert((*rank.get(next)?, next));
            }
        }
    }
    (result.len() == edges.len()).then_some(result)
}

pub(super) fn complete_order(
    preferred: &[String],
    other: &[String],
    base: &[String],
) -> Vec<String> {
    let mut result = preferred.to_vec();
    let mut present: BTreeSet<_> = result.iter().cloned().collect();
    for source in [other, base] {
        for (index, id) in source.iter().enumerate() {
            if present.insert(id.clone()) {
                let before = source[..index]
                    .iter()
                    .rev()
                    .find_map(|anchor| result.iter().position(|id| id == anchor));
                let after = source[index + 1..]
                    .iter()
                    .find_map(|anchor| result.iter().position(|id| id == anchor));
                result.insert(
                    before.map_or_else(|| after.unwrap_or(result.len()), |index| index + 1),
                    id.clone(),
                );
            }
        }
    }
    result
}
