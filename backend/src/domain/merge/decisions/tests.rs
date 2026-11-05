// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "Unexpected merge results fail the test immediately."
)]

use super::{
    resolve::{Expected, ResolutionPlan, ResolveError},
    *,
};
use crate::domain::merge::{
    fixtures::*,
    item::{ItemCandidate, merge_item},
    model::{Context, DecisionKind, Location, Merger},
};
use crate::domain::project::item::validate_item as validate_canonical;

fn resolve(
    decision: &Value,
    expected: &Expected,
    resolution: Resolution,
    current: &dyn crate::domain::merge::validation::Snapshot,
    config: &Value,
) -> Result<ResolutionPlan, ResolveError> {
    let mut capture = crate::domain::merge::groups::Overlay::new(current);
    capture.put(decision.clone()).unwrap();
    crate::store::commands::with_authoring_values(|values| {
        super::resolve::resolve(
            decision["id"].as_str().unwrap(),
            expected,
            resolution,
            &capture,
            config,
            values,
        )
    })
}

fn context() -> Context {
    Context {
        target: id(10),
        base_epoch: "commit:base".into(),
        left: Source::Commit {
            id: "a".into(),
            committer_time: 1,
            device: "Laptop".into(),
        },
        right: Source::Commit {
            id: "b".into(),
            committer_time: 2,
            device: "Desktop".into(),
        },
    }
}
fn title_proposal() -> Proposal {
    let context = context();
    let mut merger = Merger::new(&context).unwrap();
    merger
        .conflict(
            &Location::default().field("meta").field("title"),
            DecisionKind::Field,
            Some(&json!("Base")),
            Some(&json!("First")),
            Some(&json!("Second")),
        )
        .unwrap();
    Proposal::Item(Box::new(merger.decisions.remove(0)))
}
fn title_target() -> Value {
    let mut item = page(10);
    item["meta"]["title"] = json!("Second");
    item
}
fn resolution(selection: Selection) -> Resolution {
    Resolution {
        selection,
        resolved_at: "2026-10-31T12:00:00.000Z".into(),
        device: "Laptop".into(),
    }
}
fn expected(decision: &Value, item: &Value) -> Expected {
    Expected {
        decision_revision: decision["revision"].as_str().unwrap().into(),
        targets: std::collections::BTreeMap::from([(
            item["id"].as_str().unwrap().into(),
            item["revision"].as_str().unwrap().into(),
        )]),
    }
}

fn preview(
    decision: &Value,
    current: &dyn crate::domain::merge::validation::Snapshot,
) -> Result<Expected, ResolveError> {
    let mut capture = crate::domain::merge::groups::Overlay::new(current);
    capture.put(decision.clone()).unwrap();
    super::resolve::preview(decision["id"].as_str().unwrap(), &capture)
}
fn closed(decision: &Value, target: &Value, selection: Selection) -> Value {
    resolve(
        decision,
        &expected(decision, target),
        resolution(selection),
        &snapshot(std::slice::from_ref(target)),
        &config(),
    )
    .unwrap()
    .decision
}

#[test]
fn canonical_roundtrip_reuses_open_and_resolved_records_exactly() {
    let proposal = title_proposal();
    let item = prepare(proposal.clone(), None, "UTC").unwrap();
    let bytes = canonical_json::to_vec(&item).unwrap();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    validate_canonical(&parsed, proposal.id()).unwrap();
    assert_eq!(
        prepare(proposal.clone(), Some(&parsed), "UTC").unwrap(),
        item
    );
    let resolved = closed(&item, &title_target(), Selection::First);
    assert!(read(&resolved).unwrap().resolution.is_some());
    assert_eq!(prepare(proposal, Some(&resolved), "UTC").unwrap(), resolved);
}

#[test]
fn malformed_identity_alternatives_and_status_are_rejected() {
    let item = prepare(title_proposal(), None, "UTC").unwrap();
    for changed in [
        "identity",
        "alternative",
        "index",
        "unknown",
        "archive",
        "timestamp",
        "tags",
    ] {
        let mut bad = item.clone();
        match changed {
            "identity" => bad["id"] = json!(id(20)),
            "alternative" => {
                bad["content"]["proposal"]["decision"]["alternatives"][0]["value"] =
                    json!({"presence":"present","value":"changed"});
            }
            "index" => bad["content"]["proposal"]["decision"]["provisional"] = json!(2),
            "unknown" => bad["content"]["proposal"]["decision"]["obsolete"] = json!(true),
            "archive" => bad["meta"]["state"] = json!("archived"),
            "timestamp" => {
                bad["content"]["resolution"] =
                    json!({"selection":"first","resolved_at":"yesterday","device":"Laptop"});
            }
            _ => bad["tags"] = json!(["hidden"]),
        }
        assert!(
            validate_canonical(&bad, bad["id"].as_str().unwrap()).is_err(),
            "{changed}"
        );
    }
}

#[test]
fn resolved_tombstone_dominates_open_and_missing_branches_in_both_orders() {
    let open = prepare(title_proposal(), None, "UTC").unwrap();
    let resolved = closed(&open, &title_target(), Selection::First);
    for (left, right) in [
        (Some(&open), Some(&resolved)),
        (Some(&resolved), Some(&open)),
        (None, Some(&resolved)),
        (None, None),
    ] {
        let mut context = context();
        context.target = open["id"].as_str().unwrap().into();
        let result = merge_item(&context, Some(&resolved), left, right).unwrap();
        assert_eq!(result.candidate, ItemCandidate::Existing(resolved.clone()));
        assert_eq!(result.decisions, []);
    }
}

#[test]
fn agreeing_resolutions_converge_and_competing_intents_stop() {
    let open = prepare(title_proposal(), None, "UTC").unwrap();
    let first = closed(&open, &title_target(), Selection::First);
    let same = closed(&open, &title_target(), Selection::First);
    assert_eq!(
        merge(Some(&open), Some(&first), Some(&same)).unwrap(),
        merge(Some(&open), Some(&same), Some(&first)).unwrap()
    );
    let second = closed(&open, &title_target(), Selection::Second);
    assert!(
        merge(Some(&open), Some(&first), Some(&second))
            .unwrap_err()
            .contains("Competing")
    );
    assert!(
        merge(Some(&open), Some(&second), Some(&first))
            .unwrap_err()
            .contains("Competing")
    );
}

#[test]
fn resolution_preserves_unrelated_edits_and_checks_both_preview_revisions() {
    let decision = prepare(title_proposal(), None, "UTC").unwrap();
    let mut target = title_target();
    target["revision"] = json!("new-revision");
    target["meta"]["summary"] = json!("Unrelated edit after the merge");
    let current = snapshot(std::slice::from_ref(&target));
    let plan = resolve(
        &decision,
        &expected(&decision, &target),
        resolution(Selection::First),
        &current,
        &config(),
    )
    .unwrap();
    let after = plan.targets[&id(10)].clone();
    assert_eq!(after["meta"]["title"], "First");
    assert_eq!(after["meta"]["summary"], target["meta"]["summary"]);
    assert_eq!(after["content"], target["content"]);
    assert_ne!(after["revision"], target["revision"]);
    for stale_decision in [true, false] {
        let mut expected = expected(&decision, &target);
        if stale_decision {
            expected.decision_revision = "old".into();
        } else {
            expected.targets.insert(id(10), "old".into());
        }
        assert!(matches!(
            resolve(
                &decision,
                &expected,
                resolution(Selection::First),
                &current,
                &config()
            ),
            Err(ResolveError::Stale(_))
        ));
    }
    target["meta"]["title"] = json!("New authored title");
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &target),
            resolution(Selection::First),
            &snapshot(&[target]),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn keeping_provisional_closes_decision_without_rewriting_target() {
    let decision = prepare(title_proposal(), None, "UTC").unwrap();
    let target = title_target();
    let plan = resolve(
        &decision,
        &expected(&decision, &target),
        resolution(Selection::Provisional),
        &snapshot(&[target]),
        &config(),
    )
    .unwrap();
    assert!(plan.targets.is_empty());
    assert!(matches!(
        resolve(
            &plan.decision,
            &Expected {
                decision_revision: plan.decision["revision"].as_str().unwrap().into(),
                targets: std::collections::BTreeMap::from([(id(10), "revision-1".into())])
            },
            resolution(Selection::First),
            &snapshot(&[]),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn paragraph_resolution_uses_identity_and_preserves_labels_and_other_blocks() {
    let base = page(10);
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["document"]["content"][0]["content"][0]["text"] = json!("First");
    right["content"]["document"]["content"][0]["content"][0]["text"] = json!("Second");
    let merged = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap();
    let proposal = Proposal::Item(Box::new(merged.decisions[0].clone()));
    let decision = prepare(proposal, None, "UTC").unwrap();
    right["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["recent"]);
    let extra = page(11)["content"]["document"]["content"][0].clone();
    right["content"]["document"]["content"]
        .as_array_mut()
        .unwrap()
        .insert(0, extra.clone());
    let plan = resolve(
        &decision,
        &expected(&decision, &right),
        resolution(Selection::First),
        &snapshot(std::slice::from_ref(&right)),
        &config(),
    )
    .unwrap();
    let after = plan.targets[&id(10)].clone();
    assert_eq!(after["content"]["document"]["content"][0], extra);
    assert_eq!(
        after["content"]["document"]["content"][1]["content"][0]["text"],
        "First"
    );
    assert_eq!(
        after["content"]["document"]["content"][1]["attrs"]["labels"],
        json!(["recent"])
    );
}

#[test]
fn current_property_constraints_can_block_an_old_resolution_without_mutation() {
    let mut property = named("property", "score");
    property["content"]["valueType"] = json!("number");
    let context = context();
    let mut merger = Merger::new(&context).unwrap();
    merger
        .conflict(
            &Location::default().field("meta").field("score"),
            DecisionKind::Field,
            Some(&json!(0)),
            Some(&json!("text")),
            Some(&json!(2)),
        )
        .unwrap();
    let proposal = merger.decisions.remove(0);
    let selection = if proposal.alternatives[0].value == Fragment::Present(json!("text")) {
        Selection::First
    } else {
        Selection::Second
    };
    let decision = prepare(Proposal::Item(Box::new(proposal)), None, "UTC").unwrap();
    let mut target = page(10);
    target["meta"]["score"] = json!(2);
    let current = snapshot(&[target.clone(), property]);
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &target),
            resolution(selection),
            &current,
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
    assert_eq!(current.items[&id(10)], target);
    assert!(read(&decision).unwrap().resolution.is_none());
}

#[test]
fn restored_declarations_can_be_confirmed_but_deletion_keeps_its_uses_safe() {
    let declaration = named("tag", "shared");
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut note = page(10);
    note["tags"] = json!(["shared"]);
    let left = snapshot(&[declaration, note.clone()]);
    let right = snapshot(&[]);
    let candidate = snapshot(std::slice::from_ref(&note));
    let plan = crate::domain::merge::groups::repair(
        &context(),
        &config(),
        &[declaration_id, id(10)].into_iter().collect(),
        crate::domain::merge::validation::Snapshots {
            base: &base,
            left: &left,
            right: &right,
            candidate: &candidate,
        },
    )
    .unwrap();
    let decision = prepare(
        Proposal::Group(Box::new(plan.groups.into_iter().next().unwrap())),
        None,
        "UTC",
    )
    .unwrap();
    validate_canonical(&decision, decision["id"].as_str().unwrap()).unwrap();
    assert!(matches!(
        read(&decision).unwrap().proposal,
        Proposal::Group(_)
    ));
    let expected = preview(&decision, &left).unwrap();
    assert!(
        resolve(
            &decision,
            &expected,
            resolution(Selection::Provisional),
            &left,
            &config()
        )
        .unwrap()
        .targets
        .is_empty()
    );
    assert!(matches!(
        resolve(
            &decision,
            &expected,
            resolution(Selection::First),
            &left,
            &config()
        ),
        Err(ResolveError::Unsupported(_))
    ));
    let mut malformed = decision.clone();
    malformed["content"]["proposal"]["decision"]["provisional"]
        .as_array_mut()
        .unwrap()
        .clear();
    assert!(validate_canonical(&malformed, decision["id"].as_str().unwrap()).is_err());
}

mod linked;
mod policies;

#[test]
fn field_removal_is_distinct_from_null_and_never_reintroduces_old_metadata() {
    let context = context();
    let mut merger = Merger::new(&context).unwrap();
    merger
        .conflict(
            &Location::default().field("meta").field("summary"),
            DecisionKind::DeleteEdit,
            Some(&json!("base")),
            None,
            Some(&json!("edit")),
        )
        .unwrap();
    let proposal = merger.decisions.remove(0);
    let selection = if proposal.alternatives[0].value == Fragment::Missing {
        Selection::First
    } else {
        Selection::Second
    };
    let decision = prepare(Proposal::Item(Box::new(proposal)), None, "UTC").unwrap();
    let mut target = page(10);
    target["meta"]["summary"] = json!("edit");
    let plan = resolve(
        &decision,
        &expected(&decision, &target),
        resolution(selection),
        &snapshot(&[target]),
        &config(),
    )
    .unwrap();
    assert!(
        plan.targets[&id(10)].clone()["meta"]
            .get("summary")
            .is_none()
    );
}

#[test]
fn missing_targets_and_snapshot_failures_leave_decisions_open() {
    let decision = prepare(title_proposal(), None, "UTC").unwrap();
    let target = title_target();
    let mut current = snapshot(std::slice::from_ref(&target));
    current.fail_read = Some(id(10));
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &target),
            resolution(Selection::First),
            &current,
            &config()
        ),
        Err(ResolveError::Invalid(_))
    ));
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &target),
            resolution(Selection::First),
            &snapshot(&[]),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
    assert!(read(&decision).unwrap().resolution.is_none());
}

fn prepare(proposal: Proposal, existing: Option<&Value>, timezone: &str) -> Result<Value, String> {
    crate::store::commands::with_authoring_values(|values| {
        super::prepare(proposal, existing, timezone, values)
    })
}
