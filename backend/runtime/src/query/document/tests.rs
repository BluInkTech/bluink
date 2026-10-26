// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use serde_json::json;

fn fields() -> HashSet<String> {
    ["author", "priority"].map(str::to_owned).into()
}

#[test]
fn entry_round_trips_semantics_without_persisting_inferred_sorts() {
    for source in [
        "",
        "*",
        "climate^2 -research",
        "\"two words\" \"other phrase\"",
        "+type:page -is:archived #births/1900 -@quote",
        "author:\"Mary Grey\" -priority:>=3 -has:author is:pinned",
        "created:last_7d modified:2026-10",
        "sort:title-desc sort:priority-asc,title-desc",
        r#"author:"Mary \"Grey\" \\ Research""#,
        "\"C#\" \"café ☕\"",
    ] {
        let parsed = parse(source, &fields());
        let document = parsed
            .document
            .unwrap_or_else(|| panic!("{source}: {:?}", parsed.diagnostics));
        let reparsed = parse(&document.format(), &fields());
        assert_eq!(reparsed.document.as_ref(), Some(&document), "{source}");
    }
    let document = parse("climate", &fields()).document.unwrap();
    assert_eq!(document.sort, []);
    let document = parse("sort:title-desc", &fields()).document.unwrap();
    assert_eq!(document.sort.len(), 1);
    assert_eq!(document.sort[0].field, "title");
    let document = parse("sort:id-desc", &fields()).document.unwrap();
    assert_eq!(document.sort[0].direction, SortDirection::Desc);
}

#[test]
fn source_mapping_uses_browser_utf16_offsets_and_invalid_batches_are_atomic() {
    let parsed = parse("\"café 🦀\" #history", &fields());
    assert!(parsed.document.is_some());
    assert_eq!(parsed.source_map[0].start, 0);
    assert_eq!(parsed.source_map[0].end, 9);
    assert_eq!(parsed.source_map[1].start, 10);
    assert_eq!(parsed.source_map[1].end, 18);
    for source in [
        "\"café 🦀\" priority:",
        "#history unknown:value",
        "#history is:unknown",
        "#history OR",
    ] {
        let parsed = parse(source, &fields());
        assert!(parsed.document.is_none(), "{source}");
        assert!(!parsed.diagnostics.is_empty());
        for error in parsed.diagnostics {
            let start = error.start.unwrap();
            let end = error.end.unwrap();
            assert!(
                start <= end && end <= source.encode_utf16().count(),
                "{source}"
            );
        }
    }
}

#[test]
fn structured_validation_identifies_bad_values_without_discarding_other_conditions() {
    let document: QueryDocument = serde_json::from_value(json!({
        "version":2,
        "clauses":[
            {"polarity":"required","predicate":{"kind":"tag","key":"history"}},
            {"polarity":"excluded","predicate":{"kind":"field","field":"removed","operator":"match","value":"Mary"}},
            {"polarity":"required","predicate":{"kind":"text","mode":"terms","value":"hello author:Mary"}}
        ],
        "sort":[{"field":"title","direction":"asc"},{"field":"title","direction":"desc"}]
    })).unwrap();
    let errors = document.validate_fields(&fields()).unwrap_err();
    assert!(errors.iter().any(|e| e.path == "/version"));
    assert!(errors.iter().any(|e| e.path == "/clauses/1/predicate"));
    assert!(
        errors
            .iter()
            .any(|e| e.path == "/clauses/2/predicate/value")
    );
    assert!(errors.iter().any(|e| e.path == "/sort/1/field"));
    assert!(errors.iter().all(|e| e.start.is_none() && e.end.is_none()));
    assert_eq!(document.clauses.len(), 3);
}

#[test]
fn wire_document_rejects_unknown_shapes_and_operator_strings() {
    for value in [
        json!("#history"),
        json!({"version":1,"clauses":[],"sort":[],"raw":"#history"}),
        json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"raw","value":"#history"}}],"sort":[]}),
        json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":"priority","operator":">=","value":"3"}}],"sort":[]}),
        json!({"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"tag","key":"history","id":"ui-only"}}],"sort":[]}),
        json!({"version":1,"clauses":[],"sort":[{"field":"title","direction":"up"}]}),
    ] {
        assert!(
            serde_json::from_value::<QueryDocument>(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn match_operator_is_independent_of_literal_value_and_polarity() {
    let document: QueryDocument = serde_json::from_value(json!({
        "version":1,"clauses":[{"polarity":"excluded","predicate":{
            "kind":"field","field":"author","operator":"match","value":">Mary"
        }}],"sort":[]
    }))
    .unwrap();
    document.validate_fields(&fields()).unwrap();
    assert_eq!(document.clauses[0].polarity, Polarity::Excluded);
    assert_eq!(
        document.clauses[0].predicate,
        Predicate::Field {
            field: "author".into(),
            operator: Comparison::Match,
            value: ">Mary".into()
        }
    );
    // The old text grammar treats a leading comparison marker as an operator,
    // even inside quotes. That limit must never mutate a structured condition.
    assert_ne!(
        parse(&document.format(), &fields()).document.as_ref(),
        Some(&document)
    );
}

#[test]
fn property_types_validate_scalar_values_without_coercing_them() {
    use crate::engine::project::types::MetadataFieldValueType as Type;
    let schema = super::super::schema::QuerySchema::from_properties(
        [
            ("priority".into(), Type::Number),
            ("reviewed".into(), Type::Checkbox),
        ]
        .into_iter(),
    );
    for (field, value, valid) in [
        ("priority", "9007199254740993", true),
        ("priority", "2.75", true),
        ("priority", "NaN", false),
        ("reviewed", "true", true),
        ("reviewed", "yes", false),
    ] {
        let document: QueryDocument = serde_json::from_value(json!({
            "version":1,"clauses":[{"polarity":"required","predicate":{"kind":"field","field":field,"operator":"match","value":value}}],"sort":[]
        })).unwrap();
        assert_eq!(
            document.validate(&schema).is_ok(),
            valid,
            "{field}: {value}"
        );
        assert_eq!(
            serde_json::to_value(&document).unwrap()["clauses"][0]["predicate"]["value"],
            value
        );
    }
}

#[test]
fn composition_preserves_saved_criteria_and_only_replaces_explicit_sorts() {
    let base = test_query("+type:page #history sort:title-asc");
    let temporary = test_query("@quote -priority:>=3");
    let composed = base.compose(&temporary).unwrap();
    assert_eq!(
        composed.clauses.len(),
        base.clauses.len() + temporary.clauses.len()
    );
    assert_eq!(composed.sort, base.sort);
    assert_eq!(
        base.compose(&test_query("sort:priority-desc"))
            .unwrap()
            .sort[0]
            .field,
        "priority"
    );
    assert_eq!(base.sort[0].field, "title");
    let mut invalid = QueryDocument::empty();
    invalid.version = 2;
    assert!(!invalid.is_empty());
    assert_eq!(base.compose(&invalid).unwrap_err()[0].path, "/version");
}

#[test]
fn cache_identity_is_semantic_and_tracks_the_native_relative_day() {
    let plain = test_query("#history sort:title-asc");
    assert_eq!(
        plain.cache_identity().unwrap(),
        test_query(" +#history  sort:title-asc ")
            .cache_identity()
            .unwrap()
    );
    assert_ne!(
        plain.cache_identity().unwrap(),
        test_query("-#history sort:title-asc")
            .cache_identity()
            .unwrap()
    );
    assert!(
        test_query("created:today")
            .cache_identity()
            .unwrap()
            .contains(":utc:")
    );
    assert!(
        !test_query("created:2026-10")
            .cache_identity()
            .unwrap()
            .contains(":utc:")
    );
}
