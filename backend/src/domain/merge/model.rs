// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::domain::project::canonical_json;

/// Missing and JSON null are different alternatives, including in identity hashes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "presence", content = "value", rename_all = "snake_case")]
pub(crate) enum Fragment {
    Missing,
    Present(Value),
}

impl From<Option<&Value>> for Fragment {
    fn from(value: Option<&Value>) -> Self {
        value.map_or(Self::Missing, |value| Self::Present(value.clone()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Source {
    Commit {
        id: String,
        committer_time: i64,
        device: String,
    },
    Draft {
        submission_id: String,
        device: String,
    },
    Revision {
        revision: String,
        modified_at: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub(crate) target: String,
    /// A typed/domain-qualified merge base or trusted draft-base revision.
    pub(crate) base_epoch: String,
    pub(crate) left: Source,
    pub(crate) right: Source,
}

impl Context {
    pub(crate) fn prefer_left(&self) -> Result<bool, String> {
        match (&self.left, &self.right) {
            (
                Source::Commit {
                    id: left,
                    committer_time: left_time,
                    ..
                },
                Source::Commit {
                    id: right,
                    committer_time: right_time,
                    ..
                },
            ) => {
                if left == right {
                    return Err("Merge sources must identify distinct commits.".into());
                }
                Ok((left_time, left) > (right_time, right))
            }
            (Source::Draft { .. }, Source::Revision { .. }) => Ok(true),
            (Source::Revision { .. }, Source::Draft { .. }) => Ok(false),
            _ => Err("Expected two commits, or a draft and its current canonical revision.".into()),
        }
    }
}

/// Structured locations avoid ambiguities between field names and block IDs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Location(pub(crate) Vec<Segment>);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum Segment {
    Field(String),
    Entry(String),
    Cell(CellLocation),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CellLocation {
    pub(crate) index: usize,
    pub(crate) structure: Vec<Value>,
}

pub(super) fn cell_structure(row: &Value) -> Result<Vec<Value>, String> {
    if row["type"] != "table_row" {
        return Err("Cell parent is not a table row".into());
    }
    row["content"]
        .as_array()
        .ok_or_else(|| "Table row has no cells".into())
        .map(|cells| {
            cells
                .iter()
                .map(|cell| serde_json::json!({"type":cell["type"],"attrs":cell["attrs"]}))
                .collect()
        })
}

impl Location {
    pub(super) fn field(&self, key: &str) -> Self {
        self.child(Segment::Field(key.to_owned()))
    }
    pub(super) fn entry(&self, id: &str) -> Self {
        self.child(Segment::Entry(id.to_owned()))
    }
    pub(super) fn cell(&self, index: usize, structure: Vec<Value>) -> Self {
        self.child(Segment::Cell(CellLocation { index, structure }))
    }
    fn child(&self, segment: Segment) -> Self {
        let mut path = self.0.clone();
        path.push(segment);
        Self(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecisionKind {
    Field,
    Block,
    DeleteEdit,
    Position,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Alternative {
    pub(crate) value: Fragment,
    pub(crate) source: Source,
}

/// A proposal, not a persisted conflict item. The coordinator must consult open
/// decisions and resolution tombstones before creating or accepting this decision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Decision {
    pub(crate) id: String,
    pub(crate) target: String,
    pub(crate) location: Location,
    pub(crate) kind: DecisionKind,
    pub(crate) base_epoch: String,
    pub(crate) base: Fragment,
    pub(crate) alternatives: [Alternative; 2],
    pub(crate) provisional: usize,
}

impl Decision {
    /// Ordering choices preserve surviving entries absent from the selected
    /// branch. Use the same completion policy as candidate construction.
    pub(super) fn selected(&self, index: usize) -> Result<Fragment, String> {
        let chosen = self
            .alternatives
            .get(index)
            .ok_or("Invalid decision alternative")?;
        if self.kind != DecisionKind::Position {
            return Ok(chosen.value.clone());
        }
        let order = |fragment: &Fragment| -> Result<Vec<String>, String> {
            let Fragment::Present(value) = fragment else {
                return Err("Ordering needs a present identity sequence".into());
            };
            let values: Vec<String> =
                serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            let mut unique = std::collections::BTreeSet::new();
            for value in &values {
                crate::domain::project::identity::ItemId::try_from(value.as_str())?;
                if !unique.insert(value) {
                    return Err("Duplicate ordering identity".into());
                }
            }
            Ok(values)
        };
        Ok(Fragment::Present(Value::from(
            super::sequence::complete_order(
                &order(&chosen.value)?,
                &order(&self.alternatives[1 - index].value)?,
                &order(&self.base)?,
            ),
        )))
    }

    pub(super) fn identity(&self) -> Result<String, String> {
        stable_id(&(
            "bluink:merge-decision:v1",
            &self.target,
            &self.location,
            self.kind,
            &self.base_epoch,
            &self.base,
            &self.alternatives[0].value,
            &self.alternatives[1].value,
        ))
    }
}

pub(super) struct Merger<'a> {
    pub(super) context: &'a Context,
    pub(super) prefer_left: bool,
    pub(super) decisions: Vec<Decision>,
    /// Blocks moved into another container during the current document merge.
    pub(super) moved: BTreeMap<String, super::document::Moved>,
}

impl<'a> Merger<'a> {
    pub(super) fn new(context: &'a Context) -> Result<Self, String> {
        if context.target.is_empty() || context.base_epoch.is_empty() {
            return Err("Merge target and trusted base epoch are required.".into());
        }
        Ok(Self {
            context,
            prefer_left: context.prefer_left()?,
            decisions: Vec::new(),
            moved: BTreeMap::new(),
        })
    }

    pub(super) fn conflict(
        &mut self,
        location: &Location,
        kind: DecisionKind,
        base: Option<&Value>,
        left: Option<&Value>,
        right: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        // An edited value survives deletion even when the deleting side would
        // otherwise win provisionally. The deletion stays in the decision.
        let choose_left = if kind == DecisionKind::DeleteEdit {
            left.is_some()
        } else {
            self.prefer_left
        };
        let mut alternatives = [
            Alternative {
                value: left.into(),
                source: self.context.left.clone(),
            },
            Alternative {
                value: right.into(),
                source: self.context.right.clone(),
            },
        ];
        let left_bytes =
            canonical_json::to_vec(&alternatives[0].value).map_err(|e| e.to_string())?;
        let right_bytes =
            canonical_json::to_vec(&alternatives[1].value).map_err(|e| e.to_string())?;
        let swap = left_bytes > right_bytes;
        if swap {
            alternatives.swap(0, 1);
        }
        let base = Fragment::from(base);
        let mut decision = Decision {
            id: String::new(),
            target: self.context.target.clone(),
            location: location.clone(),
            kind,
            base_epoch: self.context.base_epoch.clone(),
            base,
            alternatives,
            provisional: usize::from(choose_left == swap),
        };
        decision.id = decision.identity()?;
        self.decisions.push(decision);
        Ok(if choose_left { left } else { right }.cloned())
    }
}

pub(crate) fn stable_id(value: &impl Serialize) -> Result<String, String> {
    let hash = Sha256::digest(canonical_json::to_vec(value).map_err(|e| e.to_string())?);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}
