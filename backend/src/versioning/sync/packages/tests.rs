// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use super::*;
use crate::domain::merge::model::Source;

fn context() -> Context {
    Context {
        target: "01a0e2a0-0000-7000-8000-000000000001".into(),
        base_epoch: "base".into(),
        left: Source::Commit {
            id: "left".into(),
            committer_time: 1,
            device: "A".into(),
        },
        right: Source::Commit {
            id: "right".into(),
            committer_time: 2,
            device: "B".into(),
        },
    }
}

#[test]
fn json_fields_resolve_separately_and_preserve_unrelated_changes() {
    use serde_json::json;
    let (base, left, right) = (
        json!({"a":0,"nested":{"b":0},"other":0}),
        json!({"a":1,"nested":{"b":1},"other":2}),
        json!({"a":2,"nested":{"b":2},"other":0}),
    );
    let mut proposals = Vec::new();
    let candidate = super::json_merge(
        &context(),
        "bluink.json",
        &mut Vec::new(),
        [Some(&base), Some(&left), Some(&right)],
        &mut proposals,
    )
    .unwrap()
    .unwrap();
    assert_eq!(proposals.len(), 2);
    let mut source = serde_json::to_string(&candidate).unwrap();
    for proposal in &proposals {
        let file = proposal.file_decision().unwrap();
        source = file
            .apply(Some(&source), &file.selected(Selection::First).unwrap())
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        serde_json::from_str::<Value>(&source).unwrap(),
        json!({"a":1,"nested":{"b":1},"other":2})
    );
    let independent = json!({"a":9,"nested":{"b":1},"other":2}).to_string();
    let first = proposals[0].file_decision().unwrap();
    assert!(
        first
            .apply(
                Some(&independent),
                &first.selected(Selection::First).unwrap()
            )
            .is_err()
    );
}

#[test]
fn two_text_regions_allow_independent_choices_without_markers_or_offset_addressing() {
    use std::fmt::Write;
    let mut shared = String::new();
    for n in 0..18 {
        writeln!(shared, "shared {n}").unwrap();
    }
    let base = format!("start\nold first\n{shared}old second\nfinish");
    let left = base.replace("old", "left");
    let right = base.replace("old", "right");
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::init_bare(temp.path()).unwrap();
    let blob = |text: &str| Some(repo.blob(text.as_bytes()).unwrap());
    let (oid, proposals) = merge(
        &repo,
        &context(),
        "themes/theme-test/page.html",
        blob(&base),
        blob(&left),
        blob(&right),
    )
    .unwrap();
    let mut text =
        String::from_utf8(repo.find_blob(oid.unwrap()).unwrap().content().to_vec()).unwrap();
    assert_eq!(text, right);
    assert_eq!(proposals.len(), 2);
    text = text.replace("shared 8\n", "later independent edit\n");
    let first = proposals[0].file_decision().unwrap();
    text = first
        .apply(Some(&text), &first.selected(Selection::First).unwrap())
        .unwrap()
        .unwrap();
    let second = proposals[1].file_decision().unwrap();
    assert_eq!(
        second.current(Some(&text)).unwrap(),
        second.selected(Selection::Second).unwrap()
    );
    assert!(
        text.contains("left first\n")
            && text.contains("right second\n")
            && text.contains("later independent edit")
    );
    assert!(
        second
            .current(Some(&text.replace("right second", "new author edit")))
            .is_err()
    );
}

#[test]
fn delete_against_edited_final_line_preserves_the_edit_and_exact_final_newline() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::init_bare(temp.path()).unwrap();
    let blob = |text: &str| Some(repo.blob(text.as_bytes()).unwrap());
    let mut context = context();
    std::mem::swap(&mut context.left, &mut context.right);
    let path = "themes/theme-test/page.html";
    let (oid, proposals) = merge(
        &repo,
        &context,
        path,
        blob("start\noriginal"),
        blob("start\n"),
        blob("start\nkeep this"),
    )
    .unwrap();
    assert_eq!(
        repo.find_blob(oid.unwrap()).unwrap().content(),
        b"start\nkeep this"
    );
    assert_eq!(proposals.len(), 1);
    let file = proposals[0].file_decision().unwrap();
    assert!(
        file.alternatives
            .iter()
            .any(|choice| choice.value == Fragment::Missing)
    );
    assert_eq!(
        file.apply(Some("start\nkeep this"), &Fragment::Missing)
            .unwrap()
            .as_deref(),
        Some("start\n")
    );
}
