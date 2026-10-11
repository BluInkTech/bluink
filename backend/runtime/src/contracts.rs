// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Development-only generation from the types used by Rust dispatch and serde.
use crate::engine::project::types::{FacetKind, ItemState, ItemType};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::PathBuf;
use ts_rs::{Config, TS, TypeVisitor};

const HEADER: &str = "// SPDX-License-Identifier: MIT\n// See backend/contracts/LICENSE.\n// Generated from Rust; do not edit.\n// Run npm run contracts:generate from the repository root.\n\n";

pub(crate) struct Registry {
    config: Config,
    visited: HashSet<std::any::TypeId>,
    declarations: BTreeMap<String, String>,
    operations: BTreeMap<&'static str, (String, String, &'static str)>,
}
impl TypeVisitor for Registry {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if !self.visited.insert(std::any::TypeId::of::<T>()) {
            return;
        }
        if T::output_path().is_some() {
            let name = T::ident(&self.config);
            let declaration = T::decl(&self.config);
            if let Some(existing) = self.declarations.insert(name.clone(), declaration.clone()) {
                assert_eq!(
                    existing, declaration,
                    "Conflicting generated type name: {name}"
                );
            }
        }
        T::visit_dependencies(self);
        T::visit_generics(self);
    }
}
impl Registry {
    pub(crate) fn operation<Q: TS + 'static, R: TS + 'static>(
        &mut self,
        name: &'static str,
        method: &'static str,
    ) {
        self.visit::<Q>();
        self.visit::<R>();
        let request = if method == "GET" {
            "undefined".to_owned()
        } else {
            Q::name(&self.config)
        };
        assert!(
            self.operations
                .insert(name, (request, R::name(&self.config), method))
                .is_none()
        );
    }
}
fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contracts/generated")
}
fn generated() -> Vec<(&'static str, String)> {
    let mut registry = Registry {
        config: Config::new().with_large_int("number"),
        visited: HashSet::new(),
        declarations: BTreeMap::new(),
        operations: BTreeMap::new(),
    };
    crate::application::operation::contracts(&mut registry);
    registry.visit::<crate::engine::publication::PublicationContent>();
    registry.visit::<crate::http::uploads::Uploaded>();
    registry.visit::<crate::engine::project::types::WritingGoalContent>();
    let mut types = HEADER.to_owned();
    for declaration in registry.declarations.values() {
        writeln!(types, "export {declaration}").unwrap();
    }
    types.push_str("\nexport type Api = {\n");
    let mut runtime = HEADER.to_owned() + "export const rpcMethods = {\n";
    for (name, (request, response, method)) in &registry.operations {
        writeln!(
            types,
            "  {name}: {{ method: '{method}'; request: {request}; response: {response} }}"
        )
        .unwrap();
        writeln!(runtime, "  {name}: '{method}',").unwrap();
    }
    types.push_str("}\n");
    runtime.push_str("} as const\n\n");
    for (name, values) in [
        ("ItemType", ItemType::VALUES),
        ("ItemState", ItemState::VALUES),
        ("FacetKind", FacetKind::VALUES),
    ] {
        writeln!(runtime, "export const {name} = {{").unwrap();
        for (key, value) in values {
            writeln!(runtime, "  {}: '{value}',", key.to_uppercase()).unwrap();
        }
        runtime.push_str("} as const\n\n");
    }
    writeln!(
        runtime,
        "export const PublicSystemItemId = {{ NOTES_ORDER: '{}' }} as const",
        crate::engine::project::write::ALL_NOTES_PIN_SCOPE_ID
    )
    .unwrap();
    vec![
        ("types.ts", types),
        ("runtime.ts", runtime),
        ("wire.ts", wire_examples()),
    ]
}
/// Pair request deserialization and response serialization with strict TypeScript checks.
fn wire_examples() -> String {
    use crate::application::{items::PublicItem, operation::Request};
    use serde_json::json;
    let id = "00000000-0000-7000-8000-000000000501";
    let mut source = HEADER.to_owned()
        + "import type { Api, PublicItem, OutlineNode, TaskRecord, ThemeSnapshot } from './types.js'\n\n";
    let requests = [
        ("query_items", json!({"query":"#fiction", "limit":null})),
        ("read_item", json!({"id":id})),
        ("read_items", json!({"ids":[id]})),
        ("create_item", json!({})),
        ("read_export", json!({"jobId":"job"})),
        (
            "mutate_note_task",
            json!({"noteId":id,"version":"revision-1","change":{"kind":"complete","taskId":"task-1"}}),
        ),
        (
            "create_export",
            json!({"source":{"kind":"note","id":id},"format":"pdf","start":false,"themeId":"classic","outsideLinks":"error","profile":"default"}),
        ),
    ];
    for (operation, value) in requests {
        Request::decode(operation, &serde_json::to_vec(&value).unwrap()).unwrap();
        writeln!(
            source,
            "export const {operation} = {value} satisfies Api['{operation}']['request']"
        )
        .unwrap();
    }
    let item: PublicItem = serde_json::from_value(json!({
        "id":id, "revision":"revision-1", "createdAt":"2026-10-10T12:00:00Z", "modifiedAt":"2026-10-10T12:00:00Z",
        "meta":{"type":"page", "title":"Example", "custom":42}, "content":{"content":{"type":"doc","content":[{"type":"paragraph"}]}}
    })).unwrap();
    let task: crate::model::TaskRecord = serde_json::from_value(json!({
        "noteId":id,"taskId":"task-1","revision":"revision-1","noteTitle":"Example","text":"Read the chapter",
        "createdAt":item.created_at,"modifiedAt":item.modified_at,"completedAt":null,"dueDate":null
    })).unwrap();
    let outline = json!({"kind":"page", "pageId":id, "children":[]});
    let _: crate::engine::publication::OutlineNode =
        serde_json::from_value(outline.clone()).unwrap();
    let theme = crate::engine::publication::theme::capture_theme(
        std::path::Path::new("/unused-contract-fixture-root"),
        "classic",
        crate::engine::publication::plan::PublicationFormat::Html,
    )
    .unwrap();
    for (name, ty, value) in [
        ("item", "PublicItem", serde_json::to_value(&item).unwrap()),
        ("task", "TaskRecord", serde_json::to_value(task).unwrap()),
        ("outline", "OutlineNode", outline),
        (
            "theme",
            "ThemeSnapshot",
            serde_json::to_value(theme).unwrap(),
        ),
    ] {
        writeln!(source, "export const {name} = {value} satisfies {ty}").unwrap();
    }
    let serialized = serde_json::to_value(&item).unwrap();
    assert!(serialized.get("revision").is_none());
    assert_eq!(serialized["version"], "revision-1");
    assert!(serialized.get("tags").is_none());
    source
}

#[test]
fn generated_contracts_match_rust_and_wire_examples() {
    for (file, expected) in generated() {
        let actual = std::fs::read_to_string(directory().join(file))
            .expect("Run npm run contracts:generate");
        assert_eq!(
            actual, expected,
            "Run npm run contracts:generate after changing the contract."
        );
    }
}
#[test]
#[ignore = "Explicit developer command writes checked-in contracts."]
fn regenerate() {
    std::fs::create_dir_all(directory()).unwrap();
    for (file, contents) in generated() {
        std::fs::write(directory().join(file), contents).unwrap();
    }
}

#[test]
fn operation_registry_covers_http_fixtures() {
    let mut registry = Registry {
        config: Config::new().with_large_int("number"),
        visited: HashSet::new(),
        declarations: BTreeMap::new(),
        operations: BTreeMap::new(),
    };
    crate::application::operation::contracts(&mut registry);
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../test-fixtures/http-contract.json")).unwrap();
    let names = fixture
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        registry.operations.keys().copied().collect::<Vec<_>>(),
        names
    );
}
