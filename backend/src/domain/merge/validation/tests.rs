// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

#![allow(
    clippy::unwrap_used,
    reason = "Fixture setup and unexpected validation errors fail these tests immediately."
)]

use super::*;
use crate::domain::merge::fixtures::*;
use crate::domain::project::candidate;

fn check(
    changed: &[String],
    base: &Memory,
    left: &Memory,
    right: &Memory,
    candidate: &Memory,
) -> Report {
    validate_affected(
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
fn issue(report: &Report, owner: &str, dependency: &Dependency) -> bool {
    report
        .violations
        .iter()
        .any(|issue| issue.owner == owner && &issue.dependency == dependency)
}

#[test]
fn changed_property_checks_unchanged_and_incoming_owners_from_both_branches() {
    let declaration = named("property", "score");
    let property_id = declaration["id"].as_str().unwrap().to_owned();
    let mut old = page(10);
    old["meta"]["score"] = json!("Before");
    let base = snapshot(&[declaration.clone(), old.clone()]);
    let mut changed = declaration.clone();
    changed["content"]["valueType"] = json!("number");
    let mut left_note = old.clone();
    left_note["meta"]["score"] = json!(10);
    let left = snapshot(&[changed.clone(), left_note]);
    let mut incoming = page(11);
    incoming["meta"]["score"] = json!("Incoming");
    let right = snapshot(&[declaration, old.clone(), incoming.clone()]);
    let candidate = snapshot(&[changed, old, incoming]);
    let report = check(
        &[property_id.clone(), id(11)],
        &base,
        &left,
        &right,
        &candidate,
    );
    let dependency = Dependency::named(NamedKind::Property, "score");
    assert!(issue(&report, &id(10), &dependency));
    assert!(issue(&report, &id(11), &dependency));
    assert_eq!(
        report.affected,
        [property_id, id(10), id(11)].into_iter().collect()
    );
    for source in [&base, &left, &right, &candidate] {
        assert_eq!(source.queries.borrow().get(&dependency), Some(&1));
        assert!(source.reads.borrow().values().all(|count| *count == 1));
    }
    assert_eq!(
        report,
        check(
            &report.affected.iter().cloned().collect::<Vec<_>>(),
            &base,
            &right,
            &left,
            &candidate
        )
    );
}

#[test]
fn removed_property_options_report_the_value_owner() {
    let mut property = named("property", "choice");
    property["content"]["valueType"] = json!("select");
    property["content"]["options"] = json!(["a", "b"]);
    let property_id = property["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["choice"] = json!("b");
    let base = snapshot(&[property.clone(), note.clone()]);
    property["content"]["options"] = json!(["a"]);
    let candidate = snapshot(&[property, note]);
    let report = check(&[property_id], &base, &candidate, &base, &candidate);
    assert!(issue(
        &report,
        &id(10),
        &Dependency::named(NamedKind::Property, "choice")
    ));
}

#[test]
fn deleting_a_declaration_cannot_turn_existing_values_into_unchecked_metadata() {
    let property = named("property", "custom");
    let property_id = property["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["custom"] = json!("Keep me");
    let base = snapshot(&[property, note.clone()]);
    let candidate = snapshot(&[note]);
    let report = check(&[property_id], &base, &candidate, &base, &candidate);
    assert!(issue(
        &report,
        &id(10),
        &Dependency::named(NamedKind::Property, "custom")
    ));
}

#[test]
fn required_builtin_declaration_cannot_disappear_even_when_unused() {
    let property = named("property", "priority");
    let property_id = property["id"].as_str().unwrap().to_owned();
    let base = snapshot(&[property]);
    let candidate = snapshot(&[]);
    let report = check(
        std::slice::from_ref(&property_id),
        &base,
        &candidate,
        &base,
        &candidate,
    );
    assert!(issue(
        &report,
        &property_id,
        &Dependency::named(NamedKind::Property, "priority")
    ));
}

#[test]
fn removed_tags_and_labels_respect_active_owner_usage_rules() {
    for kind in [NamedKind::Tag, NamedKind::Label] {
        let declaration = named(kind.as_str(), "shared");
        let declaration_id = declaration["id"].as_str().unwrap().to_owned();
        let mut note = page(10);
        if kind == NamedKind::Tag {
            note["tags"] = json!(["shared"]);
        } else {
            note["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["shared"]);
        }
        let base = snapshot(&[declaration, note.clone()]);
        let candidate = snapshot(&[note.clone()]);
        let report = check(
            std::slice::from_ref(&declaration_id),
            &base,
            &candidate,
            &base,
            &candidate,
        );
        assert!(issue(&report, &id(10), &Dependency::named(kind, "shared")));
        note["meta"]["state"] = json!("archived");
        let candidate = snapshot(&[note]);
        let report = check(
            &[declaration_id, id(10)],
            &base,
            &candidate,
            &base,
            &candidate,
        );
        assert_eq!(report.violations, []);
    }
}

#[test]
fn snippet_removal_checks_archived_templates_too() {
    let snippet = named("snippet", "reference");
    let snippet_id = snippet["id"].as_str().unwrap().to_owned();
    let mut note = page(10);
    note["meta"]["type"] = json!("template");
    note["meta"]["state"] = json!("archived");
    note["meta"]["title"] = json!("Template");
    note["content"]["document"]["content"] =
        json!([{"type":"embed","attrs":{"id":id(1010),"snippetId":"reference"}}]);
    let base = snapshot(&[snippet, note.clone()]);
    let candidate = snapshot(&[note]);
    let report = check(&[snippet_id], &base, &candidate, &base, &candidate);
    assert_eq!(report.violations.len(), 1);
    assert!(issue(
        &report,
        &id(10),
        &Dependency::named(NamedKind::Snippet, "reference")
    ));
}

#[test]
fn page_archival_detects_incoming_tasks_and_validates_a_repaired_candidate() {
    let note = page(10);
    let base = snapshot(std::slice::from_ref(&note));
    let mut archived = note.clone();
    archived["meta"]["state"] = json!("archived");
    let left = snapshot(std::slice::from_ref(&archived));
    let task = record(&id(11), "task", json!({"noteId":id(10),"text":"Finish"}));
    let right = snapshot(&[note, task.clone()]);
    let candidate = snapshot(&[archived.clone(), task.clone()]);
    let report = check(&[id(10), id(11)], &base, &left, &right, &candidate);
    assert!(issue(&report, &id(11), &Dependency::Item(id(10))));
    let mut repaired_task = task;
    repaired_task["meta"]["state"] = json!("archived");
    let repaired = snapshot(&[archived, repaired_task.clone()]);
    assert_eq!(
        check(&[id(10), id(11)], &base, &left, &right, &repaired).violations,
        []
    );
    let no_parent = snapshot(&[repaired_task]);
    assert!(issue(
        &check(&[id(10), id(11)], &base, &left, &right, &no_parent),
        &id(11),
        &Dependency::Item(id(10))
    ));
}

#[test]
fn default_workspace_is_checked_even_without_an_item_change() {
    let base = snapshot(&[]);
    let mut candidate = snapshot(&[]);
    candidate.items.remove(&id(1));
    let report = check(&[], &base, &base, &base, &candidate);
    assert!(issue(&report, "bluink.json", &Dependency::Item(id(1))));
}

#[test]
fn existing_pins_survive_archival_but_new_pins_require_an_active_workspace() {
    let other = workspace(2);
    let mut note = page(10);
    note["pins"] = json!([id(2)]);
    let base = snapshot(&[other.clone(), note.clone()]);
    let mut archived = other.clone();
    archived["meta"]["state"] = json!("archived");
    let candidate = snapshot(&[archived.clone(), note.clone()]);
    assert_eq!(
        check(&[id(2)], &base, &candidate, &base, &candidate).violations,
        []
    );
    let pinned_branch = snapshot(&[other.clone(), note.clone()]);
    note.as_object_mut().unwrap().remove("pins");
    let unpinned = snapshot(&[other, note.clone()]);
    let archived_branch = snapshot(&[archived, note]);
    assert_eq!(
        check(
            &[id(2), id(10)],
            &unpinned,
            &candidate,
            &unpinned,
            &candidate
        )
        .violations,
        []
    );
    assert!(issue(
        &check(
            &[id(2), id(10)],
            &unpinned,
            &pinned_branch,
            &archived_branch,
            &candidate
        ),
        &id(10),
        &Dependency::Item(id(2))
    ));
}

#[test]
fn unknown_metadata_and_free_facets_remain_supported() {
    let base = snapshot(&[page(10)]);
    let mut note = page(10);
    note["meta"]["custom"] = json!({"extension":true});
    note["tags"] = json!(["undeclared"]);
    note["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["undeclared"]);
    let candidate = snapshot(&[note]);
    let report = check(&[id(10)], &base, &candidate, &base, &candidate);
    assert_eq!(report.violations, []);
    assert_eq!(report.constraints, BTreeSet::new());
}

#[test]
fn cosmetic_changes_do_not_expand_to_fifty_thousand_tag_owners() {
    let tag = named("tag", "shared");
    let tag_id = tag["id"].as_str().unwrap().to_owned();
    let mut base = snapshot(std::slice::from_ref(&tag));
    let dependency = Dependency::named(NamedKind::Tag, "shared");
    base.incoming
        .insert(dependency, (10000..60000).map(id).collect());
    let mut changed = tag;
    changed["content"]["color"] = json!("blue");
    let candidate = snapshot(&[changed]);
    let report = check(
        std::slice::from_ref(&tag_id),
        &base,
        &candidate,
        &base,
        &candidate,
    );
    assert_eq!(report.affected, std::iter::once(tag_id).collect());
    assert_eq!(report.violations, []);
    assert_eq!(base.queries.borrow().len(), 0);
    assert_eq!(candidate.queries.borrow().len(), 0);
    assert!(candidate.reads.borrow().len() < 10);
}

#[test]
fn missing_dependency_index_stops_instead_of_scanning_or_assuming_unused() {
    let tag = named("tag", "shared");
    let tag_id = tag["id"].as_str().unwrap().to_owned();
    let mut base = snapshot(&[tag]);
    base.fail_lookup = true;
    let candidate = snapshot(&[]);
    let error = validate_affected(
        &config(),
        &std::iter::once(tag_id).collect(),
        Snapshots {
            base: &base,
            left: &candidate,
            right: &base,
            candidate: &candidate,
        },
    )
    .unwrap_err();
    assert!(error.contains("dependency index"));
}

#[test]
fn stale_reverse_candidates_are_verified_and_missing_records_are_not_errors() {
    let tag = named("tag", "shared");
    let tag_id = tag["id"].as_str().unwrap().to_owned();
    let mut base = snapshot(&[tag, page(10)]);
    base.incoming.insert(
        Dependency::named(NamedKind::Tag, "shared"),
        [id(10), id(11)].into_iter().collect(),
    );
    let candidate = snapshot(&[page(10)]);
    let report = check(&[tag_id], &base, &candidate, &base, &candidate);
    assert_eq!(report.violations, []);
    assert!(report.affected.contains(&id(10)));
    assert!(report.affected.contains(&id(11)));
}

#[test]
fn read_errors_and_malformed_items_do_not_become_absent_records() {
    let base = snapshot(&[page(10)]);
    let mut candidate = snapshot(&[page(10)]);
    candidate.fail_read = Some(id(10));
    let validate = |candidate: &Memory| {
        validate_affected(
            &config(),
            &std::iter::once(id(10)).collect(),
            Snapshots {
                base: &base,
                left: &base,
                right: &base,
                candidate,
            },
        )
    };
    assert!(validate(&candidate).unwrap_err().contains("read failed"));
    candidate.fail_read = None;
    candidate.items.get_mut(&id(10)).unwrap()["formatVersion"] = json!(0);
    assert!(validate(&candidate).unwrap_err().contains("unsupported"));
}

#[test]
fn overlapping_constraints_read_each_affected_record_once_per_snapshot() {
    let first = named("property", "first");
    let second = named("property", "second");
    let mut note = page(10);
    note["meta"]["first"] = json!("one");
    note["meta"]["second"] = json!("two");
    let base = snapshot(&[first.clone(), second.clone(), note.clone()]);
    let left = snapshot(&[second.clone(), note.clone()]);
    let right = snapshot(&[first.clone(), note.clone()]);
    let candidate = snapshot(&[note]);
    let changed = [
        first["id"].as_str().unwrap().to_owned(),
        second["id"].as_str().unwrap().to_owned(),
    ];
    let report = check(&changed, &base, &left, &right, &candidate);
    assert_eq!(report.violations.len(), 2);
    for source in [&base, &left, &right, &candidate] {
        assert!(source.reads.borrow().values().all(|count| *count == 1));
    }
}

#[test]
fn full_restore_and_incremental_checks_share_property_and_owner_rules() {
    let mut property = named("property", "score");
    property["content"]["valueType"] = json!("number");
    let mut note = page(10);
    note["meta"]["score"] = json!("invalid");
    let candidate = snapshot(&[property, note]);
    let report = check(&[id(10)], &candidate, &candidate, &candidate, &candidate);
    assert!(issue(
        &report,
        &id(10),
        &Dependency::named(NamedKind::Property, "score")
    ));
    assert!(
        candidate::validate(
            &config(),
            candidate.items.keys().map(String::as_str),
            |id| Ok(candidate.items[id].clone())
        )
        .is_err()
    );
}

#[test]
fn captured_relationships_keep_references_without_retaining_prose() {
    let mut note = page(10);
    note["content"]["document"]["content"][0]["content"][0]["text"] =
        json!("Long prose ".repeat(100_000));
    note["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["shared"]);
    note["content"]["document"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"embed","attrs":{"id":id(1011),"snippetId":"reference"}}));
    let source = snapshot(&[note]);
    let mut captured = Capture::new(Snapshots {
        base: &source,
        left: &source,
        right: &source,
        candidate: &source,
    });
    let summary = captured.read(CANDIDATE, &id(10)).unwrap().unwrap();
    assert!(summary.to_string().len() < 500);
    let references = &captured.references[&id(10)];
    assert!(references.contains(&Dependency::named(NamedKind::Label, "shared")));
    assert!(references.contains(&Dependency::named(NamedKind::Snippet, "reference")));
    assert!(
        captured
            .catalogue
            .check_captured(&summary, references)
            .unwrap()
            .iter()
            .any(|issue| issue.dependency == Dependency::named(NamedKind::Snippet, "reference"))
    );
    captured.read(CANDIDATE, &id(10)).unwrap();
    assert_eq!(source.reads.borrow()[&id(10)], 1);
}
