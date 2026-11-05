// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
#![allow(
    clippy::unwrap_used,
    reason = "Shared test fixtures fail immediately on invalid setup."
)]
use super::validation::Snapshot;
use crate::domain::project::{
    candidate::{Dependency, dependencies},
    item, named_items,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};
pub(super) fn id(number: usize) -> String {
    format!("01a0e2a0-0000-7000-8000-{number:012}")
}
pub(super) fn config() -> Value {
    json!({"format":"bluink-project","formatVersion":20,"id":id(9000),"defaultWorkspaceId":id(1),"analytics":{"timezone":"UTC","eventRetentionDays":30}})
}
pub(super) fn record(id: &str, kind: &str, content: Value) -> Value {
    let mut value = json!({"format":"bluink.item","formatVersion":20,"id":id,"revision":"revision-1", "createdAt":"2026-10-30T10:00:00.000Z","modifiedAt":"2026-10-30T10:00:00.000Z","meta":{"type":kind,"state":"active"},"analytics":{}});
    value["content"] = content;
    value
}
pub(super) fn workspace(number: usize) -> Value {
    record(
        &id(number),
        "workspace",
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/workspace-starter.json"
        )))
        .unwrap(),
    )
}
pub(super) fn page(number: usize) -> Value {
    record(
        &id(number),
        "page",
        json!({"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":id(number + 1000)},"content":[{"type":"text","text":"Text"}]}]}}),
    )
}
pub(super) fn publication(number: usize, root: usize) -> Value {
    let mut item = record(
        &id(number),
        "publication",
        json!({
            "kind":"publication", "selectedOutput":"html", "rootPageId":id(root),
            "outline":[], "selectionRules":[], "themeId":"classic",
            "targets":{"pdf":{"profile":"default","outsideLinks":"text-only"},"html":{"outsideLinks":"text-only"},"docx":{"outsideLinks":"text-only"},"epub":{"outsideLinks":"text-only"},"site":{"layout":"documentation","outsideLinks":"text-only"}}
        }),
    );
    item["meta"]["title"] = json!("Publication");
    item
}
pub(super) fn named(kind: &str, key: &str) -> Value {
    let mut content = json!({"key":key});
    if kind == "property" {
        content["valueType"] = json!("text");
    }
    if kind == "snippet" {
        content["paragraph"] =
            json!({"type":"paragraph","content":[{"type":"text","text":"Reference"}]});
    }
    record(&named_items::id(kind, key), kind, content)
}
pub(super) fn snapshot(values: &[Value]) -> Memory {
    let mut items: BTreeMap<_, _> = values
        .iter()
        .map(|value| (value["id"].as_str().unwrap().to_owned(), value.clone()))
        .collect();
    items.entry(id(1)).or_insert_with(|| workspace(1));
    let mut incoming: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (id, value) in &items {
        item::validate_item(value, id).unwrap();
        for dependency in dependencies(value).unwrap() {
            incoming.entry(dependency).or_default().insert(id.clone());
        }
    }
    Memory {
        items,
        incoming,
        reads: RefCell::new(BTreeMap::new()),
        queries: RefCell::new(BTreeMap::new()),
        fail_lookup: false,
        fail_read: None,
    }
}

pub(super) struct Memory {
    pub(super) items: BTreeMap<String, Value>,
    pub(super) incoming: BTreeMap<Dependency, BTreeSet<String>>,
    pub(super) reads: RefCell<BTreeMap<String, usize>>,
    pub(super) queries: RefCell<BTreeMap<Dependency, usize>>,
    pub(super) fail_lookup: bool,
    pub(super) fail_read: Option<String>,
}
impl Snapshot for Memory {
    fn read(&self, id: &str) -> Result<Option<Value>, String> {
        *self.reads.borrow_mut().entry(id.to_owned()).or_default() += 1;
        if self.fail_read.as_deref() == Some(id) {
            return Err("Snapshot read failed".into());
        }
        Ok(self.items.get(id).cloned())
    }
    fn dependents(&self, dependency: &Dependency) -> Result<BTreeSet<String>, String> {
        *self
            .queries
            .borrow_mut()
            .entry(dependency.clone())
            .or_default() += 1;
        if self.fail_lookup {
            return Err("Snapshot dependency index is unavailable".into());
        }
        Ok(self.incoming.get(dependency).cloned().unwrap_or_default())
    }
}
