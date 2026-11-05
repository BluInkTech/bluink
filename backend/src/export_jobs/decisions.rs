// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Report open choices without changing or blocking exported provisional prose.
use super::*;
use crate::domain::{
    merge::decisions::{self, Proposal, Selection},
    publication::plan::DecisionWarning,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn annotate(root: &Path, plan: &mut PublicationPlan) -> Result<()> {
    if let Some(warnings) = collect(
        root,
        &plan.publication_id,
        &plan.pages.iter().map(|page| page.id.clone()).collect(),
    )? {
        plan.warnings = warnings;
        plan.decisions_checked = true;
    }
    Ok(())
}

pub(super) fn annotate_page(root: &Path, plan: &mut PagePlan) -> Result<()> {
    if let Some(warnings) = collect(root, &plan.page_id, &BTreeSet::from([plan.page_id.clone()]))? {
        plan.warnings = warnings;
        plan.decisions_checked = true;
    }
    Ok(())
}

fn collect(
    root: &Path,
    publication: &str,
    included: &BTreeSet<String>,
) -> Result<Option<Vec<DecisionWarning>>> {
    let store = crate::store::Store::shared(root)?;
    let warnings=store.read(|tx| {
        let ids=tx.prepare("SELECT e.uuid FROM decision_projection d JOIN entity e ON e.id=d.item_id WHERE d.resolved=0 ORDER BY e.uuid")?.query_map([],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut warnings: BTreeMap<String,Vec<String>>=BTreeMap::new();
        for id in ids {
            let item=crate::store::item::read(tx,&id,true)?.ok_or(crate::store::Error::NotFound)?;
            let record=decisions::read(&item).map_err(crate::store::Error::Invalid)?;
            for page in affected(&record.proposal,publication,included).map_err(crate::store::Error::Invalid)? {
                warnings.entry(page).or_default().push(id.clone());
            }
        }
        Ok(warnings.into_iter().map(|(page_id,decision_ids)|DecisionWarning {page_id,decision_ids}).collect())
    })?;
    Ok(Some(warnings))
}

fn affected(
    proposal: &Proposal,
    publication: &str,
    included: &BTreeSet<String>,
) -> std::result::Result<BTreeSet<String>, String> {
    let (proposal, _) = proposal.effective(Selection::Provisional)?;
    let mut targets = BTreeSet::new();
    match proposal {
        Proposal::Item(item) => {
            targets.insert(item.target.clone());
        }
        Proposal::Group(group) => {
            targets.extend(
                group
                    .provisional
                    .iter()
                    .map(|field| field.field.item.clone()),
            );
            targets.extend(
                group
                    .members
                    .iter()
                    .flat_map(|member| member.owners.iter().cloned()),
            );
        }
        Proposal::File(file) => {
            if file.path.starts_with("assets/") {
                if let Some(owner) = file.path.split('/').nth(2) {
                    targets.insert(owner.to_owned());
                }
            } else {
                return Ok(included.clone());
            }
        }
        Proposal::Resolution(_) => return Err("Unresolved nested decision".into()),
    }
    if targets.contains(publication) {
        return Ok(included.clone());
    }
    Ok(targets.intersection(included).cloned().collect())
}
