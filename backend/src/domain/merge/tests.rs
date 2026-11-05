// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "Tests fail immediately with the unexpected merge output."
)]

use super::item::{ItemCandidate, ItemMerge, merge_item};
use super::model::{Context, DecisionKind, Fragment, Location, Merger, Source};
use serde_json::{Value, json};

const ITEM: &str = "01a0e2a0-0000-7000-8000-000000000001";

#[test]
fn publication_outline_merges_page_identity_and_nested_children_independently() {
    let mut base = super::fixtures::publication(1, 10);
    base["content"]["outline"] = json!([{"kind":"section","id":id(20),"title":"Section","children":[{"kind":"page","pageId":id(30),"children":[]}]}]);
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["outline"][0]["title"] = json!("Renamed");
    right["content"]["outline"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"page","pageId":id(31),"children":[]}));
    let merged = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap();
    assert_eq!(merged.decisions, []);
    assert_eq!(fields(&merged)["content"]["outline"][0]["title"], "Renamed");
    assert_eq!(
        fields(&merged)["content"]["outline"][0]["children"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut reverse = context();
    std::mem::swap(&mut reverse.left, &mut reverse.right);
    assert_eq!(
        merged,
        merge_item(&reverse, Some(&base), Some(&right), Some(&left)).unwrap()
    );
}

fn id(number: usize) -> String {
    format!("01a0e2a0-0000-7000-8000-{number:012}")
}
fn paragraph(number: usize, text: &str) -> Value {
    json!({"type":"paragraph","attrs":{"id":id(number)},"content":[{"type":"text","text":text}]})
}
fn page(blocks: Vec<Value>) -> Value {
    let mut item = json!({"format":"bluink.item","formatVersion":20,"id":ITEM,"revision":"base-revision",
        "createdAt":"2026-10-30T10:00:00.000Z","modifiedAt":"2026-10-30T10:00:00.000Z",
        "meta":{"type":"page","state":"active","title":"A page"},"analytics":{},
        "content":{"document":{"type":"doc","content":[]}}});
    item["content"]["document"]["content"] = Value::Array(blocks);
    item
}
fn context() -> Context {
    Context {
        target: ITEM.into(),
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
fn fields(result: &ItemMerge) -> &Value {
    match &result.candidate {
        ItemCandidate::Existing(value) | ItemCandidate::Changed { fields: value, .. } => value,
        ItemCandidate::Deleted => panic!("Expected a surviving item"),
    }
}
fn blocks(result: &ItemMerge) -> &Vec<Value> {
    fields(result)["content"]["document"]["content"]
        .as_array()
        .unwrap()
}
fn merge(base: &Value, left: &Value, right: &Value) -> ItemMerge {
    merge_item(&context(), Some(base), Some(left), Some(right)).unwrap()
}
fn change_text(item: &mut Value, block: usize, text: &str) {
    item["content"]["document"]["content"][block]["content"][0]["text"] = json!(text);
}

#[test]
fn independent_paragraphs_merge_and_archive_keeps_the_edit() {
    let base = page(vec![paragraph(1, "First"), paragraph(2, "Second")]);
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "First edited");
    change_text(&mut right, 1, "Second edited");
    right["meta"]["state"] = json!("archived");
    let result = merge(&base, &left, &right);
    assert_eq!(blocks(&result)[0]["content"][0]["text"], "First edited");
    assert_eq!(blocks(&result)[1]["content"][0]["text"], "Second edited");
    assert_eq!(fields(&result)["meta"]["state"], "archived");
    assert_eq!(result.decisions, []);
    assert!(matches!(result.candidate, ItemCandidate::Changed { .. }));
    assert!(fields(&result).get("revision").is_none());
    assert!(fields(&result).get("analytics").is_none());
}

#[test]
fn same_block_is_one_decision_and_never_character_merged() {
    let base = page(vec![paragraph(1, "The wind")]);
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "The gale");
    change_text(&mut right, 0, "The breeze");
    let result = merge(&base, &left, &right);
    assert_eq!(
        blocks(&result)[0],
        right["content"]["document"]["content"][0]
    );
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::Block);
    assert!(matches!(&result.candidate, ItemCandidate::Existing(value) if value == &right));
    let decision = &result.decisions[0];
    assert_eq!(
        decision.alternatives[decision.provisional].source,
        context().right
    );
}

#[test]
fn identical_edits_and_volatile_envelopes_reuse_input_without_decisions() {
    let base = page(vec![paragraph(1, "First")]);
    let mut left = base.clone();
    change_text(&mut left, 0, "Edited");
    let mut right = left.clone();
    right["revision"] = json!("other");
    right["analytics"] = json!({"current":{"wordCount":123}});
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions, []);
    assert_eq!(result.candidate, ItemCandidate::Existing(right));
}

#[test]
fn set_membership_is_base_relative_and_order_is_not_an_edit() {
    let mut base = page(vec![paragraph(1, "First")]);
    base["tags"] = json!(["old", "keep"]);
    let mut left = base.clone();
    left["tags"] = json!(["keep", "new_left"]);
    let mut right = base.clone();
    right["tags"] = json!(["old", "new_right", "keep"]);
    let result = merge(&base, &left, &right);
    assert_eq!(
        fields(&result)["tags"],
        json!(["keep", "new_left", "new_right"])
    );
    assert_eq!(result.decisions, []);
    right["tags"] = json!(["keep", "old"]);
    assert_eq!(
        merge(&base, &base, &right).candidate,
        ItemCandidate::Existing(right)
    );
}

#[test]
fn paragraph_labels_and_text_merge_independently() {
    let mut base = page(vec![paragraph(1, "First")]);
    base["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["old"]);
    let mut left = base.clone();
    change_text(&mut left, 0, "Edited");
    let mut right = base.clone();
    right["content"]["document"]["content"][0]["attrs"]["labels"] = json!(["new"]);
    let result = merge(&base, &left, &right);
    assert_eq!(blocks(&result)[0]["attrs"]["labels"], json!(["new"]));
    assert_eq!(blocks(&result)[0]["content"][0]["text"], "Edited");
    assert_eq!(result.decisions, []);
}

#[test]
fn deletion_against_edit_preserves_the_edit_even_when_deletion_is_preferred() {
    let base = page(vec![paragraph(1, "First"), paragraph(2, "Second")]);
    let mut left = base.clone();
    change_text(&mut left, 0, "Keep this edit");
    let right = page(vec![paragraph(2, "Second")]);
    let result = merge(&base, &left, &right);
    assert_eq!(blocks(&result)[0]["content"][0]["text"], "Keep this edit");
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::DeleteEdit);
    assert!(
        result.decisions[0]
            .alternatives
            .iter()
            .any(|a| a.value == Fragment::Missing)
    );
    let deleted = merge_item(&context(), Some(&base), Some(&base), None).unwrap();
    assert_eq!(deleted.candidate, ItemCandidate::Deleted);
    let edited = merge_item(&context(), Some(&base), Some(&left), None).unwrap();
    assert_eq!(edited.candidate, ItemCandidate::Existing(left));
    assert_eq!(edited.decisions[0].kind, DecisionKind::DeleteEdit);
}

#[test]
fn metadata_is_field_by_field_and_null_is_not_missing() {
    let mut base = page(vec![paragraph(1, "First")]);
    base["meta"]["custom"] = json!("before");
    let mut left = base.clone();
    left["meta"].as_object_mut().unwrap().remove("custom");
    left["meta"]["title"] = json!("Left title");
    let mut right = base.clone();
    right["meta"]["custom"] = Value::Null;
    right["meta"]["summary"] = json!("Right summary");
    let result = merge(&base, &left, &right);
    assert_eq!(fields(&result)["meta"]["title"], "Left title");
    assert_eq!(fields(&result)["meta"]["summary"], "Right summary");
    assert_eq!(result.decisions.len(), 1);
    assert!(
        result.decisions[0]
            .alternatives
            .iter()
            .any(|a| a.value == Fragment::Missing)
    );
    assert!(
        result.decisions[0]
            .alternatives
            .iter()
            .any(|a| a.value == Fragment::Present(Value::Null))
    );
}

#[test]
fn decisions_and_output_are_independent_of_argument_order() {
    let base = page(vec![paragraph(1, "First"), paragraph(2, "Second")]);
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "Left");
    change_text(&mut right, 0, "Right");
    left["meta"]["title"] = json!("Left");
    right["meta"]["title"] = json!("Right");
    let result = merge(&base, &left, &right);
    let mut swapped = context();
    std::mem::swap(&mut swapped.left, &mut swapped.right);
    assert_eq!(
        result,
        merge_item(&swapped, Some(&base), Some(&right), Some(&left)).unwrap()
    );
}

#[test]
fn conflict_identity_ignores_source_commits_and_envelopes_but_not_base_epoch() {
    let base = page(vec![paragraph(1, "First")]);
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "Left");
    change_text(&mut right, 0, "Right");
    let first = merge(&base, &left, &right).decisions[0].id.clone();
    let mut context = context();
    context.left = Source::Commit {
        id: "unrelated-commit".into(),
        committer_time: 101,
        device: "Another name".into(),
    };
    left["revision"] = json!("different-revision");
    assert_eq!(
        first,
        merge_item(&context, Some(&base), Some(&left), Some(&right))
            .unwrap()
            .decisions[0]
            .id
    );
    context.base_epoch = "commit:another-base".into();
    assert_ne!(
        first,
        merge_item(&context, Some(&base), Some(&left), Some(&right))
            .unwrap()
            .decisions[0]
            .id
    );
}

#[test]
fn draft_provisional_keeps_the_submitted_text_and_commit_ties_use_id() {
    let base = page(vec![paragraph(1, "First")]);
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "Draft");
    change_text(&mut right, 0, "Canonical");
    let mut context = context();
    context.left = Source::Draft {
        submission_id: "draft-1".into(),
        device: "Laptop".into(),
    };
    context.right = Source::Revision {
        revision: "canonical".into(),
        modified_at: "2026-10-30T12:00:00.000Z".into(),
    };
    assert_eq!(
        merge_item(&context, Some(&base), Some(&left), Some(&right))
            .unwrap()
            .candidate,
        ItemCandidate::Existing(left.clone())
    );
    context.left = Source::Commit {
        id: "z".into(),
        committer_time: 100,
        device: "A".into(),
    };
    context.right = Source::Commit {
        id: "a".into(),
        committer_time: 100,
        device: "B".into(),
    };
    assert_eq!(
        merge_item(&context, Some(&base), Some(&left), Some(&right))
            .unwrap()
            .candidate,
        ItemCandidate::Existing(left)
    );
}

fn ordered(numbers: &[usize]) -> Value {
    page(
        numbers
            .iter()
            .map(|number| paragraph(*number, &format!("Block {number}")))
            .collect(),
    )
}
fn order(result: &ItemMerge) -> Vec<String> {
    blocks(result)
        .iter()
        .map(|v| v["attrs"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn moved_block_keeps_other_sides_edit() {
    let base = ordered(&[1, 2, 3]);
    let left = ordered(&[2, 3, 1]);
    let mut right = base.clone();
    change_text(&mut right, 0, "Edited while moved");
    let result = merge(&base, &left, &right);
    assert_eq!(order(&result), vec![id(2), id(3), id(1)]);
    assert_eq!(
        blocks(&result)[2]["content"][0]["text"],
        "Edited while moved"
    );
    assert_eq!(result.decisions, []);
}

#[test]
fn concurrent_insertions_keep_both_runs_in_provisional_order() {
    let base = ordered(&[1, 2]);
    let left = ordered(&[1, 3, 4, 2]);
    let right = ordered(&[1, 5, 6, 2]);
    let result = merge(&base, &left, &right);
    assert_eq!(order(&result), [1, 5, 6, 3, 4, 2].map(id));
    assert_eq!(result.decisions, []);
}

#[test]
fn disjoint_moves_merge_and_incompatible_positions_open_a_decision() {
    let base = ordered(&[1, 2, 3, 4, 5, 6]);
    let left = ordered(&[2, 1, 3, 4, 5, 6]);
    let right = ordered(&[1, 2, 3, 4, 6, 5]);
    let result = merge(&base, &left, &right);
    assert_eq!(order(&result), [2, 1, 3, 4, 6, 5].map(id));
    assert_eq!(result.decisions, []);
    let base = ordered(&[1, 2, 3, 4, 5]);
    let left = ordered(&[3, 1, 2, 4, 5]);
    let right = ordered(&[1, 2, 4, 5, 3]);
    let result = merge(&base, &left, &right);
    assert_eq!(order(&result), [1, 2, 4, 5, 3].map(id));
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::Position);
}

#[test]
fn all_small_permutations_preserve_membership_and_converge_in_both_orders() {
    fn permutations(values: &[usize]) -> Vec<Vec<usize>> {
        if values.is_empty() {
            return vec![vec![]];
        }
        values
            .iter()
            .enumerate()
            .flat_map(|(index, value)| {
                let remaining: Vec<_> = values
                    .iter()
                    .enumerate()
                    .filter_map(|(i, value)| (i != index).then_some(*value))
                    .collect();
                permutations(&remaining)
                    .into_iter()
                    .map(|mut tail| {
                        tail.insert(0, *value);
                        tail
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    let base = ordered(&[1, 2, 3, 4]);
    let mut swapped = context();
    std::mem::swap(&mut swapped.left, &mut swapped.right);
    for left_order in permutations(&[1, 2, 3, 4]) {
        let left = ordered(&left_order);
        for right_order in permutations(&[1, 2, 3, 4]) {
            let right = ordered(&right_order);
            let result = merge(&base, &left, &right);
            assert_eq!(
                result,
                merge_item(&swapped, Some(&base), Some(&right), Some(&left)).unwrap()
            );
            let mut actual = order(&result);
            actual.sort();
            assert_eq!(actual, [1, 2, 3, 4].map(id));
        }
    }
}

#[test]
fn type_change_against_edit_is_one_whole_block_decision() {
    let base = ordered(&[1]);
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["document"]["content"][0] = json!({"type":"heading","attrs":{"id":id(1),"level":2},"content":[{"type":"text","text":"Heading"}]});
    change_text(&mut right, 0, "Paragraph edited");
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::Block);
    assert_eq!(blocks(&result)[0]["type"], "paragraph");
}

fn table() -> Value {
    json!({"type":"table","attrs":{"id":id(10),"columnWidthsPercent":[50,50]},"content":[
        {"type":"table_row","attrs":{"id":id(11)},"content":[
            {"type":"table_cell","content":[paragraph(12,"A")]},
            {"type":"table_cell","content":[paragraph(13,"B")]}
        ]}
    ]})
}

#[test]
fn table_cells_merge_but_geometry_change_conflicts_as_one_table() {
    let base = page(vec![table()]);
    let mut left = base.clone();
    let mut right = base.clone();
    let path = "/content/document/content/0/content/0/content/0/content/0/content/0/text";
    *left.pointer_mut(path).unwrap() = json!("Left cell");
    let path = "/content/document/content/0/content/0/content/1/content/0/content/0/text";
    *right.pointer_mut(path).unwrap() = json!("Right cell");
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions, []);
    assert_eq!(fields(&result).pointer(path), Some(&json!("Right cell")));
    left["content"]["document"]["content"][0]["attrs"]["columnWidthsPercent"] = json!([30, 70]);
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::Block);
    assert_eq!(
        blocks(&result)[0],
        right["content"]["document"]["content"][0]
    );
}

#[test]
fn invalid_input_cannot_hide_behind_a_fast_path() {
    let base = ordered(&[1]);
    let mut invalid = base.clone();
    invalid["content"]["document"]["content"][0]["attrs"]["id"] = json!("not-a-uuid");
    assert!(merge_item(&context(), Some(&base), Some(&invalid), Some(&base)).is_err());
    let duplicate = ordered(&[1, 1]);
    assert!(merge_item(&context(), Some(&base), Some(&duplicate), Some(&base)).is_err());
}

#[test]
fn malformed_sequence_and_unsupported_source_pairs_are_rejected() {
    let context = context();
    let mut merger = Merger::new(&context).unwrap();
    let duplicates = json!([{"id":"same"},{"id":"same"}]);
    assert!(
        merger
            .sequence(
                &Location::default(),
                None,
                &duplicates,
                &json!([]),
                |v| v["id"].as_str(),
                Merger::fields
            )
            .is_err()
    );
    let mut context = context;
    context.right = context.left.clone();
    assert!(Merger::new(&context).is_err());
}

#[test]
fn long_sequence_merge_does_not_need_a_quadratic_diff_matrix() {
    let base = ordered(&(1..=2000).collect::<Vec<_>>());
    let mut left = base.clone();
    let mut right = base.clone();
    change_text(&mut left, 0, "First edit");
    change_text(&mut right, 1999, "Last edit");
    let result = merge(&base, &left, &right);
    assert_eq!(blocks(&result).len(), 2000);
    assert_eq!(result.decisions, []);
}

#[test]
fn lists_and_boxes_recurse_through_stable_children() {
    let list = json!({"type":"list","attrs":{"id":id(10),"kind":"bullet"},"content":[
        {"type":"listItem","attrs":{"id":id(11)},"content":[paragraph(12,"One")]},
        {"type":"listItem","attrs":{"id":id(13)},"content":[paragraph(14,"Two")]}
    ]});
    let base = page(vec![
        json!({"type":"box","attrs":{"id":id(20),"kind":"plain"},"content":[list]}),
    ]);
    let mut left = base.clone();
    let mut right = base.clone();
    let first = "/content/document/content/0/content/0/content/0/content/0/content/0/text";
    let second = "/content/document/content/0/content/0/content/1/content/0/content/0/text";
    *left.pointer_mut(first).unwrap() = json!("Left");
    *right.pointer_mut(second).unwrap() = json!("Right");
    let result = merge(&base, &left, &right);
    assert_eq!(fields(&result).pointer(first), Some(&json!("Left")));
    assert_eq!(fields(&result).pointer(second), Some(&json!("Right")));
    assert_eq!(result.decisions, []);
}

#[test]
fn valid_inputs_whose_combination_is_invalid_return_no_candidate() {
    let base = ordered(&[1, 2]);
    let left = ordered(&[1]);
    let right = ordered(&[2]);
    // Each side leaves a valid document; together they remove its final block.
    let error = merge_item(&context(), Some(&base), Some(&left), Some(&right)).unwrap_err();
    assert!(error.contains("Merged candidate is invalid"), "{error}");
}

#[test]
fn task_text_and_completion_merge_as_fields() {
    let mut base = ordered(&[1]);
    base["meta"]["type"] = json!("task");
    base["content"] = json!({"noteId":id(2),"text":"Before"});
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["text"] = json!("Edited");
    right["content"]["completedAt"] = json!("2026-10-30T12:00:00.000Z");
    let result = merge(&base, &left, &right);
    assert_eq!(fields(&result)["content"]["text"], "Edited");
    assert_eq!(
        fields(&result)["content"]["completedAt"],
        "2026-10-30T12:00:00.000Z"
    );
    assert_eq!(result.decisions, []);
}

#[test]
fn declarations_merge_options_and_preserve_extension_fields() {
    let mut base = ordered(&[1]);
    base["meta"]["type"] = json!("property");
    base["id"] = json!(crate::domain::project::named_items::id(
        "property", "priority"
    ));
    base["content"] = json!({"key":"priority","valueType":"select","options":["a","b"],"color":"blue","custom":{"preserved":true}});
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["options"] = json!(["b", "c"]);
    right["content"]["options"] = json!(["a", "b", "d"]);
    right["content"]["color"] = json!("green");
    let mut context = context();
    context.target = base["id"].as_str().unwrap().to_owned();
    let result = merge_item(&context, Some(&base), Some(&left), Some(&right)).unwrap();
    assert_eq!(
        fields(&result)["content"]["options"],
        json!(["b", "d", "c"])
    );
    assert_eq!(fields(&result)["content"]["color"], "green");
    assert_eq!(
        fields(&result)["content"]["custom"],
        json!({"preserved":true})
    );
    assert_eq!(result.decisions, []);
}

#[test]
fn property_option_reorder_merges_with_addition_and_conflicting_moves_need_a_decision() {
    let mut base = ordered(&[1]);
    base["meta"]["type"] = json!("property");
    base["id"] = json!(crate::domain::project::named_items::id(
        "property", "priority"
    ));
    base["content"] = json!({"key":"priority","valueType":"select","options":["a","b","c"],"optionAppearance":{"a":{"color":"blue"}}});
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["options"] = json!(["b", "a", "c"]);
    right["content"]["options"] = json!(["a", "b", "c", "d"]);
    let mut context = context();
    context.target = base["id"].as_str().unwrap().to_owned();
    let result = merge_item(&context, Some(&base), Some(&left), Some(&right)).unwrap();
    assert_eq!(
        fields(&result)["content"]["options"],
        json!(["b", "a", "c", "d"])
    );
    assert_eq!(
        fields(&result)["content"]["optionAppearance"],
        base["content"]["optionAppearance"]
    );
    assert_eq!(result.decisions, []);
    right["content"]["options"] = json!(["a", "c", "b"]);
    let conflicting = merge_item(&context, Some(&base), Some(&left), Some(&right)).unwrap();
    assert_ne!(conflicting.decisions, []);
}

#[test]
fn workspace_widgets_merge_independently_and_edit_survives_deletion() {
    let mut base = ordered(&[1]);
    base["meta"]["type"] = json!("workspace");
    base["content"] =
        serde_json::from_str(include_str!("../../../resources/workspace-starter.json")).unwrap();
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["widgets"][0]["title"] = json!("My writing");
    right["content"]["widgets"][1]["title"] = json!("My goals");
    let result = merge(&base, &left, &right);
    assert_eq!(
        fields(&result)["content"]["widgets"][0]["title"],
        "My writing"
    );
    assert_eq!(
        fields(&result)["content"]["widgets"][1]["title"],
        "My goals"
    );
    assert_eq!(result.decisions, []);
    right["content"]["widgets"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    let result = merge(&base, &left, &right);
    assert_eq!(
        fields(&result)["content"]["widgets"][0]["title"],
        "My writing"
    );
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::DeleteEdit);
}

#[test]
fn ignored_findings_merge_as_membership_without_touching_prose() {
    let mut base = ordered(&[1]);
    base["content"]["ignoredFindings"] = json!(["old", "keep"]);
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["ignoredFindings"] = json!(["keep"]);
    right["content"]["ignoredFindings"] = json!(["old", "keep", "new"]);
    let result = merge(&base, &left, &right);
    assert_eq!(
        fields(&result)["content"]["ignoredFindings"],
        json!(["keep", "new"])
    );
    assert_eq!(blocks(&result)[0], paragraph(1, "Block 1"));
    assert_eq!(result.decisions, []);
}

#[test]
fn independently_created_items_merge_without_a_fabricated_base() {
    let mut left = ordered(&[1]);
    let mut right = ordered(&[2]);
    left["tags"] = json!(["left"]);
    right["tags"] = json!(["right"]);
    let result = merge_item(&context(), None, Some(&left), Some(&right)).unwrap();
    assert_eq!(order(&result), [2, 1].map(id));
    assert_eq!(fields(&result)["tags"], json!(["left", "right"]));
    assert_eq!(result.decisions, []);
}

#[test]
fn missing_and_null_bases_have_different_decision_identities() {
    let context = context();
    let mut missing = Merger::new(&context).unwrap();
    let mut null = Merger::new(&context).unwrap();
    for (merger, base) in [(&mut missing, None), (&mut null, Some(&Value::Null))] {
        merger
            .atomic(
                &Location::default(),
                base,
                Some(&json!("left")),
                Some(&json!("right")),
            )
            .unwrap();
    }
    assert_ne!(missing.decisions[0].id, null.decisions[0].id);
}

#[test]
fn moving_a_block_against_deletion_keeps_it_and_opens_a_decision() {
    let base = ordered(&[1, 2, 3, 4, 5]);
    let left = ordered(&[1, 2, 4, 5, 3]);
    let right = ordered(&[1, 2, 4, 5]);
    let result = merge(&base, &left, &right);
    assert_eq!(order(&result), [1, 2, 4, 5, 3].map(id));
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::DeleteEdit);
}

#[test]
fn sets_use_string_order_not_json_escape_order() {
    let result = super::value::set(
        Some(&json!(["old"])),
        Some(&json!(["\"quoted\"", "z"])),
        Some(&json!(["old", "\\slash", "a"])),
    )
    .unwrap();
    assert_eq!(result, Some(json!(["\"quoted\"", "\\slash", "a", "z"])));
}

#[test]
fn snippets_use_the_same_conflict_policy_without_inventing_block_ids() {
    let mut base = ordered(&[1]);
    base["meta"]["type"] = json!("snippet");
    base["id"] = json!(crate::domain::project::named_items::id(
        "snippet", "example"
    ));
    base["content"] = json!({"key":"example","paragraph":{"type":"paragraph","content":[{"type":"text","text":"Before"}]}});
    let mut left = base.clone();
    let mut right = base.clone();
    left["content"]["paragraph"]["content"][0]["text"] = json!("Left");
    right["content"]["paragraph"]["content"][0]["text"] = json!("Right");
    let mut context = context();
    context.target = base["id"].as_str().unwrap().to_owned();
    let result = merge_item(&context, Some(&base), Some(&left), Some(&right)).unwrap();
    assert_eq!(result.candidate, ItemCandidate::Existing(right));
    assert_eq!(result.decisions.len(), 1);
}

fn boxed(number: usize, blocks: Vec<Value>) -> Value {
    let mut node = json!({"type":"box","attrs":{"id":id(number),"kind":"plain"}});
    node["content"] = Value::Array(blocks);
    node
}

#[test]
fn a_block_moved_into_a_container_keeps_the_other_sides_edit() {
    let base = page(vec![
        paragraph(1, "Moved"),
        boxed(2, vec![paragraph(3, "Inside")]),
    ]);
    let left = page(vec![boxed(
        2,
        vec![paragraph(3, "Inside"), paragraph(1, "Moved")],
    )]);
    let mut right = base.clone();
    change_text(&mut right, 0, "Moved and edited");
    for (left, right) in [(&left, &right), (&right, &left)] {
        let result = merge(&base, left, right);
        assert_eq!(result.decisions, []);
        assert_eq!(
            blocks(&result),
            &vec![boxed(
                2,
                vec![paragraph(3, "Inside"), paragraph(1, "Moved and edited")]
            )]
        );
    }
}

#[test]
fn a_block_moved_into_a_new_container_keeps_the_other_sides_edit() {
    let base = page(vec![paragraph(1, "Indented"), paragraph(2, "Kept")]);
    let left = page(vec![
        boxed(5, vec![paragraph(1, "Indented")]),
        paragraph(2, "Kept"),
    ]);
    let mut right = base.clone();
    change_text(&mut right, 0, "Indented and edited");
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions, []);
    assert_eq!(
        blocks(&result),
        &vec![
            boxed(5, vec![paragraph(1, "Indented and edited")]),
            paragraph(2, "Kept"),
        ]
    );
}

#[test]
fn deleting_a_block_the_other_side_moved_is_a_decision() {
    let base = page(vec![
        paragraph(1, "Moved"),
        boxed(2, vec![paragraph(3, "Inside")]),
    ]);
    let left = page(vec![boxed(
        2,
        vec![paragraph(3, "Inside"), paragraph(1, "Moved")],
    )]);
    let right = page(vec![boxed(2, vec![paragraph(3, "Inside")])]);
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::DeleteEdit);
    assert_eq!(
        blocks(&result),
        &vec![boxed(
            2,
            vec![paragraph(3, "Inside"), paragraph(1, "Moved")]
        )]
    );
}

#[test]
fn both_sides_moving_a_block_merge_its_edits_or_decide_its_place() {
    let base = page(vec![
        paragraph(1, "Moved"),
        boxed(2, vec![paragraph(3, "Two")]),
        boxed(4, vec![paragraph(5, "Four")]),
    ]);
    let mut left = page(vec![
        boxed(2, vec![paragraph(3, "Two"), paragraph(1, "Moved")]),
        boxed(4, vec![paragraph(5, "Four")]),
    ]);
    let mut right = left.clone();
    right["content"]["document"]["content"][0]["content"][1]["content"][0]["text"] =
        json!("Moved and edited");
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions, []);
    assert_eq!(
        blocks(&result)[0],
        boxed(
            2,
            vec![paragraph(3, "Two"), paragraph(1, "Moved and edited")]
        )
    );
    left = page(vec![
        boxed(2, vec![paragraph(3, "Two")]),
        boxed(4, vec![paragraph(5, "Four"), paragraph(1, "Moved")]),
    ]);
    let result = merge(&base, &left, &right);
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].kind, DecisionKind::Block);
}
