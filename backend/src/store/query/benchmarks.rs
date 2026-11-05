// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::{at_least, criteria, desc, term};

use super::*;
use rusqlite::{Connection, params};
use std::time::Instant;

#[test]
#[ignore = "60k durable SQLite query corpus; run explicitly for the storage performance gate."]
fn sixty_thousand_item_query_and_facets() {
    let temporary = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temporary.path().join("queries.sqlite3")).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0;",
        )
        .unwrap();
    connection.execute_batch(crate::store::SCHEMA).unwrap();
    let tx = connection.transaction().unwrap();
    populate_fixture(&tx);
    tx.commit().unwrap();
    connection
        .execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let tx = connection.transaction().unwrap();
    for (name, source) in [
        (
            "number_filter_sort",
            criteria([at_least("priority", "50")]).sorted([desc("priority")]),
        ),
        ("broad_full_text", criteria([term("common")])),
    ] {
        let query = source;
        let compiled = compile(&tx, &query, None, chrono_tz::UTC).unwrap();
        let explain = format!(
            "EXPLAIN QUERY PLAN {}SELECT i.uuid FROM {} WHERE {} ORDER BY {} LIMIT 50",
            compiled.with(),
            compiled.from,
            compiled.filter,
            compiled.ordering.join(",")
        );
        let plan = tx
            .prepare(&explain)
            .unwrap()
            .query_map(
                rusqlite::params_from_iter(compiled.parameters.iter()),
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        eprintln!("SQL60k {name} plan={plan:?}");
        let mut samples = Vec::new();
        for iteration in 0..110 {
            let start = Instant::now();
            let result = search(
                &tx,
                &query,
                &SearchOptions {
                    content: false,
                    limit: 50,
                    offset: 0,
                    pin_scope: None,
                    prioritize_pins: false,
                    timezone: chrono_tz::UTC,
                },
            )
            .unwrap();
            assert_eq!(result.items.len(), 50);
            if iteration >= 10 {
                samples.push(start.elapsed());
            }
        }
        samples.sort();
        eprintln!(
            "SQL60k {name} p95_ms={:.2}",
            samples[94].as_secs_f64() * 1000.0
        );
    }
    for selected in [
        vec!["topic/42".to_owned()],
        vec!["topic".to_owned()],
        Vec::new(),
    ] {
        let mut samples = Vec::new();
        for iteration in 0..110 {
            let start = Instant::now();
            let result = facet_counts(&tx, "tag", &selected, Some("page")).unwrap();
            assert!(!result.is_empty());
            if iteration >= 10 {
                samples.push(start.elapsed());
            }
        }
        samples.sort();
        eprintln!(
            "SQL60k facets={selected:?} p95_ms={:.2}",
            samples[94].as_secs_f64() * 1000.0
        );
    }
    measure_catalogue(&tx);
    let plan=tx.prepare("EXPLAIN QUERY PLAN SELECT id FROM entity WHERE named_kind=?1 AND named_key>=?2 AND named_key<?3").unwrap().query_map(params!["tag","topic/","topic0"],|row|row.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("SEARCH entity USING COVERING INDEX")),
        "{plan:?}"
    );
    eprintln!("SQL60k tag_prefix_plan={plan:?}");
}

/// Measure the service including the merge of registered and inferred keys.
fn measure_catalogue(tx: &rusqlite::Transaction<'_>) {
    tx.execute_batch(
        "
        INSERT INTO item(id,header_json,body_id,title_sort,created_ms,modified_ms)
        SELECT e.id,json_object('id',e.uuid,'revision',printf('declaration-%d',e.id),
            'meta',json_object('type','tag','state','active','title',e.named_key)),
            1,e.named_key,0,0
        FROM entity e WHERE e.named_kind='tag' AND e.id%2=0;
    ",
    )
    .unwrap();
    let mut samples = Vec::new();
    for iteration in 0..110 {
        let start = Instant::now();
        let result = crate::store::reads::catalogue::list_facets(
            tx,
            Some(crate::domain::project::types::FacetKind::Tag),
        )
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(result.len(), 4500);
        assert_eq!(result.iter().filter(|entry| entry.inferred).count(), 2250);
        if iteration >= 10 {
            samples.push(elapsed);
        }
    }
    samples.sort();
    eprintln!(
        "SQL60k list_facets(tag) p95_ms={:.2}",
        samples[94].as_secs_f64() * 1000.0
    );
}

fn populate_fixture(tx: &rusqlite::Transaction<'_>) {
    tx.execute_batch("CREATE TEMP TABLE fixture_id(n INTEGER PRIMARY KEY); WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<60000) INSERT INTO fixture_id SELECT n FROM ids;
        INSERT INTO entity(id,uuid) SELECT n,printf('01a0e2a0-0000-7000-8000-%012d',n) FROM fixture_id;
        INSERT INTO body(id,content_json) VALUES(1,'{}');
        INSERT INTO item(id,header_json,body_id,title_sort,created_ms,modified_ms)
        SELECT n,json_object('id',printf('01a0e2a0-0000-7000-8000-%012d',n),'revision',printf('r%d',n),'createdAt','2026-01-01T00:00:00.000Z','modifiedAt','2026-01-01T00:00:00.000Z','meta',json_object('type','page','state','active','title',printf('Page %d',n),'priority',n%100)),1,printf('page %d',n),1767225600000,1767225600000 FROM fixture_id;
        INSERT INTO item_projection(item_id,preview,analyzed_text_hash,word_count) SELECT n,'A representative short result preview.','hash',100 FROM fixture_id;
        INSERT INTO field_definition(id,key,normalization_version) VALUES(1,'priority',1);
        INSERT INTO field_presence(item_id,field_id,value_state,observed_type) SELECT n,1,'value','number' FROM fixture_id;
        INSERT INTO field_value(item_id,field_id,number_value) SELECT n,1,n%100 FROM fixture_id;
        INSERT INTO entity(id,uuid,named_kind,named_key) SELECT 100000+n,printf('tag-%d',n),'tag',printf('topic/%d',n) FROM fixture_id WHERE n<=4500;
        INSERT INTO relation(kind,source_id,target_id) SELECT 'tag',n,100001+n%4500 FROM fixture_id;
        INSERT INTO relation(kind,source_id,target_id) SELECT 'tag',n,100001+(n+250)%4500 FROM fixture_id;
        ").unwrap();
    tx.execute(
        "INSERT INTO item_fts(rowid,title,body) SELECT 2*n+1,'',?1 FROM fixture_id",
        [crate::store::normalize::fts_text(
            "common searchable words with a long paragraph of useful text",
        )],
    )
    .unwrap();
}
