// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Result-level preservation corpus for the structured query conversion.

use super::support::{Fixture, page_content};
use crate::structured_query::document::{Comparison, QueryDocument};
use crate::structured_query::fixtures::{
    archived, asc, at_least, at_most, comparison, criteria, desc, exclude, field, greater_than,
    has, label, less_than, phrase, pinned, tag, term,
};
use axum::http::StatusCode;
use serde_json::json;

#[test]
fn structured_literals_and_range_bounds_are_data_not_expression_syntax() {
    let api = Fixture::new(false);
    api.page("Mary Ann", "War (1939) and peace");
    api.page("Mary Jane", "Another record");
    api.page("Zebra", "Later title");

    for (predicate, expected) in [
        (
            json!({"kind":"text","mode":"phrase","value":"War (1939)"}),
            vec!["Mary Ann"],
        ),
        (
            json!({"kind":"text","mode":"phrase","value":"AND"}),
            vec!["Mary Ann"],
        ),
        (
            json!({"kind":"field","field":"title","operator":"gte","value":"Mary Jane"}),
            vec!["Mary Jane", "Zebra"],
        ),
        (
            json!({"kind":"field","field":"title","operator":"gt","value":"Mary Jane"}),
            vec!["Zebra"],
        ),
        (
            json!({"kind":"field","field":"title","operator":"lt","value":"Mary Jane"}),
            vec!["Mary Ann"],
        ),
        (
            json!({"kind":"field","field":"title","operator":"lte","value":"Mary Jane"}),
            vec!["Mary Ann", "Mary Jane"],
        ),
        (
            json!({"kind":"field","field":"title","operator":"gte","value":"Zebra OR title:*"}),
            vec![],
        ),
        (
            json!({"kind":"field","field":"title","operator":"lt","value":"*"}),
            vec![],
        ),
    ] {
        let mut query = serde_json::to_value(QueryDocument::active_pages()).unwrap();
        query["clauses"]
            .as_array_mut()
            .unwrap()
            .push(json!({"polarity":"required","predicate":predicate}));
        query["sort"] = json!([{"field":"title","direction":"asc"}]);
        assert_eq!(
            api.call("validate_query", json!({"document":query}))["valid"],
            true,
            "{predicate}"
        );
        let results = api.call("query_items", json!({"query":query}));
        let titles: Vec<_> = results["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["meta"]["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, expected, "{predicate}");
    }
}

struct QueryFixture {
    api: Fixture,
    scope: String,
    target: String,
    created: String,
}

impl QueryFixture {
    fn new() -> Self {
        let api = Fixture::new(false);
        api.call(
            "create_property",
            json!({"key":"author","valueType":"text"}),
        );
        let scope = api.call(
            "create_item",
            json!({"meta":{"type":"workspace","title":"Query corpus"},"content":{}}),
        );
        let target = api.page("Target", "A reference destination");
        let alpha = api.call("create_item", json!({
            "meta":{"type":"page","title":"Alpha","author":"Mary Grey","priority":2},
            "tags":["history","births/1900"], "pins":[scope["id"]],
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["quote"]},"content":[
                {"type":"text","text":"climate change evidence café C# "},
                {"type":"text","text":"Source","marks":[{"type":"link","attrs":{"href":format!("/?note={}",target["id"].as_str().unwrap())}}]}
            ]}]}}
        }));
        api.call("create_item", json!({"meta":{"type":"page","title":"Beta","author":"Robert","priority":3},"tags":["history"],"content":page_content("climate research and social change")}));
        api.call("create_item", json!({"meta":{"type":"page","title":"Gamma","author":"Mary Grey","priority":5},"tags":["science"],"content":page_content("climate policy observations")}));
        api.call("create_item", json!({"meta":{"type":"page","state":"archived","title":"Archived"},"content":page_content("climate change archive")}));

        Self {
            scope: scope["id"].as_str().unwrap().into(),
            target: target["id"].as_str().unwrap().into(),
            created: alpha["createdAt"].as_str().unwrap()[..10].into(),
            api,
        }
    }

    fn titles(&self, document: QueryDocument, sorted: bool) -> Vec<String> {
        self.titles_in_scope(document, sorted, None)
    }

    fn titles_in_scope(
        &self,
        document: QueryDocument,
        sorted: bool,
        scope: Option<&str>,
    ) -> Vec<String> {
        let document = self.validate_document(document, scope);
        let result = self
            .api
            .call("query_items", json!({"query":document,"pinScopeId":scope}));
        let mut titles = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["meta"]["title"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        if sorted {
            titles.sort();
        }
        titles
    }

    fn validate_document(&self, document: QueryDocument, scope: Option<&str>) -> QueryDocument {
        let validation = self.api.call(
            "validate_query",
            json!({"document":document,"pinScopeId":scope}),
        );
        assert_eq!(validation["valid"], true, "{document:?}: {validation}");
        document
    }
}

#[test]
fn complete_condition_baseline_preserves_native_membership_and_order() {
    let f = QueryFixture::new();
    let cases: &[(QueryDocument, &[&str])] = &[
        (criteria([term("*")]), &["Alpha", "Beta", "Gamma", "Target"]),
        (criteria([term("climate^2")]), &["Alpha", "Beta", "Gamma"]),
        (
            criteria([term("climate"), term("change")]),
            &["Alpha", "Beta"],
        ),
        (criteria([phrase("climate change")]), &["Alpha"]),
        (
            criteria([phrase("climate"), phrase("change")]),
            &["Alpha", "Beta"],
        ),
        (
            criteria([term("climate"), exclude(term("research"))]),
            &["Alpha", "Gamma"],
        ),
        (criteria([tag("history")]), &["Alpha", "Beta"]),
        (criteria([tag("births/1900")]), &["Alpha"]),
        (
            criteria([exclude(tag("science"))]),
            &["Alpha", "Beta", "Target"],
        ),
        (criteria([label("quote")]), &["Alpha"]),
        (
            criteria([exclude(label("quote"))]),
            &["Beta", "Gamma", "Target"],
        ),
        (
            criteria([field("author", "Mary Grey")]),
            &["Alpha", "Gamma"],
        ),
        (
            criteria([exclude(field("author", "Mary Grey"))]),
            &["Beta", "Target"],
        ),
        (criteria([at_least("priority", "3")]), &["Beta", "Gamma"]),
        (criteria([greater_than("priority", "3")]), &["Gamma"]),
        (criteria([at_most("priority", "3")]), &["Alpha", "Beta"]),
        (criteria([less_than("priority", "3")]), &["Alpha"]),
        (
            criteria([exclude(at_least("priority", "3"))]),
            &["Alpha", "Target"],
        ),
        (criteria([has("author")]), &["Alpha", "Beta", "Gamma"]),
        (criteria([exclude(has("author"))]), &["Target"]),
        (criteria([pinned()]), &["Alpha"]),
        (criteria([exclude(pinned())]), &["Beta", "Gamma", "Target"]),
        (
            criteria([field("created", "today")]),
            &["Alpha", "Beta", "Gamma", "Target"],
        ),
        (
            criteria([field("created", "last_7d")]),
            &["Alpha", "Beta", "Gamma", "Target"],
        ),
        (criteria([phrase("café")]), &["Alpha"]),
        (criteria([phrase("C#")]), &["Alpha"]),
    ];
    for (condition, expected) in cases {
        eprintln!("Structured query baseline: {condition:?}");
        assert_eq!(
            f.titles(
                QueryDocument::active_pages().compose(condition).unwrap(),
                true
            ),
            *expected,
            "{condition:?}"
        );
    }
    for key in ["link", "reference"] {
        assert_eq!(
            f.titles(
                criteria([
                    field("type", "page"),
                    field("state", "active"),
                    crate::structured_query::fixtures::field(key, &f.target)
                ]),
                true
            ),
            ["Alpha"]
        );
    }
    for prefix in [&f.created[..4], &f.created[..7], &f.created] {
        assert_eq!(
            f.titles(
                criteria([
                    field("type", "page"),
                    field("state", "active"),
                    field("created", prefix)
                ]),
                true
            ),
            ["Alpha", "Beta", "Gamma", "Target"]
        );
    }
}

#[test]
fn complete_condition_baseline_preserves_scopes_sorts_and_diagnostics() {
    let f = QueryFixture::new();
    assert_eq!(
        f.titles(criteria([field("type", "page"), archived()]), true),
        ["Archived"]
    );
    assert_eq!(
        f.titles(criteria([field("type", "page"), exclude(archived())]), true),
        ["Alpha", "Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles(
            criteria([field("type", "page"), field("state", "active")])
                .sorted([desc("priority"), asc("title")]),
            false
        ),
        ["Gamma", "Beta", "Alpha", "Target"]
    );
    assert_eq!(
        f.titles(
            criteria([field("type", "page"), field("state", "active")])
                .sorted([asc("priority"), desc("title")]),
            false
        ),
        ["Alpha", "Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles_in_scope(
            criteria([field("type", "page"), field("state", "active"), pinned()]),
            true,
            Some(&f.scope)
        ),
        ["Alpha"]
    );
    assert_eq!(
        f.titles_in_scope(
            criteria([
                field("type", "page"),
                field("state", "active"),
                exclude(pinned())
            ]),
            true,
            Some(&f.scope)
        ),
        ["Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles_in_scope(
            criteria([field("type", "page"), field("state", "active")]).sorted([desc("priority")]),
            false,
            Some(&f.scope)
        ),
        ["Alpha", "Gamma", "Beta", "Target"]
    );
    let mut expected = crate::domain::project::named_items::list(f.api.root.path(), "property")
        .unwrap()
        .into_iter()
        .map(|item| item["meta"]["title"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    expected.extend(["All notes".to_owned(), "Query corpus".to_owned()]);
    expected.sort();
    assert_eq!(
        f.titles(criteria([exclude(field("type", "page"))]), true),
        expected
    );
    expected.extend(["Alpha", "Archived", "Beta", "Gamma", "Target"].map(str::to_owned));
    expected.sort();
    assert_eq!(f.titles(criteria([]), true), expected);
    for document in [
        criteria([field("unknown_field", "value")]),
        criteria([term("priority:")]),
        criteria([term("AND")]),
        criteria([term("(climate)")]),
        criteria([field("created", "2026/10/18")]),
    ] {
        let message = f.api.error(
            "query_items",
            json!({"query":document}),
            StatusCode::BAD_REQUEST,
        );
        assert!(!message.is_empty(), "{document:?}");
        let parsed = f.api.call("validate_query", json!({"document":document}));
        assert!(parsed["valid"] == false, "{document:?}: {parsed}");
        assert_ne!(parsed["diagnostics"], json!([]));
    }
}

#[test]
fn structured_query_rpc_validates_objects_and_exposes_native_capabilities() {
    let f = QueryFixture::new();
    let document = json!({"version":1,"clauses":[
        {"polarity":"required","predicate":{"kind":"field","field":"type","operator":"match","value":"page"}},
        {"polarity":"excluded","predicate":{"kind":"field","field":"author","operator":"match","value":"Mary Grey"}},
        {"polarity":"required","predicate":{"kind":"field","field":"priority","operator":"gte","value":"3"}}
    ],"sort":[{"field":"title","direction":"desc"}]});
    assert_eq!(
        f.api.call("validate_query", json!({"document":document}))["valid"],
        true
    );
    let hits = f.api.call("query_items", json!({"query":document}));
    assert_eq!(hits["count"], 1);
    assert_eq!(hits["items"][0]["meta"]["title"], "Beta");
    let capabilities = f.api.call("read_query_capabilities", json!({}));
    let capabilities = capabilities["fields"].as_array().unwrap();
    let field = |key: &str| {
        capabilities
            .iter()
            .find(|field| field["key"] == key)
            .unwrap()
    };
    assert_eq!(
        field("author")["operators"],
        json!(["match", "gt", "gte", "lt", "lte"])
    );
    assert_eq!(field("author")["canTestPresence"], true);
    let relative = json!([
        "today",
        "yesterday",
        "tomorrow",
        "last_7d",
        "last_30d",
        "next_7d",
        "next_30d"
    ]);
    assert_eq!(field("created")["relativeValues"], relative);
    // Every date field offers relative dates, including declared properties.
    assert_eq!(field("due_date")["relativeValues"], relative);
    assert_eq!(field("open_tasks")["valueType"], "number");
    assert_eq!(field("open_tasks")["canSort"], true);
    assert_eq!(field("reference")["referencesItems"], true);
    assert_eq!(field("body")["canSort"], false);
    assert_eq!(field("body")["canTestPresence"], false);
    assert_eq!(field("title")["canSort"], true);
    assert_eq!(field("id")["valueType"], "text"); // Searchable despite being read-only.
    let keys: std::collections::HashSet<_> = capabilities
        .iter()
        .map(|field| field["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), capabilities.len());
}

#[test]
fn execution_and_validation_return_the_same_condition_diagnostics() {
    let f = QueryFixture::new();
    for (field, value) in [
        ("removed", "Mary"),
        ("created", "2026/10/18"),
        ("priority", "not-a-number"),
    ] {
        let document = json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":field,"operator":"match","value":value}}],"sort":[]});
        let validation = f.api.call("validate_query", json!({"document":document}));
        assert_eq!(validation["valid"], false, "{field}: {validation}");
        assert!(
            validation["diagnostics"][0]["path"]
                .as_str()
                .unwrap()
                .starts_with("/clauses/0/predicate")
        );
        let (status, _, bytes) = f.api.request(
            "POST",
            "/api/query_items",
            json!({"query":document}).to_string(),
        );
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let failure: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(failure["diagnostics"], validation["diagnostics"]);
    }
    let unsupported = json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"has","field":"body"}}],"sort":[]});
    assert_eq!(
        f.api
            .call("validate_query", json!({"document":unsupported}))["valid"],
        false
    );
    for document in [
        json!("#history"),
        json!({"version":1,"clauses":[],"sort":[],"raw":"#history"}),
        json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"raw","value":"#history"}}],"sort":[]}),
    ] {
        f.api.error(
            "validate_query",
            json!({"document":document}),
            StatusCode::BAD_REQUEST,
        );
    }
    let document = json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":"author","operator":"match","value":">Mary"}}],"sort":[]});
    assert_eq!(
        f.api.call("validate_query", json!({"document":document}))["valid"],
        true
    );
}

#[test]
fn date_comparisons_validate_calendar_values_and_resolve_relative_boundaries() {
    let f = QueryFixture::new();
    f.api.call(
        "create_property",
        json!({"key":"journal_day","valueType":"date"}),
    );
    for field in ["created", "modified", "completed", "journal_day"] {
        for value in ["bad-date", "2026-99", "2026-02-30", "2026x10", "2026/10/01"] {
            let document = json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":field,"operator":"lt","value":value}}],"sort":[]});
            let validation = f.api.call("validate_query", json!({"document": document}));
            assert_eq!(validation["valid"], false, "{field} {value}: {validation}");
            assert_eq!(
                validation["diagnostics"][0]["path"],
                "/clauses/0/predicate/value"
            );
            f.api.error(
                "query_items",
                json!({"query": document}),
                StatusCode::BAD_REQUEST,
            );
        }
    }
    for value in ["2024-02-29", "2026-10", "2026"] {
        let document = json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":"journal_day","operator":"gte","value":value}}],"sort":[]});
        assert_eq!(
            f.api.call("validate_query", json!({"document":document}))["valid"],
            true,
            "{value}"
        );
    }
    for relative in ["today", "yesterday", "last_7d"] {
        let boundary = match relative {
            "today" => chrono::Utc::now(),
            "yesterday" => chrono::Utc::now() - chrono::Duration::days(1),
            _ => chrono::Utc::now() - chrono::Duration::days(6),
        }
        .format("%Y-%m-%d")
        .to_string();
        for operator in [
            Comparison::Gt,
            Comparison::Gte,
            Comparison::Lt,
            Comparison::Lte,
        ] {
            assert_eq!(
                f.titles(
                    criteria([
                        field("type", "page"),
                        field("state", "active"),
                        comparison("created", operator, relative)
                    ]),
                    false
                ),
                f.titles(
                    criteria([
                        field("type", "page"),
                        field("state", "active"),
                        comparison("created", operator, &boundary)
                    ]),
                    false
                ),
                "{relative} {operator:?}"
            );
        }
    }
    assert_eq!(
        f.titles(
            criteria([
                field("type", "page"),
                field("state", "active"),
                at_least("created", "today")
            ]),
            false
        )
        .len(),
        4
    );
    assert_eq!(
        f.titles(
            criteria([
                field("type", "page"),
                field("state", "active"),
                less_than("created", "today")
            ]),
            false
        ),
        Vec::<String>::new()
    );
}

#[test]
fn passage_membership_is_scoped_in_rust_before_counts_and_paging() {
    let f = QueryFixture::new();
    f.api.call("create_item",json!({"meta":{"type":"page","title":"Another labelled page"},"tags":["science"],"content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["quote"]},"content":[{"type":"text","text":"Another quote"}]}]}}}));
    let query = criteria([field("type", "page"), tag("history")]);
    let result = f.api.call(
        "query_label_passages",
        json!({"label":"quote","query":query,"limit":1}),
    );
    assert_eq!(result["count"], 1);
    assert_eq!(result["items"][0]["noteTitle"], "Alpha");
    let next = f.api.call(
        "query_label_passages",
        json!({"label":"quote","query":query,"limit":1,"offset":1}),
    );
    assert_eq!(next["count"], 1);
    assert_eq!(next["items"], json!([]));
    let none = f.api.call(
        "query_label_passages",
        json!({"label":"quote","query":criteria([tag("missing")]),"limit":1}),
    );
    assert_eq!(none["count"], 0);
}
