// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::model::{DecisionKind, Location, Merger};
use super::value::{Agreement, set, unchanged};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn block_id(value: &Value) -> Option<&str> {
    value.get("attrs")?.get("id")?.as_str()
}

pub(super) fn paragraph_body(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(attrs) = value.get_mut("attrs").and_then(Value::as_object_mut) {
        attrs.remove("labels");
    }
    value
}

/// A block one or both sides moved into another container. Per-container
/// merging alone would see a deletion in one place and an addition in another,
/// and keep a copy in each.
#[derive(Clone)]
pub(super) struct Moved {
    base: Value,
    /// The side that left the block where it was (true for left), and its copy,
    /// lifted out of that side so it merges into the moved copy instead.
    /// `None` when both sides moved it to the same container.
    stayed: Option<(bool, Option<Value>)>,
}

impl Moved {
    pub(super) const fn base(&self) -> &Value {
        &self.base
    }
}

/// Each block's container: the nearest ancestor block ID, plus cell positions
/// for containers without one. The document root is "".
fn containers(node: &Value, container: &str, found: &mut BTreeMap<String, String>) {
    let Some(children) = node.get("content").and_then(Value::as_array) else {
        return;
    };
    for (index, child) in children.iter().enumerate() {
        if let Some(id) = block_id(child) {
            found.insert(id.to_owned(), container.to_owned());
            containers(child, id, found);
        } else {
            containers(child, &format!("{container}/{index}"), found);
        }
    }
}

fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if block_id(node) == Some(id) {
        return Some(node);
    }
    node.get("content")?
        .as_array()?
        .iter()
        .find_map(|child| find(child, id))
}

fn take(node: &mut Value, id: &str) -> Option<Value> {
    let children = node.get_mut("content")?.as_array_mut()?;
    if let Some(index) = children
        .iter()
        .position(|child| block_id(child) == Some(id))
    {
        return Some(children.remove(index));
    }
    children.iter_mut().find_map(|child| take(child, id))
}

/// Lift a block out of the side that left it in place. A block inside another
/// lifted block travels with it, so look there too.
fn lift(
    document: &mut Value,
    moved: &mut BTreeMap<String, Moved>,
    left: bool,
    id: &str,
) -> Option<Value> {
    take(document, id).or_else(|| {
        moved
            .values_mut()
            .find_map(|other| match &mut other.stayed {
                Some((side, Some(value))) if *side == left => take(value, id),
                _ => None,
            })
    })
}

impl Merger<'_> {
    /// Merge a whole document, treating a block whose container changed on one
    /// side as moved: the other side's edits to it follow it to its new place.
    pub(super) fn document(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        let Some(base_document) = base else {
            return self.node(path, base, left, right);
        };
        let parents = |document: &Value| {
            let mut found = BTreeMap::new();
            containers(document, "", &mut found);
            found
        };
        let (from, to_left, to_right) = (parents(base_document), parents(left), parents(right));
        let mut lifted_left = left.clone();
        let mut lifted_right = right.clone();
        let mut moved: BTreeMap<String, Moved> = BTreeMap::new();
        for (id, container) in &from {
            let (l, r) = (to_left.get(id), to_right.get(id));
            let left_moved = l.is_some_and(|l| l != container);
            let right_moved = r.is_some_and(|r| r != container);
            let stayed = match (left_moved, right_moved) {
                (false, false) => continue,
                (true, true) if l == r => None,
                // Two different destinations: one decision for the document.
                (true, true) => return self.block_conflict(path, base, left, right),
                (true, false) => Some((false, lift(&mut lifted_right, &mut moved, false, id))),
                (false, true) => Some((true, lift(&mut lifted_left, &mut moved, true, id))),
            };
            let base = find(base_document, id)
                .ok_or("Moved block is missing from its base")?
                .clone();
            moved.insert(id.clone(), Moved { base, stayed });
        }
        if moved.is_empty() {
            return self.node(path, base, left, right);
        }
        self.moved = moved;
        let result = self.node(path, base, &lifted_left, &lifted_right);
        self.moved.clear();
        result
    }

    /// The base and the other side's copy of a block moved into this container.
    pub(super) fn moved_into(&self, id: &str) -> Option<Moved> {
        self.moved.get(id).cloned()
    }

    /// A subtree only one side added may carry blocks the other side still had
    /// elsewhere; merge each such block with that copy.
    pub(super) fn adopt(
        &mut self,
        path: &Location,
        mut value: Value,
    ) -> Result<Option<Value>, String> {
        if let Some(id) = block_id(&value).map(str::to_owned)
            && let Some(moved) = self.moved.remove(&id)
        {
            return match moved.stayed {
                Some((stayed_left, Some(other))) => {
                    let (l, r) = if stayed_left {
                        (other, value)
                    } else {
                        (value, other)
                    };
                    self.node(path, Some(&moved.base), &l, &r).map(Some)
                }
                // Moving is authored intent: deleting it on the other side is a decision.
                Some((stayed_left, None)) => {
                    let (l, r) = if stayed_left {
                        (None, Some(&value))
                    } else {
                        (Some(&value), None)
                    };
                    self.conflict(path, DecisionKind::DeleteEdit, Some(&moved.base), l, r)
                }
                None => Ok(Some(value)),
            };
        }
        if self.moved.is_empty() {
            return Ok(Some(value));
        }
        let content = path.field("content");
        if let Some(children) = value.get_mut("content").and_then(Value::as_array_mut) {
            let mut adopted = Vec::with_capacity(children.len());
            for child in std::mem::take(children) {
                let location =
                    block_id(&child).map_or_else(|| content.clone(), |id| content.entry(id));
                adopted.extend(self.adopt(&location, child)?);
            }
            *children = adopted;
        }
        Ok(Some(value))
    }

    pub(super) fn node(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        if let Agreement::Chosen(Some(value)) = unchanged(base, Some(left), Some(right)) {
            // The side taken whole may hold a block the other side edited elsewhere.
            return self
                .adopt(path, value.clone())?
                .ok_or_else(|| "Adopting a chosen block removed it".into());
        }
        let kind = left["type"].as_str().ok_or("Document node needs a type")?;
        if left["type"] != right["type"] || base.is_some_and(|b| b["type"] != left["type"]) {
            return self.block_conflict(path, base, left, right);
        }
        match kind {
            "paragraph" => self.paragraph(path, base, left, right),
            "doc" | "list" | "listItem" | "box" | "table_cell" | "table_header" => {
                self.container(path, base, left, right)
            }
            "table"
                if base.is_some_and(|b| {
                    table_structure_unchanged(b, left) && table_structure_unchanged(b, right)
                }) =>
            {
                self.container(path, base, left, right)
            }
            "table_row" => self.row(path, base, left, right),
            _ => self.block_conflict(path, base, left, right),
        }
    }

    fn block_conflict(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        self.conflict(path, DecisionKind::Block, base, Some(left), Some(right))?
            .ok_or_else(|| "Block conflict unexpectedly deleted both alternatives".into())
    }

    fn paragraph(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        let b = base.map(paragraph_body);
        let l = paragraph_body(left);
        let r = paragraph_body(right);
        let mut result =
            if let Agreement::Chosen(Some(value)) = unchanged(b.as_ref(), Some(&l), Some(&r)) {
                value.clone()
            } else {
                self.block_conflict(&path.field("$body"), b.as_ref(), &l, &r)?
            };
        let labels = set(
            base.and_then(|b| b["attrs"].get("labels")),
            left["attrs"].get("labels"),
            right["attrs"].get("labels"),
        )?;
        if let Some(labels) = labels {
            if result.get("attrs").is_none() {
                result["attrs"] = serde_json::json!({});
            }
            result["attrs"]["labels"] = labels;
        }
        Ok(result)
    }

    fn container(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        self.object(path, base, left, right, |merge, child, key, b, l, r| {
            if let Agreement::Chosen(value) = unchanged(b, l, r)
                && (key != "content" || merge.moved.is_empty())
            {
                return Ok(value.cloned());
            }
            if key == "content" {
                let empty = Value::Array(Vec::new());
                return merge
                    .sequence(
                        child,
                        b,
                        l.unwrap_or(&empty),
                        r.unwrap_or(&empty),
                        block_id,
                        Self::node,
                    )
                    .map(Some);
            }
            if key == "attrs"
                && let (Some(l), Some(r)) = (l, r)
            {
                return merge.fields(child, b, l, r).map(Some);
            }
            merge.atomic(child, b, l, r)
        })
    }

    /// Cells have positional identity in format 19. Only a structurally unchanged
    /// table reaches here; their child paragraphs still merge by stable block ID.
    fn row(
        &mut self,
        path: &Location,
        base: Option<&Value>,
        left: &Value,
        right: &Value,
    ) -> Result<Value, String> {
        let cells = |value: &Value| {
            value["content"]
                .as_array()
                .cloned()
                .ok_or("Table row needs cells")
        };
        let b = base.map(cells).transpose()?;
        let l = cells(left)?;
        let r = cells(right)?;
        if l.len() != r.len() || b.as_ref().is_none_or(|b| b.len() != l.len()) {
            return self.block_conflict(path, base, left, right);
        }
        let content = Value::Array(
            l.iter()
                .zip(&r)
                .enumerate()
                .map(|(index, (l, r))| {
                    self.node(
                        &path.cell(index, super::model::cell_structure(left)?),
                        b.as_ref().and_then(|b| b.get(index)),
                        l,
                        r,
                    )
                })
                .collect::<Result<_, _>>()?,
        );
        self.object(path, base, left, right, |merge, child, key, b, l, r| {
            if key == "content" {
                Ok(Some(content.clone()))
            } else {
                merge.atomic(child, b, l, r)
            }
        })
    }
}

fn table_structure_unchanged(base: &Value, side: &Value) -> bool {
    if base["attrs"] != side["attrs"] {
        return false;
    }
    let (Some(base_rows), Some(side_rows)) =
        (base["content"].as_array(), side["content"].as_array())
    else {
        return false;
    };
    let columns = base_rows
        .first()
        .and_then(|row| row["content"].as_array())
        .map_or(0, Vec::len);
    let by_id: BTreeMap<_, _> = base_rows
        .iter()
        .filter_map(|row| block_id(row).map(|id| (id, row)))
        .collect();
    side_rows.iter().all(|row| {
        let Some(cells) = row["content"].as_array() else {
            return false;
        };
        if cells.len() != columns {
            return false;
        }
        if let Some(previous) = block_id(row).and_then(|id| by_id.get(id)) {
            let Some(before) = previous["content"].as_array() else {
                return false;
            };
            before.len() == cells.len()
                && before
                    .iter()
                    .zip(cells)
                    .all(|(b, s)| b["type"] == s["type"] && b["attrs"] == s["attrs"])
        } else {
            cells.iter().all(|cell| {
                ["colspan", "rowspan"]
                    .iter()
                    .all(|key| cell["attrs"].get(key).is_none_or(|value| value == 1))
            })
        }
    })
}
