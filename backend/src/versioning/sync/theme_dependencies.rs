// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Restore a referenced deleted theme as a coherent package. Every differing
//! file keeps its exact branch alternatives; package validation rejects choices
//! that would leave surviving publications with an unavailable theme.
use crate::{
    domain::{
        merge::{
            decisions::{self, Proposal},
            model::Context,
        },
        publication::theme,
    },
    versioning::{Result, failure},
};
use git2::{Oid, Repository};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn repair(
    repo: &Repository,
    context: &Context,
    sources: [&BTreeMap<String, Oid>; 3],
    candidate: &mut BTreeMap<String, Oid>,
) -> Result<Vec<Proposal>> {
    // Ordinary item merges need no package discovery. Scan publication
    // declarations only if one of the captured themes actually disappeared.
    if !sources.iter().any(|files| {
        files.keys().any(|path| {
            path.starts_with("themes/")
                && path.ends_with("/manifest.json")
                && !candidate.contains_key(path)
        })
    }) {
        return Ok(Vec::new());
    }
    let mut required = BTreeSet::new();
    for (path, oid) in candidate
        .iter()
        .filter(|(path, _)| path.starts_with("items/"))
    {
        let item: Value = serde_json::from_slice(repo.find_blob(*oid)?.content())?;
        if item["meta"]["type"] == "publication" {
            let theme = item["content"]["themeId"]
                .as_str()
                .ok_or_else(|| failure(format!("Publication {path} has no theme")))?;
            if !theme::is_builtin(theme)
                && !candidate.contains_key(&format!("themes/{theme}/manifest.json"))
            {
                required.insert(theme.to_owned());
            }
        }
    }
    let [base, left, right] = sources;
    let preferred = if context.prefer_left()? {
        [left, right, base]
    } else {
        [right, left, base]
    };
    let mut proposals = Vec::new();
    for theme in required {
        let prefix = format!("themes/{theme}/");
        let restored = preferred
            .iter()
            .find(|files| files.contains_key(&format!("{prefix}manifest.json")))
            .ok_or_else(|| failure(format!("No surviving package can restore theme {theme}")))?;
        let paths: BTreeSet<_> = base
            .keys()
            .chain(left.keys())
            .chain(right.keys())
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect();
        for path in paths {
            let chosen = restored.get(&path).copied();
            if let Some(oid) = chosen {
                candidate.insert(path.clone(), oid);
            } else {
                candidate.remove(&path);
            }
            if left.get(&path) == right.get(&path) {
                continue;
            }
            let text = |files: &BTreeMap<String, Oid>| -> Result<Option<String>> {
                files
                    .get(&path)
                    .map(|oid| {
                        String::from_utf8(repo.find_blob(*oid)?.content().to_vec())
                            .map_err(|_| failure("Theme files must contain UTF-8 or LFS pointers"))
                    })
                    .transpose()
            };
            let (b, l, r) = (text(base)?, text(left)?, text(right)?);
            let mut proposal =
                decisions::file::propose(context, &path, b.as_deref(), l.as_deref(), r.as_deref())?;
            if let Proposal::File(file) = &mut proposal {
                let selected = super::decisions::fragment(repo, chosen)?;
                file.provisional = file
                    .alternatives
                    .iter()
                    .position(|alternative| alternative.value == selected)
                    .ok_or_else(|| {
                        failure("Restored package has no matching branch alternative")
                    })?;
            }
            proposals.push(proposal);
        }
    }
    Ok(proposals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::merge::model::{Fragment, Source};
    #[test]
    fn restores_one_complete_surviving_theme_and_preserves_removed_alternatives() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repository::init_bare(temp.path()).unwrap();
        let manifest = repo.blob(b"manifest").unwrap();
        let template = repo.blob(b"template").unwrap();
        let base: BTreeMap<String, Oid> = BTreeMap::from([
            ("themes/custom/manifest.json".into(), manifest),
            ("themes/custom/page.html".into(), template),
        ]);
        let mut left = base.clone();
        let id = "01a0e2a0-0000-7000-8000-000000000020";
        let publication = serde_json::json!({"id":id,"meta":{"type":"publication"},"content":{"themeId":"custom"}});
        left.insert(
            "items/00/publication.json".into(),
            repo.blob(&serde_json::to_vec(&publication).unwrap())
                .unwrap(),
        );
        let right = BTreeMap::new();
        let mut candidate = left.clone();
        candidate.retain(|path, _| !path.starts_with("themes/"));
        let context = Context {
            target: id.into(),
            base_epoch: "base".into(),
            left: Source::Commit {
                id: "left".into(),
                committer_time: 1,
                device: "first".into(),
            },
            right: Source::Commit {
                id: "right".into(),
                committer_time: 2,
                device: "second".into(),
            },
        };
        let proposals = repair(&repo, &context, [&base, &left, &right], &mut candidate).unwrap();
        assert_eq!(candidate, left);
        assert_eq!(proposals.len(), 2);
        for proposal in proposals {
            let file = proposal.file_decision().unwrap();
            assert!(
                file.alternatives
                    .iter()
                    .any(|alternative| alternative.value == Fragment::Missing)
            );
            assert!(matches!(
                file.selected(decisions::Selection::Provisional).unwrap(),
                Fragment::Present(_)
            ));
        }
    }
}
