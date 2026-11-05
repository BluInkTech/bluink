// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{
    asc, at_least, criteria, desc, exclude, field, label, tag,
};

use super::*;
use crate::domain::project::query::Polarity;
use serde_json::json;

fn fields() -> HashSet<String> {
    ["author", "priority"].map(str::to_owned).into()
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
}

#[test]
fn property_types_validate_scalar_values_without_coercing_them() {
    use crate::domain::project::types::MetadataFieldValueType as Type;
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
    let base = criteria([field("type", "page"), tag("history")]).sorted([asc("title")]);
    let temporary = criteria([label("quote"), exclude(at_least("priority", "3"))]);
    let composed = base.compose(&temporary).unwrap();
    assert_eq!(
        composed.clauses.len(),
        base.clauses.len() + temporary.clauses.len()
    );
    assert_eq!(composed.sort, base.sort);
    assert_eq!(
        base.compose(&criteria([]).sorted([desc("priority")]))
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
