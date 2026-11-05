// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{
    archived, asc, criteria, desc, exclude, field, has, phrase, term,
};

use super::{QueryDocument, SearchOptions, facet_counts, search};
use crate::store::{fields, normalize};
use rusqlite::{Connection, Transaction, params};
use serde_json::{Value, json};

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::store::SCHEMA).unwrap();
    connection
}

fn record(
    tx: &Transaction<'_>,
    id: i64,
    kind: &str,
    title: &str,
    body: &str,
    meta: Value,
) -> String {
    let uuid = format!("01a0e2a0-0000-7000-8000-{id:012}");
    let Value::Object(mut metadata) = meta else {
        panic!("fixture metadata must be an object")
    };
    metadata.insert("type".into(), json!(kind));
    metadata.insert("state".into(), json!("active"));
    metadata.insert("title".into(), json!(title));
    let header = json!({"id":uuid,"revision":format!("r{id}"),"createdAt":"2026-01-01T00:00:00.000Z","modifiedAt":"2026-01-01T00:00:00.000Z","meta":metadata});
    tx.execute(
        "INSERT INTO entity(id,uuid) VALUES(?1,?2)",
        params![id, uuid],
    )
    .unwrap();
    tx.execute("INSERT INTO body(id,content_json) VALUES(?1,'{}')", [id])
        .unwrap();
    tx.execute("INSERT INTO item(id,header_json,body_id,title_sort,created_ms,modified_ms) VALUES(?1,?2,?1,?3,1767225600000,1767225600000)",params![id,header.to_string(),normalize::sort_key(title)]).unwrap();
    tx.execute("INSERT INTO item_projection(item_id,preview,analyzed_text_hash,word_count) VALUES(?1,?2,'test',3)",params![id,body]).unwrap();
    fields::replace(tx, id, &metadata, None).unwrap();
    crate::store::fts::update_title(tx, id, title).unwrap();
    crate::store::fts::update_body(tx, id, body).unwrap();
    uuid
}

fn ids(tx: &Transaction<'_>, document: &QueryDocument) -> Vec<String> {
    search(
        tx,
        document,
        &SearchOptions {
            content: false,
            limit: 100,
            offset: 0,
            pin_scope: None,
            prioritize_pins: false,
            timezone: chrono_tz::UTC,
        },
    )
    .unwrap()
    .items
    .into_iter()
    .map(|hit| hit.id)
    .collect()
}

#[test]
fn fts_matches_terms_across_columns_but_not_phrases_and_replaces_postings() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    let first = record(&tx, 1, "page", "Midnight", "train journey Café", json!({}));
    let second = record(&tx, 2, "page", "Another", "midnight train", json!({}));
    assert_eq!(
        ids(&tx, &criteria([term("midnight"), term("train")])).len(),
        2
    );
    assert_eq!(
        ids(&tx, &criteria([phrase("midnight train")])),
        vec![second]
    );
    assert_eq!(
        ids(&tx, &criteria([phrase("train journey")])),
        vec![first.clone()]
    );
    assert_eq!(ids(&tx, &criteria([term("café")])), vec![first.clone()]);
    assert_eq!(ids(&tx, &criteria([term("cafe")])), Vec::<String>::new());
    crate::store::fts::update_body(&tx, 1, "updated").unwrap();
    assert_eq!(ids(&tx, &criteria([term("café")])), Vec::<String>::new());
    assert_eq!(ids(&tx, &criteria([term("updated")])), vec![first]);
}

#[test]
fn metadata_tokens_keep_array_boundaries_presence_and_scalar_types() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    let first = record(
        &tx,
        1,
        "page",
        "One",
        "",
        json!({"person":"Mary Jane","segments":["red","train"],"rating":0,"checked":false,"empty":[],"nothing":null}),
    );
    record(
        &tx,
        2,
        "page",
        "Two",
        "",
        json!({"person":"Mary","segments":["red train"],"rating":2,"checked":true}),
    );
    assert_eq!(ids(&tx, &criteria([field("person", "Mary")])).len(), 2);
    assert_eq!(
        ids(&tx, &criteria([field("person", "Mary Jane")])),
        vec![first.clone()]
    );
    assert_eq!(
        ids(&tx, &criteria([field("segments", "red train")])).len(),
        1
    );
    assert_eq!(
        ids(
            &tx,
            &criteria([field("rating", "0"), field("checked", "false")])
        ),
        vec![first]
    );
    assert_eq!(ids(&tx, &criteria([has("empty")])), Vec::<String>::new());
    assert_eq!(ids(&tx, &criteria([has("nothing")])), Vec::<String>::new());
}

#[test]
fn option_rank_sort_is_live_paginated_and_uses_earliest_multi_choice() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    record(&tx, 10, "property", "Stage", "", json!({}));
    let content =
        json!({"key":"stage","valueType":"multiSelect","options":["review","draft","final"]});
    tx.execute(
        "UPDATE body SET content_json=?1 WHERE id=10",
        [content.to_string()],
    )
    .unwrap();
    fields::declare(&tx, 10, &content).unwrap();
    let first = record(
        &tx,
        1,
        "page",
        "One",
        "",
        json!({"stage":["final","review"]}),
    );
    let second = record(&tx, 2, "page", "Two", "", json!({"stage":["draft"]}));
    let missing = record(&tx, 3, "page", "Missing", "", json!({}));
    assert_eq!(
        ids(
            &tx,
            &criteria([field("type", "page")]).sorted([asc("stage")])
        ),
        vec![first.clone(), second.clone(), missing.clone()]
    );
    assert_eq!(
        ids(
            &tx,
            &criteria([field("type", "page")]).sorted([desc("stage")])
        ),
        vec![second.clone(), first.clone(), missing.clone()]
    );
    fields::declare(
        &tx,
        10,
        &json!({"key":"stage","valueType":"multiSelect","options":["draft","review","final"]}),
    )
    .unwrap();
    assert_eq!(
        ids(
            &tx,
            &criteria([field("type", "page")]).sorted([asc("stage")])
        ),
        vec![second, first, missing]
    );
    let page = search(
        &tx,
        &criteria([field("type", "page")]).sorted([asc("stage")]),
        &SearchOptions {
            content: false,
            limit: 1,
            offset: 1,
            pin_scope: None,
            prioritize_pins: false,
            timezone: chrono_tz::UTC,
        },
    )
    .unwrap();
    assert_eq!(page.count, 3);
    assert_eq!(page.items.len(), 1);
}

#[test]
fn facet_counts_use_unique_active_owners_and_hierarchical_selection() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    record(&tx, 1, "page", "One", "", json!({}));
    record(&tx, 2, "page", "Two", "", json!({}));
    for (id, key) in [(101, "parent/child"), (102, "other"), (103, "parent")] {
        tx.execute(
            "INSERT INTO entity(id,uuid,named_kind,named_key) VALUES(?1,?2,'tag',?3)",
            params![id, format!("tag-{id}"), key],
        )
        .unwrap();
    }
    for (owner, target) in [(1, 101), (1, 103), (1, 102), (2, 103), (2, 102)] {
        tx.execute(
            "INSERT INTO relation(kind,source_id,target_id) VALUES('tag',?1,?2)",
            params![owner, target],
        )
        .unwrap();
    }
    let counts = facet_counts(&tx, "tag", &["parent".into()], Some("page")).unwrap();
    assert_eq!(
        counts
            .iter()
            .map(|count| (count.tid.as_str(), count.count))
            .collect::<Vec<_>>(),
        vec![("other", 2), ("parent/child", 1)]
    );
}

#[test]
fn catalogue_counts_only_eligible_owners_even_if_an_edge_outlives_its_item() {
    let mut db = connection();
    let tx = db.transaction().unwrap();
    for (id, kind) in [(1, "page"), (2, "page"), (3, "template"), (4, "page")] {
        record(&tx, id, kind, "A", "", json!({}));
    }
    tx.execute_batch(
        "
        UPDATE item SET header_json=json_set(header_json,'$.meta.state','archived') WHERE id=4;
        INSERT INTO entity(id,uuid) VALUES(5,'missing-owner');
        INSERT INTO entity(id,uuid,named_kind,named_key) VALUES
            (10,'a','tag','a'),(11,'b','tag','b'),(12,'c','label','c'),(13,'unused','tag','unused');
        INSERT INTO relation(kind,source_id,target_id) VALUES
            ('tag',1,10),('tag',2,10),('tag',3,10),('tag',4,10),('tag',5,10),
            ('tag',3,11),('tag',4,11),('tag',5,11),
            ('label',1,12),('label',3,12),('label',4,12),('label',5,12);
        ",
    )
    .unwrap();
    let counts = |kind, source| {
        facet_counts(&tx, kind, &[], source)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.tid, entry.count))
            .collect::<Vec<_>>()
    };
    assert_eq!(counts("tag", Some("page")), [("a".to_owned(), 2)]);
    assert_eq!(counts("label", Some("page")), [("c".to_owned(), 1)]);
    assert_eq!(
        counts("tag", Some("template")),
        [("a".to_owned(), 1), ("b".to_owned(), 1)]
    );
    assert_eq!(
        counts("tag", None),
        [("a".to_owned(), 3), ("b".to_owned(), 1)]
    );
    tx.execute("UPDATE item SET header_json=json_set(header_json,'$.meta.state','archived') WHERE type='page'",[]).unwrap();
    assert_eq!(counts("tag", Some("page")), vec![]);
}

#[test]
fn open_choice_sort_uses_unicode_keys_and_checks_existing_values_when_closed() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    tx.execute("INSERT INTO entity(id,uuid) VALUES(10,'field')", [])
        .unwrap();
    fields::declare(&tx, 10, &json!({"key":"stage","valueType":"select"})).unwrap();
    let upper = record(&tx, 1, "page", "One", "", json!({"stage":"Über"}));
    let lower = record(&tx, 2, "page", "Two", "", json!({"stage":"ßeta"}));
    let empty = record(&tx, 3, "page", "Three", "", json!({"stage":""}));
    assert_eq!(
        ids(&tx, &criteria([]).sorted([asc("stage")])),
        vec![lower, upper, empty]
    );
    assert!(
        fields::declare(
            &tx,
            10,
            &json!({"key":"stage","valueType":"select","options":["draft"]})
        )
        .is_err()
    );
}

#[test]
fn negative_and_gapped_phrases_preserve_membership_and_pin_priority() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    let first = record(
        &tx,
        1,
        "page",
        "One",
        "red small train",
        json!({"stage":"a"}),
    );
    let second = record(&tx, 2, "page", "Two", "red train", json!({"stage":"b"}));
    let gap = "x".repeat(40);
    assert_eq!(
        ids(&tx, &criteria([phrase(&format!("red {gap} train"))])),
        vec![first.clone()]
    );
    assert_eq!(
        ids(&tx, &criteria([exclude(phrase("red train"))])),
        vec![first.clone()]
    );
    tx.execute("INSERT INTO entity(id,uuid) VALUES(100,'workspace')", [])
        .unwrap();
    tx.execute(
        "INSERT INTO relation(kind,source_id,target_id) VALUES('pin',2,100)",
        [],
    )
    .unwrap();
    let document = criteria([]).sorted([asc("stage")]);
    let results = search(
        &tx,
        &document,
        &SearchOptions {
            content: false,
            limit: 1,
            offset: 0,
            pin_scope: Some("workspace"),
            prioritize_pins: true,
            timezone: chrono_tz::UTC,
        },
    )
    .unwrap();
    assert_eq!(results.pinned_count, Some(1));
    assert_eq!(results.count, 2);
    assert_eq!(results.items[0].id, second);
    assert_eq!(
        super::matching_ids(
            &tx,
            &document,
            None,
            Some(std::slice::from_ref(&first)),
            chrono_tz::UTC
        )
        .unwrap(),
        vec![first.clone()]
    );
    assert_eq!(
        super::matching_ids(
            &tx,
            &document,
            None,
            Some(&vec![first.clone(); 40_000]),
            chrono_tz::UTC
        )
        .unwrap(),
        vec![first.clone()]
    );
    // Candidates keep their first-seen order and duplicates collapse without
    // crossing SQLite's host-parameter limit.
    let mut candidates = vec![second.clone(), first.clone()];
    candidates.extend(vec![first.clone(); 40_000]);
    let cards =
        super::candidate_page_cards(&tx, &document, None, &candidates, 50, chrono_tz::UTC).unwrap();
    assert_eq!(
        cards.iter().map(|card| card.id.clone()).collect::<Vec<_>>(),
        vec![second, first]
    );
    let plan = tx
        .prepare("EXPLAIN QUERY PLAN SELECT doc FROM item_fts_terms WHERE term=?1")
        .unwrap()
        .query_map([normalize::fts_token("train")], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter().any(|step| step.contains("INDEX 259:")),
        "{plan:?}"
    );
}

#[test]
fn sqlite_membership_matches_frozen_tantivy_fixtures() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    frozen_fixture(&tx);
    // Recorded against the file-backed engine at 6bc74fc2 before its removal.
    let cases: Vec<(QueryDocument, Vec<String>)> =
        serde_json::from_str(include_str!("fixtures/tantivy-membership.json")).unwrap();
    // Typed date expectations correct the former numeric-year and timestamp
    // tokenization defects while preserving the rest of the frozen corpus.
    for (query, expected) in cases {
        let actual = super::matching_ids(&tx, &query, None, None, chrono_tz::UTC).unwrap();
        assert_eq!(actual, expected, "{query:?}");
    }
}

#[test]
fn declaration_creation_checks_inferred_objects_and_rebuilds_only_its_field() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    record(
        &tx,
        1,
        "page",
        "One",
        "",
        json!({"stage":{"nested":"draft"},"other":0}),
    );
    record(
        &tx,
        2,
        "page",
        "Two",
        "",
        json!({"published":"2026-01-02","other":2}),
    );
    for id in [10, 11] {
        tx.execute(
            "INSERT INTO entity(id,uuid) VALUES(?1,?2)",
            params![id, format!("declaration-{id}")],
        )
        .unwrap();
    }
    let invalid = fields::declare(
        &tx,
        10,
        &json!({"key":"stage","valueType":"select","options":["draft"]}),
    );
    assert!(invalid.is_err());
    assert!(!fields::definitions(&tx).unwrap()["stage"].declared);
    let other_before:i64=tx.query_row("SELECT count(*) FROM field_value v JOIN field_definition f ON f.id=v.field_id WHERE f.key='other'",[],|row|row.get(0)).unwrap();
    fields::declare(&tx, 11, &json!({"key":"published","valueType":"date"})).unwrap();
    assert_eq!(
        ids(&tx, &criteria([field("published", "2026-01-02")])).len(),
        1
    );
    assert_eq!(tx.query_row("SELECT count(*) FROM field_value v JOIN field_definition f ON f.id=v.field_id WHERE f.key='other'",[],|row|row.get::<_,i64>(0)).unwrap(),other_before);
    fields::undeclare(&tx, 11).unwrap();
    assert!(!fields::definitions(&tx).unwrap()["published"].declared);
}

#[test]
fn default_lists_read_pages_in_index_order_and_other_scopes_seek_headers() {
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    let mut pages = Vec::new();
    for (id, kind, title) in [(1, "page", "b"), (2, "page", "a"), (3, "task", "c")] {
        pages.push(record(&tx, id, kind, title, "", json!({})));
    }
    tx.execute_batch(
        "UPDATE item SET modified_ms=modified_ms+id;
         UPDATE item SET header_json=json_set(header_json,'$.meta.state','archived') WHERE id=1;",
    )
    .unwrap();
    let plan = |document: QueryDocument| {
        let compiled = super::compile(&tx, &document, None, chrono_tz::UTC).unwrap();
        tx.prepare(&format!(
            "EXPLAIN QUERY PLAN SELECT i.uuid FROM {} WHERE {} ORDER BY {} LIMIT 50 OFFSET 50",
            compiled.from,
            compiled.filter,
            compiled.ordering.join(",")
        ))
        .unwrap()
        .query_map(
            rusqlite::params_from_iter(compiled.parameters.iter()),
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("; ")
    };
    for (document, index) in [
        (
            criteria([field("type", "page"), exclude(archived())]),
            "item_by_modified",
        ),
        (
            criteria([field("type", "page"), field("state", "active")]).sorted([asc("title")]),
            "item_by_title",
        ),
        (
            criteria([field("type", "page"), archived()]).sorted([desc("modified")]),
            "item_by_modified",
        ),
    ] {
        let plan = plan(document.clone());
        assert!(
            plan.contains(&format!(
                "SEARCH i USING INDEX {index} (type=? AND state=?)"
            )) && !plan.contains("TEMP B-TREE"),
            "{document:?}: {plan}"
        );
    }
    for document in [
        criteria([field("type", "page"), exclude(archived())]).sorted([desc("title")]),
        criteria([
            field("type", "page"),
            exclude(archived()),
            crate::structured_query::fixtures::tag("history"),
        ]),
        criteria([exclude(archived())]),
        criteria([field("type", "page")]),
    ] {
        assert!(
            plan(document.clone()).contains("item_query_header"),
            "{document:?}"
        );
    }
    assert_eq!(
        ids(&tx, &criteria([field("type", "page"), exclude(archived())])),
        vec![pages[1].clone()]
    );
    assert_eq!(
        ids(&tx, &criteria([field("type", "page"), archived()])),
        vec![pages[0].clone()]
    );
    assert_eq!(
        ids(&tx, &criteria([exclude(archived())])),
        vec![pages[2].clone(), pages[1].clone()]
    );
}

fn frozen_fixture(tx: &Transaction<'_>) {
    for (id, title, body, meta) in [
        (
            1,
            "Midnight",
            "train journey Café",
            json!({"person":"Mary Jane","rating":0,"checked":false,"segments":["red","train"],"empty":[],"nothing":null,"appointment":"2026-01-01T12:00:00.000Z","day":"2026-01-01"}),
        ),
        (
            2,
            "Other",
            "midnight train",
            json!({"person":"Mary","rating":2,"checked":true,"segments":["red train"],"appointment":"2026-01-02T12:00:00.000Z","day":"2026-01-02"}),
        ),
        (
            3,
            "ΣΟΣ",
            "İstanbul Café red small train",
            json!({"person":"O'Connor","rating":3,"checked":false,"segments":[]}),
        ),
    ] {
        record(tx, id, "page", title, body, meta);
    }
    for (id, key, kind) in [(101, "day", "date"), (102, "appointment", "dateTime")] {
        tx.execute(
            "INSERT INTO entity(id,uuid) VALUES(?1,?2)",
            params![id, format!("field-{id}")],
        )
        .unwrap();
        fields::declare(tx, id, &json!({"key":key,"valueType":kind})).unwrap();
    }
}

#[test]
fn pages_query_task_facts_relative_dates_and_page_links() {
    use crate::domain::project::query::Comparison;
    use crate::structured_query::fixtures::comparison;
    let mut connection = connection();
    let tx = connection.transaction().unwrap();
    tx.execute("INSERT INTO entity(id,uuid) VALUES(10,'field')", [])
        .unwrap();
    fields::declare(&tx, 10, &json!({"key":"in_area","valueType":"page"})).unwrap();
    tx.execute("INSERT INTO entity(id,uuid) VALUES(11,'due')", [])
        .unwrap();
    fields::declare(&tx, 11, &json!({"key":"due_date","valueType":"date"})).unwrap();
    let today = chrono::Utc::now().date_naive();
    let day = |offset: i64| (today + chrono::Duration::days(offset)).to_string();
    let area = record(&tx, 1, "page", "Health", "", json!({}));
    let marathon = record(
        &tx,
        2,
        "page",
        "Run a marathon",
        "",
        json!({"in_area": area, "due_date": day(3)}),
    );
    let bike = record(
        &tx,
        3,
        "page",
        "Fix the bike",
        "",
        json!({"due_date": day(-1)}),
    );
    // The marathon has one task open and one done; the bike has both done.
    for (id, owner, completed) in [
        (5, 2, None),
        (6, 2, Some(1)),
        (7, 3, Some(1)),
        (8, 3, Some(1)),
    ] {
        record(&tx, id, "task", "Step", "", json!({}));
        tx.execute(
            "INSERT INTO task_projection(item_id,note_id,text,completed_ms,due_day) VALUES(?1,?2,'Step',?3,NULL)",
            params![id, owner, completed],
        )
        .unwrap();
    }
    // A stalled project has tasks but none open.
    assert_eq!(
        ids(
            &tx,
            &criteria([field("open_tasks", "0"), has("task_progress")])
        ),
        vec![bike.clone()]
    );
    assert_eq!(
        ids(&tx, &criteria([has("open_tasks")])),
        vec![marathon.clone()]
    );
    assert_eq!(
        ids(
            &tx,
            &criteria([comparison("task_progress", Comparison::Gte, "50")])
        ),
        vec![marathon.clone(), bike.clone()]
    );
    assert_eq!(
        ids(
            &tx,
            &criteria([field("type", "page")]).sorted([asc("task_progress")])
        )[..2],
        [marathon.clone(), bike.clone()]
    );
    // Relative dates cover whole days in the project timezone.
    assert_eq!(
        ids(&tx, &criteria([field("due_date", "next_7d")])),
        vec![marathon.clone()]
    );
    assert_eq!(
        ids(
            &tx,
            &criteria([comparison("due_date", Comparison::Lt, "tomorrow")])
        ),
        vec![bike]
    );
    // A page property links its page like a link in the document does.
    assert_eq!(
        ids(&tx, &criteria([field("in_area", &area)])),
        vec![marathon.clone()]
    );
    assert_eq!(ids(&tx, &criteria([field("link", &area)])), vec![marathon]);
}
