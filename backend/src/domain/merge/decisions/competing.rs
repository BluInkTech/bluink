// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use crate::domain::merge::model::{Context, stable_id};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub(crate) selection: Selection,
    pub(crate) source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Competing {
    pub(crate) id: String,
    pub(crate) target: String,
    pub(crate) base_epoch: String,
    pub(crate) original: Box<Proposal>,
    pub(crate) alternatives: [Intent; 2],
    pub(crate) provisional: usize,
}

impl Competing {
    pub(crate) fn choice(&self, selection: Selection) -> Result<Selection, String> {
        let index = match selection {
            Selection::Provisional => self.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both => {
                return Err(
                    "Choose one resolution intent; Keep both is a block-editing action".into(),
                );
            }
        };
        self.alternatives
            .get(index)
            .map(|intent| intent.selection)
            .ok_or_else(|| "Invalid resolution intent".into())
    }
    pub(super) fn identity(&self) -> Result<String, String> {
        stable_id(&(
            "bluink:competing-resolution:v1",
            &self.target,
            &self.base_epoch,
            self.original.id(),
            self.original.selected(self.alternatives[0].selection)?,
            self.original.selected(self.alternatives[1].selection)?,
        ))
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        self.original.validate()?;
        if self.target != self.original.target() {
            return Err("Resolution intent targets must match".into());
        }
        self.choice(Selection::Provisional)?;
        let first = self.original.selected(self.alternatives[0].selection)?;
        let second = self.original.selected(self.alternatives[1].selection)?;
        ordered(&first, &second)?;
        if first == second {
            return Err("Equivalent resolutions do not need another decision".into());
        }
        Ok(())
    }
}

pub(crate) fn merge_records(
    context: &Context,
    base: Option<&Value>,
    left: Option<&Value>,
    right: Option<&Value>,
) -> Result<(Value, Vec<Proposal>), String> {
    let (Some(left), Some(right)) = (left, right) else {
        return merge(base, left, right).map(|record| (record, Vec::new()));
    };
    let l = read(left)?;
    let r = read(right)?;
    let (Some(a), Some(b)) = (&l.resolution, &r.resolution) else {
        return merge(base, Some(left), Some(right)).map(|record| (record, Vec::new()));
    };
    if l.proposal.id() != r.proposal.id() {
        return Err("Different conflict identities".into());
    }
    let first = l.proposal.selected(a.selection)?;
    let second = r.proposal.selected(b.selection)?;
    if first == second {
        return merge(None, Some(left), Some(right)).map(|record| (record, Vec::new()));
    }
    if let Some(base) = base {
        let record = read(base)?;
        if let Some(resolution) = record.resolution {
            let previous = record.proposal.selected(resolution.selection)?;
            if previous == first {
                return Ok((right.clone(), Vec::new()));
            }
            if previous == second {
                return Ok((left.clone(), Vec::new()));
            }
        }
    }
    let original = if canonical_json::to_vec(&l.proposal).map_err(|e| e.to_string())?
        <= canonical_json::to_vec(&r.proposal).map_err(|e| e.to_string())?
    {
        l.proposal
    } else {
        r.proposal
    };
    let mut alternatives = [
        Intent {
            selection: original.selection_for(&first)?,
            source: context.left.clone(),
        },
        Intent {
            selection: original.selection_for(&second)?,
            source: context.right.clone(),
        },
    ];
    let swap = canonical_json::to_vec(&first).map_err(|e| e.to_string())?
        > canonical_json::to_vec(&second).map_err(|e| e.to_string())?;
    if swap {
        alternatives.swap(0, 1);
    }
    let prefer_left = context.prefer_left()?;
    let mut followup = Competing {
        id: String::new(),
        target: original.target().to_owned(),
        base_epoch: context.base_epoch.clone(),
        original: Box::new(original),
        alternatives,
        provisional: usize::from(prefer_left == swap),
    };
    followup.id = followup.identity()?;
    let proposal = Proposal::Resolution(Box::new(followup));
    proposal.validate()?;
    Ok((
        if prefer_left { left } else { right }.clone(),
        vec![proposal],
    ))
}
