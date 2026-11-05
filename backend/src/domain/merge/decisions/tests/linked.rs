// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::domain::merge::{groups, validation::Snapshots};

fn materialize(candidate: ItemCandidate, before: &Value) -> Value {
    match candidate {
        ItemCandidate::Existing(value) => value,
        ItemCandidate::Changed { fields, .. } => {
            let mut value = before.clone();
            value.as_object_mut().unwrap().retain(|key, _| {
                matches!(
                    key.as_str(),
                    "revision" | "createdAt" | "modifiedAt" | "analytics"
                )
            });
            value
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            value
        }
        ItemCandidate::Deleted => panic!("Fixture must preserve items"),
    }
}

fn group(base: &[Value], left: &[Value], right: &[Value], candidate: &[Value]) -> (Value, Memory) {
    let base = snapshot(base);
    let left = snapshot(left);
    let right = snapshot(right);
    let candidate = snapshot(candidate);
    let changed = base
        .items
        .keys()
        .chain(left.items.keys())
        .chain(right.items.keys())
        .cloned()
        .collect();
    let plan = groups::repair(
        &context(),
        &config(),
        &changed,
        Snapshots {
            base: &base,
            left: &left,
            right: &right,
            candidate: &candidate,
        },
    )
    .unwrap();
    assert_eq!(plan.groups.len(), 1);
    let mut items = candidate.items;
    for (id, item) in plan.items {
        let before = items.get(&id).or_else(|| left.items.get(&id)).unwrap();
        items.insert(id.clone(), materialize(item, before));
    }
    let decision = prepare(
        Proposal::Group(Box::new(plan.groups.into_iter().next().unwrap())),
        None,
        "UTC",
    )
    .unwrap();
    (decision, snapshot(&items.into_values().collect::<Vec<_>>()))
}

fn property() -> (Value, Memory, String) {
    let declaration = named("property", "score");
    let property_id = declaration["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["score"] = json!("text");
    let base = [declaration.clone(), note.clone()];
    let mut numeric = declaration.clone();
    numeric["content"]["valueType"] = json!("number");
    let mut numeric_note = note.clone();
    numeric_note["meta"]["score"] = json!(42);
    let left = [numeric.clone(), numeric_note];
    let right = [declaration, note.clone()];
    let (decision, capture) = group(&base, &left, &right, &[numeric, note]);
    (decision, capture, property_id)
}

fn branch(decision: &Value, commit: &str) -> Selection {
    let record = read(decision).unwrap();
    let sources = match record.proposal {
        Proposal::Item(d) => d.alternatives.map(|a| a.source),
        Proposal::Group(d) => d.alternatives.map(|a| a.source),
        Proposal::File(d) => d.alternatives.map(|a| a.source),
        Proposal::Resolution(d) => d.alternatives.map(|a| a.source),
    };
    if matches!(&sources[0], Source::Commit { id, .. } if id == commit) {
        Selection::First
    } else {
        Selection::Second
    }
}

#[test]
fn property_resolution_changes_schema_and_values_together_preserving_unrelated_edits() {
    let (decision, mut capture, property_id) = property();
    capture.items.get_mut(&id(10)).unwrap()["meta"]["title"] = json!("Recent title");
    capture.items.get_mut(&id(10)).unwrap()["revision"] = json!("recent-revision");
    let expected = preview(&decision, &capture).unwrap();
    assert!(expected.targets.contains_key(&property_id));
    assert!(expected.targets.contains_key(&id(10)));
    let plan = resolve(
        &decision,
        &expected,
        resolution(branch(&decision, "a")),
        &capture,
        &config(),
    )
    .unwrap();
    assert_eq!(plan.targets.len(), 2);
    assert_eq!(plan.targets[&property_id]["content"]["valueType"], "number");
    assert_eq!(plan.targets[&id(10)]["meta"]["score"], 42);
    assert_eq!(plan.targets[&id(10)]["meta"]["title"], "Recent title");
    assert_eq!(
        plan.targets[&id(10)]["content"],
        capture.items[&id(10)]["content"]
    );
    assert!(read(&plan.decision).unwrap().resolution.is_some());
    assert_eq!(capture.items[&id(10)]["meta"]["score"], "text");
}

#[test]
fn incomplete_or_stale_preview_cannot_resolve_a_linked_group() {
    let (decision, mut capture, property_id) = property();
    let expected = preview(&decision, &capture).unwrap();
    let mut incomplete = expected.clone();
    incomplete.targets.remove(&property_id);
    assert!(matches!(
        resolve(
            &decision,
            &incomplete,
            resolution(Selection::Provisional),
            &capture,
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
    capture.items.get_mut(&id(10)).unwrap()["revision"] = json!("changed");
    assert!(matches!(
        resolve(
            &decision,
            &expected,
            resolution(Selection::Provisional),
            &capture,
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
    let fresh = preview(&decision, &capture).unwrap();
    capture.items.get_mut(&id(10)).unwrap()["meta"]["score"] = json!("authored change");
    assert!(matches!(
        resolve(
            &decision,
            &fresh,
            resolution(Selection::Provisional),
            &capture,
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn new_dependencies_block_resolution_and_reverse_false_positives_do_not() {
    use crate::domain::project::candidate::{Dependency, NamedKind};
    let (decision, mut capture, _) = property();
    let expected = preview(&decision, &capture).unwrap();
    capture.items.insert(id(11), page(11));
    capture
        .incoming
        .entry(Dependency::named(NamedKind::Property, "score"))
        .or_default()
        .insert(id(11));
    assert_eq!(preview(&decision, &capture).unwrap(), expected);
    capture.items.get_mut(&id(11)).unwrap()["meta"]["score"] = json!("new user");
    assert!(matches!(
        preview(&decision, &capture),
        Err(ResolveError::Stale(_))
    ));
    assert!(matches!(
        resolve(
            &decision,
            &expected,
            resolution(Selection::First),
            &capture,
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
}

#[test]
fn unavailable_reverse_index_stops_linked_resolution() {
    let (decision, mut capture, _) = property();
    let expected = preview(&decision, &capture).unwrap();
    capture.fail_lookup = true;
    assert!(matches!(
        resolve(
            &decision,
            &expected,
            resolution(Selection::First),
            &capture,
            &config()
        ),
        Err(ResolveError::Invalid(_))
    ));
}

fn tasks() -> (Value, Memory) {
    let note = page(10);
    let task = record(
        &id(11),
        "task",
        json!({"noteId":id(10),"text":"Preserve this task"}),
    );
    let mut archived = note.clone();
    archived["meta"]["state"] = json!("archived");
    group(
        std::slice::from_ref(&note),
        &[note.clone(), task.clone()],
        std::slice::from_ref(&archived),
        &[archived.clone(), task],
    )
}

#[test]
fn restoring_page_restores_task_state_and_keeps_recent_task_text() {
    let (decision, mut capture) = tasks();
    capture.items.get_mut(&id(11)).unwrap()["content"]["text"] = json!("Edited after merge");
    let expected = preview(&decision, &capture).unwrap();
    let plan = resolve(
        &decision,
        &expected,
        resolution(branch(&decision, "a")),
        &capture,
        &config(),
    )
    .unwrap();
    assert_eq!(plan.targets.len(), 2);
    assert_eq!(plan.targets[&id(10)]["meta"]["state"], "active");
    assert_eq!(plan.targets[&id(11)]["meta"]["state"], "active");
    assert_eq!(
        plan.targets[&id(11)]["content"]["text"],
        "Edited after merge"
    );
}

#[test]
fn absent_task_alternative_preserves_task_and_converges_with_keep_provisional() {
    let (decision, capture) = tasks();
    let expected = preview(&decision, &capture).unwrap();
    let absent = resolve(
        &decision,
        &expected,
        resolution(branch(&decision, "b")),
        &capture,
        &config(),
    )
    .unwrap();
    let provisional = resolve(
        &decision,
        &expected,
        resolution(Selection::Provisional),
        &capture,
        &config(),
    )
    .unwrap();
    assert!(absent.targets.is_empty());
    assert!(provisional.targets.is_empty());
    assert!(
        merge(
            Some(&decision),
            Some(&absent.decision),
            Some(&provisional.decision)
        )
        .is_ok()
    );
}

fn ordered(numbers: &[usize]) -> Value {
    let mut note = page(10);
    note["content"]["document"]["content"] = Value::Array(
        numbers
            .iter()
            .map(|number| page(*number)["content"]["document"]["content"][0].clone())
            .collect(),
    );
    note
}

fn ordering() -> (Value, Value) {
    let base = ordered(&[1, 2, 3, 4, 5]);
    let left = ordered(&[3, 1, 2, 4, 5, 6]);
    let right = ordered(&[1, 2, 4, 5, 3, 7]);
    let merged = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap();
    let decision = merged
        .decisions
        .into_iter()
        .find(|d| d.kind == DecisionKind::Position)
        .unwrap();
    (
        prepare(Proposal::Item(Box::new(decision)), None, "UTC").unwrap(),
        materialize(merged.candidate, &right),
    )
}

#[test]
fn ordering_preserves_both_branches_insertions_and_current_block_edits() {
    let (decision, mut note) = ordering();
    for block in note["content"]["document"]["content"]
        .as_array_mut()
        .unwrap()
    {
        block["content"][0]["text"] = json!("Edited after merge");
        block["attrs"]["labels"] = json!(["recent"]);
    }
    let capture = snapshot(std::slice::from_ref(&note));
    let plan = resolve(
        &decision,
        &expected(&decision, &note),
        resolution(branch(&decision, "a")),
        &capture,
        &config(),
    )
    .unwrap();
    let blocks = plan.targets[&id(10)]["content"]["document"]["content"]
        .as_array()
        .unwrap();
    assert_eq!(blocks.len(), 7);
    assert_eq!(blocks[0]["attrs"]["id"], id(1003));
    assert!(blocks.iter().any(|block| block["attrs"]["id"] == id(1007)));
    assert!(
        blocks
            .iter()
            .all(|block| block["content"][0]["text"] == "Edited after merge"
                && block["attrs"]["labels"] == json!(["recent"]))
    );
}

#[test]
fn accepting_completed_provisional_order_does_not_rewrite_the_note() {
    let (decision, note) = ordering();
    let plan = resolve(
        &decision,
        &expected(&decision, &note),
        resolution(Selection::Provisional),
        &snapshot(std::slice::from_ref(&note)),
        &config(),
    )
    .unwrap();
    assert!(plan.targets.is_empty());
    let same = resolve(
        &decision,
        &expected(&decision, &note),
        resolution(branch(&decision, "b")),
        &snapshot(&[note]),
        &config(),
    )
    .unwrap();
    assert!(merge(Some(&decision), Some(&plan.decision), Some(&same.decision)).is_ok());
}

#[test]
fn newly_added_sequence_entries_cause_stale_decision_without_dropping_content() {
    let (decision, mut note) = ordering();
    note["content"]["document"]["content"]
        .as_array_mut()
        .unwrap()
        .push(page(8)["content"]["document"]["content"][0].clone());
    let original = note.clone();
    assert!(matches!(
        resolve(
            &decision,
            &expected(&decision, &note),
            resolution(branch(&decision, "a")),
            &snapshot(std::slice::from_ref(&note)),
            &config()
        ),
        Err(ResolveError::Stale(_))
    ));
    assert_eq!(original, note);
}

#[test]
fn linked_resolution_commits_every_target_and_tombstone_as_one_group() {
    let (decision, mut capture, property_id) = property();
    for (index, target) in [id(10), property_id.clone()].iter().enumerate() {
        capture.items.get_mut(target).unwrap()["revision"] = json!(id(900 + index));
    }
    let expected = preview(&decision, &capture).unwrap();
    let plan = resolve(
        &decision,
        &expected,
        resolution(branch(&decision, "a")),
        &capture,
        &config(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    crate::domain::project::config::create_project_config(root.path(), config()).unwrap();
    let store = crate::store::Store::shared(root.path()).unwrap();
    let initial = [
        capture.items[&id(10)].clone(),
        capture.items[&property_id].clone(),
        decision,
    ];
    let initial = initial
        .into_iter()
        .map(|mut value| {
            value["analytics"] = crate::store::commands::assemble_item(&value, Some(&value), "UTC")
                .unwrap()["analytics"]
                .clone();
            crate::store::mutations::PreparedChange::new(value, None).unwrap()
        })
        .collect::<Vec<_>>();
    store
        .write(move |tx| crate::store::mutations::apply(tx, &initial, &[], None))
        .unwrap();
    let cursor = store.read_changes(None).unwrap().cursor;
    let mut values: Vec<_> = plan.targets.into_values().collect();
    values.push(plan.decision);
    let updates = store
        .read(|tx| {
            values
                .iter()
                .map(|value| {
                    let previous =
                        crate::store::item::read_header(tx, value["id"].as_str().unwrap())?
                            .unwrap();
                    crate::store::mutations::PreparedChange::new(
                        value.clone(),
                        previous.value["revision"].as_str().map(str::to_owned),
                    )
                })
                .collect::<crate::store::Result<Vec<_>>>()
        })
        .unwrap();
    store
        .write(move |tx| crate::store::mutations::apply(tx, &updates, &[], None))
        .unwrap();
    let committed = store.read_changes(Some(&cursor)).unwrap();
    assert_eq!(committed.batches.len(), 1);
    assert_eq!(committed.batches[0].items.len(), 3);
    for value in values {
        assert_eq!(
            crate::store::snapshots::read_project_item(root.path(), value["id"].as_str().unwrap())
                .unwrap()
                .unwrap(),
            value
        );
    }
}

#[test]
fn workspace_widget_order_changes_without_replacing_widget_configuration() {
    let base = workspace(10);
    let mut left = base.clone();
    let moved = left["content"]["widgets"].as_array_mut().unwrap().remove(2);
    left["content"]["widgets"]
        .as_array_mut()
        .unwrap()
        .insert(0, moved);
    let mut right = base.clone();
    let moved = right["content"]["widgets"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    right["content"]["widgets"]
        .as_array_mut()
        .unwrap()
        .push(moved);
    let merged = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap();
    let proposal = merged
        .decisions
        .into_iter()
        .find(|d| d.kind == DecisionKind::Position)
        .unwrap();
    let decision = prepare(Proposal::Item(Box::new(proposal)), None, "UTC").unwrap();
    let current = materialize(merged.candidate, &right);
    let plan = resolve(
        &decision,
        &expected(&decision, &current),
        resolution(branch(&decision, "a")),
        &snapshot(&[current]),
        &config(),
    )
    .unwrap();
    assert_eq!(plan.targets[&id(10)]["content"], left["content"]);
}
