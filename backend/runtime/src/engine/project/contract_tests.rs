// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Persistence regressions formerly exercised through the Node storage facade.
use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::{config, item, item_mutations, write};

struct Project {
    root: PathBuf,
    temporary: tempfile::TempDir,
    baseline: usize,
}

impl Project {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        config::open_or_create_project(&root, "UTC").unwrap();
        let baseline = item::list_project_item_ids(&root).unwrap().len();
        Self {
            root,
            temporary,
            baseline,
        }
    }

    fn close(self) {
        self.temporary.close().unwrap();
    }

    fn write(&self, input: Value, revision: Option<&str>) -> Value {
        write::write_item_input_typed(&self.root, input, revision, "2026-10-09").unwrap()["item"]
            .clone()
    }

    fn pending(&self) -> Vec<Value> {
        let mut paths = fs::read_dir(self.root.join("analytics/events"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
            .into_iter()
            .flat_map(|path| {
                fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .filter(|entry| entry["state"] == "pending")
                    .map(|entry| entry["event"].clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

fn document(text: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
}

#[test]
fn task_completion_events_follow_transitions_without_counting_task_edits_as_writing() {
    let project = Project::new();
    let note = project.write(
        json!({"meta": {"type": "page", "title": "Chapter"},
        "content": document("First paragraph")}),
        None,
    );
    let id = note["id"].as_str().unwrap();
    let task = project.write(json!({"meta":{"type":"task","title":"Revise chapter"},"content":{"noteId":id,"text":"Revise chapter"}}), None);
    let task_id = task["id"].as_str().unwrap();
    assert_eq!(project.pending().len(), 1);
    let complete_content =
        json!({"noteId":id,"text":"Revise chapter","completedAt":"2026-10-09T10:01:00.000Z"});
    let complete = project.write(
        json!({"id":task_id,"meta":{},"content":complete_content}),
        task["revision"].as_str(),
    );
    let events = project.pending();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["itemId"], id);
    assert_eq!(events[1]["sourceItemId"], task_id);
    assert_eq!(
        events[1]["id"],
        format!(
            "{task_id}:{}:completed",
            complete["revision"].as_str().unwrap()
        )
    );
    let unchanged = project.write(
        json!({"id":task_id,"meta":{},"content":complete_content}),
        complete["revision"].as_str(),
    );
    assert_eq!(project.pending().len(), 2);
    let reopened = project.write(
        json!({"id":task_id,"meta":{},"content":{"noteId":id,"text":"Revise chapter"}}),
        unchanged["revision"].as_str(),
    );
    project.write(
        json!({"id":task_id,"meta":{},"content":complete_content}),
        reopened["revision"].as_str(),
    );
    assert_eq!(
        project
            .pending()
            .iter()
            .filter(|e| e["kind"] == "task_completed")
            .count(),
        2
    );
    assert_eq!(
        project
            .pending()
            .iter()
            .filter(|e| e["kind"] == "content")
            .count(),
        1
    );
    assert_eq!(
        item::read_project_item(&project.root, id).unwrap().unwrap()["revision"],
        note["revision"]
    );
    project.close();
}

#[test]
fn compact_canonical_files_history_and_word_deltas_survive_reopening() {
    let project = Project::new();
    let first = project.write(
        json!({"meta": {"type": "page", "title": "Portable note"},
        "content": document("A portable note has five words.")}),
        None,
    );
    let id = first["id"].as_str().unwrap();
    let uuid = uuid::Uuid::parse_str(id).unwrap();
    assert_eq!(uuid.get_version_num(), 7);
    assert_eq!(first["analytics"]["current"]["wordCount"], 6);
    let directory = project.root.join("items").join(&id[..2]).join(id);
    let source = fs::read_to_string(directory.join("item.json")).unwrap();
    assert!(!source.contains('\n'));
    let ids = item::list_project_item_ids(&project.root).unwrap();
    assert_eq!(ids.len(), project.baseline + 1);
    assert!(ids.iter().any(|value| value == id));
    assert!(
        ids.contains(
            &config::read_default_workspace(&project.root).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        )
    );
    let changed = project.write(
        json!({"id": id, "meta": {},
        "content": document("A revised portable note.")}),
        first["revision"].as_str(),
    );
    let events = project.pending();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["wordsAdded"], 0);
    assert_eq!(events[1]["netWordChange"], -2);
    let snapshots = fs::read_dir(directory.join("history"))
        .unwrap()
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 1);
    let snapshot: Value =
        serde_json::from_slice(&fs::read(snapshots[0].as_ref().unwrap().path()).unwrap()).unwrap();
    assert_eq!(snapshot, first);
    config::open_or_create_project(&project.root, "UTC").unwrap();
    assert_eq!(
        item::read_project_item(&project.root, id).unwrap().unwrap(),
        changed
    );
    project.close();
}

#[test]
fn nested_blocks_keep_unique_stable_ids_when_content_is_copied() {
    fn collect(node: &Value, ids: &mut Vec<String>) {
        if node["type"].as_str().is_some_and(|kind| kind != "doc") {
            let id = node["attrs"]["id"].as_str().unwrap();
            assert!(super::identity::is_item_id(id));
            ids.push(id.to_owned());
        }
        for child in node["content"].as_array().into_iter().flatten() {
            collect(child, ids);
        }
    }

    let project = Project::new();
    let copied = uuid::Uuid::now_v7().to_string();
    let page = project.write(json!({"meta": {"type": "page", "title": "Nested"}, "content": {
        "type": "doc", "content": [
            {"type": "paragraph", "attrs": {"id": copied}},
            {"type": "code_block", "attrs": {"language": "javascript"}},
            {"type": "horizontal_rule"},
            {"type": "list", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "attrs": {"id": copied}}
            ]}]},
            {"type": "table", "attrs": {"columnWidthsPercent": [100]}, "content": [
                {"type": "table_row", "content": [{"type": "table_cell", "content": [{"type": "paragraph"}]}]}
            ]}
        ]}}), None);

    let mut ids = Vec::new();
    collect(&page["content"], &mut ids);
    assert_eq!(ids.len(), 10);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        ids.len()
    );
    assert_eq!(ids[0], copied);
    assert_eq!(
        item::read_project_item(&project.root, page["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["content"],
        page["content"]
    );
    let invalid = write::write_item_input_typed(
        &project.root,
        json!({"meta": {"type": "page"}, "content": {
            "type": "doc", "content": [{"type": "heading", "attrs": {"level": 7}}]
        }}),
        None,
        "2026-10-09",
    )
    .unwrap_err();
    assert!(invalid.to_string().contains("Heading level"), "{invalid}");
    assert_eq!(
        item::list_project_item_ids(&project.root).unwrap().len(),
        project.baseline + 1
    );
    project.close();
}

#[test]
fn archiving_a_target_preserves_authored_links_and_direct_pin_membership() {
    let project = Project::new();
    let target = project.write(
        json!({"meta": {"type": "page", "title": "Target"}, "content": document("Target")}),
        None,
    );
    let target_id = target["id"].as_str().unwrap();
    let source = project.write(json!({"meta": {"type": "page", "title": "Source"}, "content": {
        "type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Target",
            "marks": [{"type": "link", "attrs": {"href": format!("/?note={target_id}")}}]}]}]
    }}), None);
    let view = project.write(
        json!({"meta": {"type": "workspace", "title": "Saved view"}, "content": crate::engine::project::workspace::initial_content()}),
        None,
    );
    let main = config::read_default_workspace(&project.root).unwrap();
    let scope = main["id"].as_str().unwrap();
    let pinned = project.write(
        json!({"id": target_id, "meta": {}, "pins": [view["id"], scope, view["id"]]}),
        target["revision"].as_str(),
    );
    assert_eq!(pinned["pins"], json!([scope, view["id"]]));
    item_mutations::archive_item(&project.root, target_id, "2026-10-09").unwrap();
    assert_eq!(
        item::read_project_item(&project.root, target_id)
            .unwrap()
            .unwrap()["meta"]["state"],
        "archived"
    );
    assert_eq!(
        item::read_project_item(&project.root, source["id"].as_str().unwrap())
            .unwrap()
            .unwrap(),
        source
    );
    project.close();
}

#[test]
fn settings_and_dated_goals_do_not_emit_prose_activity() {
    let project = Project::new();
    let setting = project.write(json!({"meta": {"type": "setting", "title": "Portable setting"}, "content": {"color": "blue"}}), None);
    assert_eq!(setting["content"], json!({"color": "blue"}));
    assert_eq!(setting["analytics"]["current"]["wordCount"], 0);
    let goal = json!({"meta": {"type": "goal", "title": "Novel challenge"}, "content": {
        "kind": "writing_challenge", "scope": "all_notes", "startDate": "2026-09-01",
        "endDate": "2026-10-31", "dailyWordTarget": 500
    }});
    let saved = project.write(goal.clone(), None);
    assert_eq!(saved["content"]["dailyWordTarget"], 500);
    let mut invalid = goal;
    invalid["content"]["dailyWordTarget"] = json!(0);
    let error =
        write::write_item_input_typed(&project.root, invalid, None, "2026-10-09").unwrap_err();
    assert!(
        error.to_string().contains("invalid configuration"),
        "{error}"
    );
    assert_eq!(project.pending(), [] as [Value; 0]);
    assert_eq!(
        item::list_project_item_ids(&project.root).unwrap().len(),
        project.baseline + 2
    );
    project.close();
}
