// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "Unexpected repair results fail tests immediately."
)]

use super::*;
use crate::domain::merge::fixtures::*;

fn context() -> Context {
    Context {
        target: id(9000),
        base_epoch: "commit:base".into(),
        left: Source::Commit {
            id: "a".into(),
            committer_time: 100,
            device: "Laptop".into(),
        },
        right: Source::Commit {
            id: "b".into(),
            committer_time: 200,
            device: "Desktop".into(),
        },
    }
}
fn fields(candidate: &ItemCandidate) -> &Value {
    match candidate {
        ItemCandidate::Existing(value) | ItemCandidate::Changed { fields: value, .. } => value,
        ItemCandidate::Deleted => panic!("Repairs preserve surviving items"),
    }
}
fn check(
    changed: &[String],
    base: &Memory,
    left: &Memory,
    right: &Memory,
    candidate: &Memory,
) -> RepairPlan {
    repair(
        &context(),
        &config(),
        &changed.iter().cloned().collect(),
        Snapshots {
            base,
            left,
            right,
            candidate,
        },
    )
    .unwrap()
}

#[test]
fn publication_restores_a_missing_explicit_page_with_a_linked_choice() {
    let page = page(10);
    let publication = publication(20, 10);
    let base = snapshot(std::slice::from_ref(&page));
    let left = snapshot(&[page.clone(), publication.clone()]);
    let right = snapshot(&[]);
    let candidate = snapshot(&[publication]);
    let plan = check(&[id(10), id(20)], &base, &left, &right, &candidate);
    assert_eq!(fields(&plan.items[&id(10)])["content"], page["content"]);
    assert_eq!(plan.groups.len(), 1);
    assert!(plan.groups[0].policies.contains(&Policy::RestoreTarget));
    assert_eq!(plan.checked.violations, []);
}

#[test]
fn property_group_keeps_prose_and_preserves_an_absent_field_alternative() {
    let declaration = named("property", "score");
    let property_id = declaration["id"].as_str().unwrap().to_owned();
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut note = page(10);
    note["meta"]["score"] = json!("new value");
    note["meta"]["title"] = json!("Unrelated title");
    let left = snapshot(&[declaration.clone(), note.clone()]);
    let mut changed = declaration;
    changed["content"]["valueType"] = json!("number");
    let right = snapshot(std::slice::from_ref(&changed));
    let candidate = snapshot(&[changed, note.clone()]);
    let plan = check(
        &[property_id.clone(), id(10)],
        &base,
        &left,
        &right,
        &candidate,
    );
    assert_eq!(plan.checked.violations, []);
    assert_eq!(plan.groups.len(), 1);
    let repaired = fields(&plan.items[&id(10)]);
    assert!(repaired["meta"].get("score").is_none());
    assert_eq!(repaired["meta"]["title"], note["meta"]["title"]);
    assert_eq!(repaired["content"], note["content"]);
    let property = Field::new(&id(10), &["meta", "score"]);
    let group = &plan.groups[0];
    assert!(group.alternatives.iter().any(|a| {
        a.fields
            .iter()
            .any(|v| v.field == property && v.value == Fragment::Missing)
    }));
    assert!(group.alternatives.iter().any(|a| {
        a.fields
            .iter()
            .any(|v| v.field == property && v.value == Fragment::Present(json!("new value")))
    }));
    assert!(
        group
            .provisional
            .iter()
            .all(|value| value.field.item == property_id || value.field.path == ["meta", "score"])
    );
    let mut swapped = context();
    std::mem::swap(&mut swapped.left, &mut swapped.right);
    assert_eq!(
        plan,
        repair(
            &swapped,
            &config(),
            &[property_id, id(10)].into_iter().collect(),
            Snapshots {
                base: &base,
                left: &right,
                right: &left,
                candidate: &candidate
            }
        )
        .unwrap()
    );
}

#[test]
fn choosing_the_other_property_schema_keeps_its_values_and_other_metadata() {
    let declaration = named("property", "score");
    let property_id = declaration["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["score"] = json!("text");
    let base = snapshot(&[declaration.clone(), note.clone()]);
    let mut numeric = declaration.clone();
    numeric["content"]["valueType"] = json!("number");
    numeric["content"]["color"] = json!("blue");
    let mut number_note = note.clone();
    number_note["meta"]["score"] = json!(42);
    let left = snapshot(&[numeric.clone(), number_note]);
    let right = snapshot(&[declaration, note.clone()]);
    let candidate = snapshot(&[numeric, note.clone()]);
    let plan = check(
        std::slice::from_ref(&property_id),
        &base,
        &left,
        &right,
        &candidate,
    );
    assert_eq!(
        fields(&plan.items[&property_id])["content"]["valueType"],
        "text"
    );
    assert_eq!(
        fields(&plan.items[&property_id])["content"]["color"],
        "blue"
    );
    assert!(!plan.items.contains_key(&id(10)));
}

#[test]
fn deleted_declaration_restores_surviving_edits_and_records_missing_alternative() {
    for kind in ["tag", "label", "property", "snippet"] {
        let declaration = named(kind, "shared");
        let declaration_id = declaration["id"].as_str().unwrap().to_owned();
        let base = snapshot(std::slice::from_ref(&declaration));
        let mut edited = declaration;
        edited["content"]["description"] = json!("Surviving edit");
        let mut note = page(10);
        match kind {
            "tag" => note["tags"] = json!(["shared"]),
            "label" => {
                note["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["shared"]);
            }
            "property" => note["meta"]["shared"] = json!("Value"),
            _ => {
                note["content"]["document"]["content"] =
                    json!([{"type":"embed","attrs":{"id":id(1010),"snippetId":"shared"}}]);
            }
        }
        let left = snapshot(&[edited, note.clone()]);
        let right = snapshot(&[]);
        let candidate = snapshot(&[note]);
        let plan = check(
            &[declaration_id.clone(), id(10)],
            &base,
            &left,
            &right,
            &candidate,
        );
        assert_eq!(
            fields(&plan.items[&declaration_id])["content"]["description"],
            "Surviving edit"
        );
        assert_eq!(plan.groups.len(), 1);
        assert!(
            plan.groups[0]
                .alternatives
                .iter()
                .any(|a| a.fields.iter().any(|v| v.value == Fragment::Missing))
        );
        assert!(!plan.items.contains_key(&id(10)));
    }
}

#[test]
fn task_repair_archives_all_children_without_replacing_their_text() {
    let note = page(10);
    let base = snapshot(std::slice::from_ref(&note));
    let task = record(&id(11), "task", json!({"noteId":id(10),"text":"New task"}));
    let second = record(
        &id(12),
        "task",
        json!({"noteId":id(10),"text":"Another task"}),
    );
    let left = snapshot(&[note.clone(), task.clone(), second.clone()]);
    let mut archived = note;
    archived["meta"]["state"] = json!("archived");
    let right = snapshot(std::slice::from_ref(&archived));
    let candidate = snapshot(&[archived, task.clone(), second.clone()]);
    let plan = check(&[id(10), id(11), id(12)], &base, &left, &right, &candidate);
    assert_eq!(plan.groups.len(), 1);
    for task in [task, second] {
        let result = fields(&plan.items[task["id"].as_str().unwrap()]);
        assert_eq!(result["content"], task["content"]);
        assert_eq!(result["meta"]["state"], "archived");
    }
    assert!(!plan.items.contains_key(&id(10)));
    assert!(plan.groups[0].alternatives.iter().any(|a| {
        a.fields
            .iter()
            .any(|v| v.field.item == id(11) && v.value == Fragment::Missing)
    }));
    assert_eq!(plan.checked.violations, []);
}

#[test]
fn unsupported_missing_parent_and_default_workspace_return_no_plan() {
    let note = page(10);
    let task = record(&id(11), "task", json!({"noteId":id(10),"text":"Preserve"}));
    let base = snapshot(&[note, task.clone()]);
    let candidate = snapshot(&[task]);
    assert!(
        repair(
            &context(),
            &config(),
            &std::iter::once(id(10)).collect(),
            Snapshots {
                base: &base,
                left: &base,
                right: &candidate,
                candidate: &candidate
            }
        )
        .is_err()
    );
    let mut candidate = snapshot(&[]);
    candidate.items.remove(&id(1));
    assert!(
        repair(
            &context(),
            &config(),
            &BTreeSet::new(),
            Snapshots {
                base: &base,
                left: &base,
                right: &candidate,
                candidate: &candidate
            }
        )
        .is_err()
    );
}

#[test]
fn independent_property_groups_do_not_include_unrelated_fields() {
    let a = named("property", "first");
    let b = named("property", "second");
    let mut note = page(10);
    note["meta"]["first"] = json!("a");
    note["meta"]["second"] = json!("b");
    let base = snapshot(&[a.clone(), b.clone(), note.clone()]);
    let right = snapshot(&[a.clone(), b.clone(), note.clone()]);
    let mut a2 = a.clone();
    a2["content"]["valueType"] = json!("number");
    let mut b2 = b.clone();
    b2["content"]["valueType"] = json!("number");
    let mut number_note = note.clone();
    number_note["meta"]["first"] = json!(1);
    number_note["meta"]["second"] = json!(2);
    let left = snapshot(&[a2.clone(), b2.clone(), number_note]);
    let candidate = snapshot(&[a2, b2, note]);
    let plan = check(
        &[
            a["id"].as_str().unwrap().into(),
            b["id"].as_str().unwrap().into(),
        ],
        &base,
        &left,
        &right,
        &candidate,
    );
    assert_eq!(plan.groups.len(), 2);
    assert!(!plan.items.contains_key(&id(10)));
    assert!(plan.groups.iter().all(|group| {
        group
            .provisional
            .iter()
            .filter(|v| v.field.item == id(10))
            .count()
            == 1
    }));
}

#[test]
fn group_identity_ignores_revisions_but_tracks_the_base_epoch() {
    let declaration = named("tag", "shared");
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["tags"] = json!(["shared"]);
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut left = snapshot(&[declaration, note.clone()]);
    let right = snapshot(&[]);
    let candidate = snapshot(&[note]);
    let changed: BTreeSet<_> = [declaration_id.clone(), id(10)].into_iter().collect();
    let first = repair(
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
    left.items.get_mut(&declaration_id).unwrap()["revision"] = json!("Another revision");
    let second = repair(
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
    assert_eq!(first.groups[0].id, second.groups[0].id);
    let mut context = context();
    context.base_epoch = "commit:next".into();
    assert_ne!(
        first.groups[0].id,
        repair(
            &context,
            &config(),
            &changed,
            Snapshots {
                base: &base,
                left: &left,
                right: &right,
                candidate: &candidate
            }
        )
        .unwrap()
        .groups[0]
            .id
    );
}

#[test]
fn overlay_removes_stale_references_and_exposes_new_ones() {
    let mut note = page(10);
    note["meta"]["first"] = json!("old");
    let source = snapshot(std::slice::from_ref(&note));
    let mut overlay = Overlay::new(&source);
    note["meta"].as_object_mut().unwrap().remove("first");
    note["meta"]["second"] = json!("new");
    overlay.put(note).unwrap();
    assert_eq!(
        overlay
            .dependents(&Dependency::named(NamedKind::Property, "first"))
            .unwrap(),
        BTreeSet::new()
    );
    assert_eq!(
        overlay
            .dependents(&Dependency::named(NamedKind::Property, "second"))
            .unwrap(),
        std::iter::once(id(10)).collect()
    );
}

#[test]
fn archived_declaration_preserves_candidate_edits_and_only_repairs_state() {
    let declaration = named("tag", "shared");
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut note = page(10);
    note["tags"] = json!(["shared"]);
    let left = snapshot(&[declaration.clone(), note.clone()]);
    let mut archived = declaration;
    archived["meta"]["state"] = json!("archived");
    archived["content"]["description"] = json!("Keep candidate edits");
    let right = snapshot(std::slice::from_ref(&archived));
    let candidate = snapshot(&[archived, note]);
    let plan = check(
        &[declaration_id.clone(), id(10)],
        &base,
        &left,
        &right,
        &candidate,
    );
    let result = fields(&plan.items[&declaration_id]);
    assert_eq!(result["meta"]["state"], "active");
    assert_eq!(result["content"]["description"], "Keep candidate edits");
    assert_eq!(
        plan.groups[0].provisional,
        [FieldValue {
            field: Field::new(&declaration_id, &["meta", "state"]),
            value: Fragment::Present(json!("active")),
        }]
    );
    assert_eq!(
        candidate.items[&declaration_id]["meta"]["state"],
        "archived"
    );
}

#[test]
fn restoration_and_property_repair_combine_and_are_idempotent() {
    let declaration = named("property", "score");
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["score"] = json!("Text");
    let base = snapshot(&[declaration.clone(), note.clone()]);
    let left = snapshot(&[declaration.clone(), note.clone()]);
    let mut archived = declaration;
    archived["meta"]["state"] = json!("archived");
    archived["content"]["valueType"] = json!("number");
    let right = snapshot(std::slice::from_ref(&archived));
    let candidate = snapshot(&[archived, note.clone()]);
    let changed = [declaration_id.clone(), id(10)];
    let plan = check(&changed, &base, &left, &right, &candidate);
    assert_eq!(plan.groups.len(), 1);
    assert_eq!(
        plan.groups[0].policies,
        [Policy::RestoreDeclaration, Policy::PropertyConstraint]
            .into_iter()
            .collect()
    );
    assert!(!plan.items.contains_key(&id(10)));
    assert_eq!(
        fields(&plan.items[&declaration_id])["content"]["valueType"],
        "text"
    );
    let ItemCandidate::Existing(restored) = &plan.items[&declaration_id] else {
        panic!("Restoration matches the surviving branch");
    };
    let repaired = snapshot(&[restored.clone(), note]);
    let second = check(&changed, &base, &left, &right, &repaired);
    assert!(second.items.is_empty());
    assert_eq!(second.groups, []);
    // Persistence must carry the first plan's decision; revalidation must not
    // invent another one once the candidate's relationships are already valid.
    assert_eq!(second.checked.violations, []);
}

#[test]
fn option_removal_repairs_schema_and_values_together() {
    let mut declaration = named("property", "status");
    declaration["content"]["valueType"] = json!("select");
    declaration["content"]["options"] = json!(["draft", "ready", "unused"]);
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["status"] = json!("draft");
    let base = snapshot(&[declaration.clone(), note.clone()]);
    let mut edited = note.clone();
    edited["meta"]["status"] = json!("ready");
    let left = snapshot(&[declaration.clone(), edited.clone()]);
    declaration["content"]["options"] = json!(["draft"]);
    let right = snapshot(&[declaration.clone(), note]);
    let candidate = snapshot(&[declaration, edited]);
    let plan = check(
        &[declaration_id.clone(), id(10)],
        &base,
        &left,
        &right,
        &candidate,
    );
    assert_eq!(plan.groups.len(), 1);
    assert!(!plan.items.contains_key(&id(10)));
    assert_eq!(
        fields(&plan.items[&declaration_id])["content"]["options"],
        json!(["draft", "ready"])
    );
    assert!(plan.groups[0].alternatives.iter().any(|a| {
        a.fields.iter().any(|v| {
            v.field == Field::new(&id(10), &["meta", "status"])
                && v.value == Fragment::Present(json!("ready"))
        })
    }));
}

#[test]
fn overlapping_groups_combine_transitively_and_keep_disjoint_fields_separate() {
    let unit = |fields: &[Field]| Unit {
        fields: fields.iter().cloned().collect(),
        ..Unit::default()
    };
    let a = Field::new(&id(10), &["meta", "first"]);
    let b = Field::new(&id(20), &["content", "valueType"]);
    let c = Field::new(&id(30), &["meta", "state"]);
    let d = Field::new(&id(40), &["meta", "other"]);
    let groups = combine(vec![
        unit(&[a.clone(), b.clone()]),
        unit(&[b, c.clone()]),
        unit(&[Field::new(&id(20), &[])]),
        unit(std::slice::from_ref(&d)),
    ]);
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[0].fields,
        [a, Field::new(&id(20), &[]), c].into_iter().collect()
    );
    assert_eq!(groups[1].fields, std::iter::once(d).collect());
}

#[test]
fn multi_select_preserves_used_options_from_both_branches() {
    let mut declaration = named("property", "status");
    declaration["content"]["valueType"] = json!("multiSelect");
    declaration["content"]["options"] = json!(["draft", "other", "ready"]);
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut left_note = page(10);
    left_note["meta"]["status"] = json!(["draft"]);
    let mut right_note = page(11);
    right_note["meta"]["status"] = json!(["ready"]);
    let mut left_declaration = declaration.clone();
    left_declaration["content"]["options"] = json!(["draft", "other"]);
    let left = snapshot(&[left_declaration, left_note.clone()]);
    let mut right_declaration = declaration.clone();
    right_declaration["content"]["options"] = json!(["other", "ready"]);
    let right = snapshot(&[right_declaration, right_note.clone()]);
    declaration["content"]["options"] = json!(["other"]);
    let candidate = snapshot(&[declaration, left_note, right_note]);
    let changed = [declaration_id.clone(), id(10), id(11)];
    let plan = check(&changed, &base, &left, &right, &candidate);
    assert_eq!(plan.items.len(), 1);
    assert_eq!(
        fields(&plan.items[&declaration_id])["content"]["options"],
        json!(["other", "ready", "draft"])
    );
    assert_eq!(plan.groups.len(), 1);
    let mut swapped = context();
    std::mem::swap(&mut swapped.left, &mut swapped.right);
    assert_eq!(
        plan,
        repair(
            &swapped,
            &config(),
            &changed.into_iter().collect(),
            Snapshots {
                base: &base,
                left: &right,
                right: &left,
                candidate: &candidate,
            }
        )
        .unwrap()
    );
}

#[test]
fn failure_after_a_local_repair_returns_no_partial_plan_or_source_mutation() {
    let mut declaration = named("property", "score");
    let declaration_id = declaration["id"].as_str().unwrap().to_owned();
    let base = snapshot(std::slice::from_ref(&declaration));
    let mut note = page(10);
    note["meta"]["score"] = json!("text");
    declaration["content"]["valueType"] = json!("number");
    // No branch has an active property schema. Restoring the declaration from
    // this archived source succeeds locally, but the follow-up value repair
    // cannot choose a valid branch and must discard the entire plan.
    declaration["meta"]["state"] = json!("archived");
    let left = snapshot(std::slice::from_ref(&declaration));
    let right = snapshot(&[]);
    let candidate = snapshot(&[note]);
    let original = candidate.items.clone();
    let error = repair(
        &context(),
        &config(),
        &[declaration_id, id(10)].into_iter().collect(),
        Snapshots {
            base: &base,
            left: &left,
            right: &right,
            candidate: &candidate,
        },
    )
    .unwrap_err();
    assert!(
        error.contains("Neither branch has an active property"),
        "{error}"
    );
    assert_eq!(candidate.items, original);
}

#[test]
fn valid_candidates_do_not_create_decisions_or_rewrites() {
    let source = snapshot(&[page(10)]);
    let plan = check(&[id(10)], &source, &source, &source, &source);
    assert!(plan.items.is_empty());
    assert_eq!(plan.groups, []);
    assert!(source.queries.borrow().is_empty());
}

#[test]
fn identical_alternatives_have_stable_order_and_missing_is_not_null() {
    let mut note = page(10);
    note["meta"]["score"] = Value::Null;
    let left = snapshot(&[note]);
    let right = snapshot(&[page(10)]);
    let score = Field::new(&id(10), &["meta", "score"]);
    assert_eq!(
        patches(&left, &std::iter::once(score.clone()).collect()).unwrap()[0].value,
        Fragment::Present(Value::Null)
    );
    assert_eq!(
        patches(&right, &std::iter::once(score).collect()).unwrap()[0].value,
        Fragment::Missing
    );
    let unit = Unit {
        fields: std::iter::once(Field::new(&id(10), &["meta", "state"])).collect(),
        ..Unit::default()
    };
    let snapshots = Snapshots {
        base: &left,
        left: &left,
        right: &right,
        candidate: &left,
    };
    let first = describe(&context(), &unit, snapshots, &left).unwrap();
    let mut swapped = context();
    std::mem::swap(&mut swapped.left, &mut swapped.right);
    let second = describe(
        &swapped,
        &unit,
        Snapshots {
            left: &right,
            right: &left,
            ..snapshots
        },
        &left,
    )
    .unwrap();
    assert_eq!(first, second);
}

#[test]
fn high_cardinality_properties_remain_separate_without_losing_any_owner() {
    let property = |key| Unit {
        fields: (10..50_010)
            .map(|number| Field::new(&id(number), &["meta", key]))
            .collect(),
        ..Unit::default()
    };
    let groups = combine(vec![property("first"), property("second")]);
    assert_eq!(groups.len(), 2);
    for (group, key) in groups.iter().zip(["first", "second"]) {
        assert_eq!(group.fields.len(), 50_000);
        assert!(
            group
                .fields
                .contains(&Field::new(&id(50_009), &["meta", key]))
        );
    }
}
