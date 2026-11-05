// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Package-file decisions carry canonical bytes as strings (including LFS
//! pointers). The package coordinator validates and publishes their whole tree.
use super::*;
use crate::domain::merge::model::{Alternative, Context, stable_id};

mod scope;
pub(crate) use scope::FileScope;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileDecision {
    pub(crate) id: String,
    pub(crate) target: String,
    pub(crate) path: String,
    pub(crate) scope: FileScope,
    pub(crate) base_epoch: String,
    pub(crate) base: Fragment,
    pub(crate) alternatives: [Alternative; 2],
    pub(crate) provisional: usize,
}

impl FileDecision {
    pub(crate) fn identity(&self) -> Result<String, String> {
        stable_id(&(
            "bluink:file-decision:v1",
            &self.target,
            &self.path,
            &self.scope,
            &self.base_epoch,
            &self.base,
            &self.alternatives[0].value,
            &self.alternatives[1].value,
        ))
    }
    pub(crate) fn selected(&self, selection: Selection) -> Result<Fragment, String> {
        let index = match selection {
            Selection::Provisional => self.provisional,
            Selection::First => 0,
            Selection::Second => 1,
            Selection::Both
                if self.scope == FileScope::Whole
                    && self.path.starts_with("assets/")
                    && self
                        .alternatives
                        .iter()
                        .all(|alternative| matches!(alternative.value, Fragment::Present(_))) =>
            {
                self.provisional
            }
            Selection::Both => return Err("Keep both requires two asset alternatives".into()),
        };
        self.alternatives
            .get(index)
            .map(|a| a.value.clone())
            .ok_or_else(|| "Invalid file alternative".into())
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.path.starts_with('/')
            || self.path.contains('\\')
            || self.path.contains(':')
            || self
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !(self.path == "bluink.json"
                || self.path == ".gitignore"
                || self.path == ".gitattributes"
                || self.path.starts_with("themes/")
                || self.path.starts_with("assets/"))
        {
            return Err("File decision path is outside portable project content".into());
        }
        for fragment in [
            &self.base,
            &self.alternatives[0].value,
            &self.alternatives[1].value,
        ] {
            if !matches!(self.scope, FileScope::Json { .. })
                && matches!(fragment, Fragment::Present(value) if !value.is_string())
            {
                return Err("File alternatives must contain text or an LFS pointer".into());
            }
            if matches!(self.scope, FileScope::Text { .. })
                && matches!(fragment,Fragment::Present(value) if value.as_str()==Some(""))
            {
                return Err("An empty hunk must record absence".into());
            }
        }
        self.scope.validate(&self.path)?;
        self.selected(Selection::Provisional)?;
        ordered(&self.alternatives[0].value, &self.alternatives[1].value)?;
        if self.alternatives[0].value == self.alternatives[1].value {
            return Err("File decision alternatives must differ".into());
        }
        Ok(())
    }
}

pub(crate) fn propose(
    context: &Context,
    path: &str,
    base: Option<&str>,
    left: Option<&str>,
    right: Option<&str>,
) -> Result<Proposal, String> {
    let fragment = |value: Option<&str>| {
        value.map_or(Fragment::Missing, |text| Fragment::Present(json!(text)))
    };
    propose_scoped(
        context,
        path,
        FileScope::Whole,
        fragment(base),
        fragment(left),
        fragment(right),
    )
}

pub(crate) fn propose_scoped(
    context: &Context,
    path: &str,
    scope: FileScope,
    base: Fragment,
    left: Fragment,
    right: Fragment,
) -> Result<Proposal, String> {
    let prefer_left = context.prefer_left()?;
    let choose_left = if left == Fragment::Missing || right == Fragment::Missing {
        left != Fragment::Missing
    } else {
        prefer_left
    };
    let mut alternatives = [
        Alternative {
            value: left,
            source: context.left.clone(),
        },
        Alternative {
            value: right,
            source: context.right.clone(),
        },
    ];
    let swap = canonical_json::to_vec(&alternatives[0].value).map_err(|e| e.to_string())?
        > canonical_json::to_vec(&alternatives[1].value).map_err(|e| e.to_string())?;
    if swap {
        alternatives.swap(0, 1);
    }
    let mut decision = FileDecision {
        id: String::new(),
        target: context.target.clone(),
        path: path.to_owned(),
        scope,
        base_epoch: context.base_epoch.clone(),
        base,
        alternatives,
        provisional: usize::from(choose_left == swap),
    };
    decision.id = decision.identity()?;
    let proposal = Proposal::File(Box::new(decision));
    proposal.validate()?;
    Ok(proposal)
}
