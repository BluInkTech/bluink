// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;

#[test]
fn whole_item_deletion_requires_an_unchanged_item_and_valid_dependencies() {
    let base = page(10);
    let mut edited = base.clone();
    edited["meta"]["title"] = json!("Edited");
    let merged = merge_item(&context(), Some(&base), Some(&edited), None).unwrap();
    let proposal = merged.decisions.into_iter().next().unwrap();
    let selection = if proposal.alternatives[0].value == Fragment::Missing {
        Selection::First
    } else {
        Selection::Second
    };
    let decision = prepare(Proposal::Item(Box::new(proposal)), None, "UTC").unwrap();
    let plan = resolve(
        &decision,
        &expected(&decision, &edited),
        resolution(selection),
        &snapshot(std::slice::from_ref(&edited)),
        &config(),
    )
    .unwrap();
    assert_eq!(plan.deleted, std::collections::BTreeSet::from([id(10)]));
    let missing = snapshot(&[]);
    let restore_preview = preview(&decision, &missing).unwrap();
    assert_eq!(restore_preview.targets[&id(10)], "missing");
    let restore = resolve(
        &decision,
        &restore_preview,
        resolution(Selection::Provisional),
        &missing,
        &config(),
    )
    .unwrap();
    assert_eq!(restore.targets[&id(10)]["meta"]["title"], "Edited");
    let task = record(&id(20), "task", json!({"noteId":id(10),"text":"Dependent"}));
    assert!(
        resolve(
            &decision,
            &expected(&decision, &edited),
            resolution(selection),
            &snapshot(&[edited.clone(), task]),
            &config()
        )
        .is_err()
    );
    edited["meta"]["title"] = json!("New author edit");
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &edited),
            resolution(selection),
            &snapshot(&[edited]),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn cell_decisions_resolve_only_while_the_recorded_row_structure_survives() {
    let mut base = page(10);
    let paragraph = base["content"]["document"]["content"][0].clone();
    base["content"]["document"]["content"] = json!([{"type":"table","attrs":{"id":id(30),"columnWidthsPercent":[100]},"content":[{"type":"table_row","attrs":{"id":id(31)},"content":[{"type":"table_cell","content":[paragraph]}]}]}]);
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["document"]["content"][0]["content"][0]["content"][0]["content"][0]["content"]
        [0]["text"] = json!("Left");
    right["content"]["document"]["content"][0]["content"][0]["content"][0]["content"][0]["content"]
        [0]["text"] = json!("Right");
    let merged = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap();
    let proposal = merged.decisions.into_iter().next().unwrap();
    assert!(
        proposal
            .location
            .0
            .iter()
            .any(|segment| matches!(segment, crate::domain::merge::model::Segment::Cell(_)))
    );
    let selection = if proposal.alternatives[0].source == context().left {
        Selection::First
    } else {
        Selection::Second
    };
    let decision = prepare(Proposal::Item(Box::new(proposal)), None, "UTC").unwrap();
    let plan = resolve(
        &decision,
        &expected(&decision, &right),
        resolution(selection),
        &snapshot(std::slice::from_ref(&right)),
        &config(),
    )
    .unwrap();
    assert_eq!(plan.targets[&id(10)]["content"], left["content"]);
    right["content"]["document"]["content"][0]["content"][0]["content"][0]["type"] =
        json!("table_header");
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &right),
            resolution(selection),
            &snapshot(&[right]),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn resolved_choice_replays_into_regenerated_conflict_without_reopening_it() {
    let decision = prepare(title_proposal(), None, "UTC").unwrap();
    let target = title_target();
    let closed = closed(&decision, &target, Selection::First);
    let replayed = crate::store::commands::with_authoring_values(|values| {
        super::super::replay(
            &closed,
            &snapshot(std::slice::from_ref(&target)),
            &config(),
            values,
        )
    })
    .unwrap();
    assert_eq!(replayed[&id(10)]["meta"]["title"], "First");
    let capture = snapshot(&[replayed[&id(10)].clone()]);
    assert!(
        crate::store::commands::with_authoring_values(|values| super::super::replay(
            &closed,
            &capture,
            &config(),
            values
        ))
        .unwrap()
        .is_empty()
    );
}

#[test]
fn competing_resolutions_create_a_stable_followup_and_update_both_tombstones() {
    let open = prepare(title_proposal(), None, "UTC").unwrap();
    let target = title_target();
    let first = closed(&open, &target, Selection::First);
    let second = closed(&open, &target, Selection::Second);
    let mut context = context();
    context.target = open["id"].as_str().unwrap().to_owned();
    let merged = merge_item(&context, Some(&open), Some(&first), Some(&second)).unwrap();
    let mut reverse = context.clone();
    std::mem::swap(&mut reverse.left, &mut reverse.right);
    assert_eq!(
        merged,
        merge_item(&reverse, Some(&open), Some(&second), Some(&first)).unwrap()
    );
    assert_eq!(merged.followups.len(), 1);
    let followup = prepare(merged.followups[0].clone(), None, "UTC").unwrap();
    let ItemCandidate::Existing(original) = merged.candidate else {
        panic!("Missing original tombstone");
    };
    let current = snapshot(&[target, original]);
    let preview = preview(&followup, &current).unwrap();
    assert!(preview.targets.contains_key(open["id"].as_str().unwrap()));
    let plan = resolve(
        &followup,
        &preview,
        resolution(Selection::First),
        &current,
        &config(),
    )
    .unwrap();
    let record = read(&plan.decision).unwrap();
    assert!(record.resolution.is_some());
    let original = read(&plan.targets[open["id"].as_str().unwrap()]).unwrap();
    assert_eq!(
        original
            .proposal
            .selected(original.resolution.unwrap().selection)
            .unwrap(),
        record.proposal.selected(Selection::First).unwrap()
    );
    // A later accepted followup supersedes the base intent; reconnecting an
    // unchanged replica must not create another resolution-of-resolution.
    let original_id = open["id"].as_str().unwrap();
    let previous = current.items.get(original_id).unwrap();
    let accepted = &plan.targets[original_id];
    let next = merge_item(&context, Some(previous), Some(accepted), Some(previous)).unwrap();
    assert_eq!(next.followups, []);
    assert_eq!(next.candidate, ItemCandidate::Existing(accepted.clone()));
}

#[test]
fn copied_subtree_remaps_its_internal_anchor_and_retains_external_anchors() {
    let old = id(45);
    let mut node = json!({"type":"blockquote","attrs":{"id":old},"content":[{"type":"paragraph","attrs":{"id":id(46)},"content":[{"type":"text","text":"Local","marks":[{"type":"link","attrs":{"href":format!("#{old}")}}]},{"type":"text","text":"External","marks":[{"type":"link","attrs":{"href":"https://example.com/#outside"}}]},{"type":"text","text":"Block","marks":[{"type":"link","attrs":{"href":format!("#block-{old}")}}]}]}]});
    super::super::both::renew_copy(&mut node, &id(44)).unwrap();
    assert_ne!(node["attrs"]["id"], old);
    let renewed = node["attrs"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        node["content"][0]["content"][0]["marks"][0]["attrs"]["href"],
        format!("#{renewed}")
    );
    assert_eq!(
        node["content"][0]["content"][2]["marks"][0]["attrs"]["href"],
        format!("#block-{renewed}")
    );
    assert_eq!(
        node["content"][0]["content"][1]["marks"][0]["attrs"]["href"],
        "https://example.com/#outside"
    );
}
