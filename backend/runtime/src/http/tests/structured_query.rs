// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Result-level preservation corpus for the structured query conversion.
use super::support::{Fixture, document};
use crate::indexing::coordinator::MainSnapshot;
use crate::query::{self, document::QueryDocument};
use axum::http::StatusCode;
use serde_json::json;
use tantivy::collector::{DocSetCollector, TopDocs};
use tantivy::schema::Value;

const ACTIVE_PAGES: &str = "+type:page +state:active";

#[test]
fn structured_literals_and_range_bounds_are_data_not_expression_syntax() {
    let api = Fixture::new(false);
    api.page("Mary Ann", "War (1939) and peace");
    api.page("Mary Jane", "Another record");
    api.page("Zebra", "Later title");
    api.initialize_index();
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
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["quote"]},"content":[
                {"type":"text","text":"climate change evidence café C# "},
                {"type":"text","text":"Source","marks":[{"type":"link","attrs":{"href":format!("/?note={}",target["id"].as_str().unwrap())}}]}
            ]}]}
        }));
        api.call("create_item", json!({"meta":{"type":"page","title":"Beta","author":"Robert","priority":3},"tags":["history"],"content":document("climate research and social change")}));
        api.call("create_item", json!({"meta":{"type":"page","title":"Gamma","author":"Mary Grey","priority":5},"tags":["science"],"content":document("climate policy observations")}));
        api.call("create_item", json!({"meta":{"type":"page","state":"archived","title":"Archived"},"content":document("climate change archive")}));
        api.initialize_index();
        Self {
            scope: scope["id"].as_str().unwrap().into(),
            target: target["id"].as_str().unwrap().into(),
            created: alpha["createdAt"].as_str().unwrap()[..10].into(),
            api,
        }
    }

    fn titles(&self, source: &str, sorted: bool) -> Vec<String> {
        self.titles_in_scope(source, sorted, None)
    }

    fn titles_in_scope(&self, source: &str, sorted: bool, scope: Option<&str>) -> Vec<String> {
        let document = self.assert_structured_parity(source, scope);
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

    fn assert_structured_parity(&self, source: &str, scope: Option<&str>) -> QueryDocument {
        let document = crate::query::document::test_query(source);
        let validation = self.api.call(
            "validate_query",
            json!({"document":document,"pinScopeId":scope}),
        );
        assert_eq!(validation["valid"], true, "{source}: {validation}");
        let status = self
            .api
            .runtime
            .block_on(self.api.app.initialize(true))
            .unwrap();
        let status: serde_json::Value = serde_json::from_str(&status).unwrap();
        let path = status["indexPath"].as_str().unwrap();
        let MainSnapshot { index, reader, .. } =
            crate::indexing::service::service_index(path).unwrap();
        let fields = crate::indexing::service::service_index(path)
            .unwrap()
            .query_fields;
        let searcher = reader.searcher();
        let (native, native_sorts, native_text) =
            query::compile_native_query(&index, source, &fields, scope).unwrap();
        let (structured, structured_sorts, structured_text) =
            query::compile_query_document(&index, &document, &fields, scope).unwrap();
        assert_eq!(native_text, structured_text);
        assert_eq!(
            native_sorts
                .iter()
                .map(|sort| (&sort.field, sort.descending))
                .collect::<Vec<_>>(),
            structured_sorts
                .iter()
                .map(|sort| (&sort.field, sort.descending))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            searcher.search(native.as_ref(), &DocSetCollector).unwrap(),
            searcher
                .search(structured.as_ref(), &DocSetCollector)
                .unwrap(),
            "membership: {source}"
        );
        if native_sorts.is_empty() {
            // Compare the actual scores and ranked document IDs as well as sets.
            assert_eq!(
                searcher
                    .search(native.as_ref(), &TopDocs::with_limit(100).order_by_score())
                    .unwrap(),
                searcher
                    .search(
                        structured.as_ref(),
                        &TopDocs::with_limit(100).order_by_score()
                    )
                    .unwrap(),
                "ranking: {source}"
            );
        } else {
            assert_eq!(
                query::sort::collect_fast_sorted(&searcher, native.as_ref(), &native_sorts, 100)
                    .unwrap(),
                query::sort::collect_fast_sorted(
                    &searcher,
                    structured.as_ref(),
                    &structured_sorts,
                    100
                )
                .unwrap(),
                "ordering: {source}"
            );
        }
        document
    }
}

#[test]
fn complete_condition_baseline_preserves_native_membership_and_order() {
    let f = QueryFixture::new();
    let cases: &[(&str, &[&str])] = &[
        ("*", &["Alpha", "Beta", "Gamma", "Target"]),
        ("climate^2", &["Alpha", "Beta", "Gamma"]),
        ("climate change", &["Alpha", "Beta"]),
        ("\"climate change\"", &["Alpha"]),
        ("\"climate\" \"change\"", &["Alpha", "Beta"]),
        ("climate -research", &["Alpha", "Gamma"]),
        ("#history", &["Alpha", "Beta"]),
        ("#births/1900", &["Alpha"]),
        ("-#science", &["Alpha", "Beta", "Target"]),
        ("@quote", &["Alpha"]),
        ("-@quote", &["Beta", "Gamma", "Target"]),
        ("author:\"Mary Grey\"", &["Alpha", "Gamma"]),
        ("-author:\"Mary Grey\"", &["Beta", "Target"]),
        ("priority:>=3", &["Beta", "Gamma"]),
        ("priority:>3", &["Gamma"]),
        ("priority:<=3", &["Alpha", "Beta"]),
        ("priority:<3", &["Alpha"]),
        ("-priority:>=3", &["Alpha", "Target"]),
        ("has:author", &["Alpha", "Beta", "Gamma"]),
        ("-has:author", &["Target"]),
        ("is:pinned", &["Alpha"]),
        ("-is:pinned", &["Beta", "Gamma", "Target"]),
        ("created:today", &["Alpha", "Beta", "Gamma", "Target"]),
        ("created:last_7d", &["Alpha", "Beta", "Gamma", "Target"]),
        ("\"café\"", &["Alpha"]),
        ("\"C#\"", &["Alpha"]),
    ];
    for (condition, expected) in cases {
        eprintln!("Native query baseline: {condition}");
        assert_eq!(
            f.titles(&format!("{ACTIVE_PAGES} {condition}"), true),
            *expected,
            "{condition}"
        );
    }
    for field in ["link", "reference"] {
        assert_eq!(
            f.titles(&format!("{ACTIVE_PAGES} {field}:{}", f.target), true),
            ["Alpha"]
        );
    }
    for prefix in [&f.created[..4], &f.created[..7], &f.created] {
        assert_eq!(
            f.titles(&format!("{ACTIVE_PAGES} created:{prefix}"), true),
            ["Alpha", "Beta", "Gamma", "Target"]
        );
    }
}

#[test]
fn complete_condition_baseline_preserves_scopes_sorts_and_diagnostics() {
    let f = QueryFixture::new();
    assert_eq!(f.titles("+type:page is:archived", true), ["Archived"]);
    assert_eq!(
        f.titles("+type:page -is:archived", true),
        ["Alpha", "Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles(
            &format!("{ACTIVE_PAGES} sort:priority-desc,title-asc"),
            false
        ),
        ["Gamma", "Beta", "Alpha", "Target"]
    );
    assert_eq!(
        f.titles(
            &format!("{ACTIVE_PAGES} sort:title-desc sort:priority-asc,title-desc"),
            false
        ),
        ["Alpha", "Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles_in_scope(&format!("{ACTIVE_PAGES} is:pinned"), true, Some(&f.scope)),
        ["Alpha"]
    );
    assert_eq!(
        f.titles_in_scope(&format!("{ACTIVE_PAGES} -is:pinned"), true, Some(&f.scope)),
        ["Beta", "Gamma", "Target"]
    );
    assert_eq!(
        f.titles_in_scope(
            &format!("{ACTIVE_PAGES} sort:priority-desc"),
            false,
            Some(&f.scope)
        ),
        ["Alpha", "Gamma", "Beta", "Target"]
    );
    let mut expected = crate::engine::project::named_items::list(f.api.root.path(), "property")
        .unwrap()
        .into_iter()
        .map(|item| item["meta"]["title"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    expected.extend(["All notes".to_owned(), "Query corpus".to_owned()]);
    expected.sort();
    assert_eq!(f.titles("-type:page", true), expected);
    expected.extend(["Alpha", "Archived", "Beta", "Gamma", "Target"].map(str::to_owned));
    expected.sort();
    assert_eq!(f.titles("", true), expected);
    for source in [
        "unknown_field:value",
        "priority:",
        "AND",
        "(climate)",
        "created:2026/10/18",
    ] {
        let message = f.api.error(
            "query_items",
            json!({"query":crate::query::document::test_query(source)}),
            StatusCode::BAD_REQUEST,
        );
        assert!(!message.is_empty(), "{source}");
        let parsed = f.api.call(
            "validate_query",
            json!({"document":crate::query::document::test_query(source)}),
        );
        assert!(parsed["valid"] == false, "{source}: {parsed}");
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
    let status = f.api.runtime.block_on(f.api.app.initialize(true)).unwrap();
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    let path = status["indexPath"].as_str().unwrap();
    let MainSnapshot { index, reader, .. } = crate::indexing::service::service_index(path).unwrap();
    let fields = crate::indexing::service::service_index(path)
        .unwrap()
        .query_fields;
    let document: QueryDocument = serde_json::from_value(document).unwrap();
    let (query, sorts, _) =
        query::compile_query_document(&index, &document, &fields, None).unwrap();
    let hits =
        query::sort::collect_fast_sorted(&reader.searcher(), query.as_ref(), &sorts, 100).unwrap();
    assert_eq!(hits.len(), 1); // Beta is the only non-Mary page with priority >= 3.
    let title = index.schema().get_field("title").unwrap();
    let hit: tantivy::schema::TantivyDocument = reader.searcher().doc(hits[0]).unwrap();
    assert_eq!(
        hit.get_first(title).and_then(|value| value.as_str()),
        Some("Beta")
    );

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
    assert_eq!(
        field("created")["relativeValues"],
        json!(["today", "yesterday", "last_7d"])
    );
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
        for operator in [">", ">=", "<", "<="] {
            assert_eq!(
                f.titles(
                    &format!("{ACTIVE_PAGES} created:{operator}{relative}"),
                    false
                ),
                f.titles(
                    &format!("{ACTIVE_PAGES} created:{operator}{boundary}"),
                    false
                ),
                "{relative} {operator}"
            );
        }
    }
    assert_eq!(
        f.titles(&format!("{ACTIVE_PAGES} created:>=today"), false)
            .len(),
        4
    );
    assert_eq!(
        f.titles(&format!("{ACTIVE_PAGES} created:<today"), false),
        Vec::<String>::new()
    );
}

#[test]
fn passage_membership_is_scoped_in_rust_before_counts_and_paging() {
    let f = QueryFixture::new();
    f.api.call("create_item",json!({"meta":{"type":"page","title":"Another labelled page"},"tags":["science"],"content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["quote"]},"content":[{"type":"text","text":"Another quote"}]}]}}));
    f.api.settled();
    let query = crate::query::document::test_query("+type:page #history");
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
        json!({"label":"quote","query":crate::query::document::test_query("#missing"),"limit":1}),
    );
    assert_eq!(none["count"], 0);
}
