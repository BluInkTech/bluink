// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::structured_query::fixtures::criteria;

use super::*;
use rusqlite::Connection;
use std::time::{Duration, Instant};

fn occupied(db: &Connection) -> i64 {
    db.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count,pragma_freelist_count,pragma_page_size",[],|row|row.get(0)).unwrap()
}

fn percentile(samples: &mut [Duration]) -> f64 {
    samples.sort();
    samples[(samples.len() * 95).div_ceil(100) - 1].as_secs_f64() * 1000.0
}

#[test]
#[ignore = "240 durable 1 MiB sparse editor saves and retained-body controls on the reference workstation"]
fn sparse_edit_hour() {
    let prose="A writer follows a winding road through the forest and records the changes in the landscape. ".repeat(10);
    let nodes=(0..1024).map(|number|json!({"type":"paragraph","content":[{"type":"text","text":format!("{number} {prose}")}]})).collect();
    let (root, store, mut page) = start(nodes);
    let db = Connection::open(root.path().join(crate::store::DATABASE_NAME)).unwrap();
    let original = page["content"]["document"]["content"].as_array().unwrap();
    let order = original
        .iter()
        .map(|node| node["attrs"]["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let (whole, hashes, mut manifest) = controls(root.path(), &page);
    let initial_body = page["content"].to_string();
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    let initial = (occupied(&db), occupied(&whole), occupied(&hashes));
    let mut saves = Vec::new();
    let mut opens = Vec::new();
    let mut volume = Vec::new();
    let mut peak = 0;
    let path = root.path().join(crate::store::DATABASE_NAME);
    for edit in 0..240 {
        let index = edit * 3;
        let block = &mut page["content"]["document"]["content"][index];
        block["content"][0]["text"] = json!(format!("{prose} edit{edit}"));
        manifest[index] = digest(block);
        hashes
            .execute(
                "INSERT INTO blocks VALUES(?1,?2)",
                rusqlite::params![manifest[index], block.to_string()],
            )
            .unwrap();
        hashes
            .execute(
                "INSERT INTO manifests VALUES(?1)",
                [serde_json::to_string(&manifest).unwrap()],
            )
            .unwrap();
        let changed = BTreeMap::from([(order[index].clone(), block.clone())]);
        whole
            .execute(
                "INSERT INTO bodies VALUES(?1)",
                [page["content"].to_string()],
            )
            .unwrap();
        let request = patch(&page, &order, &changed);
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let started = Instant::now();
        let ack = apply(&store, &request);
        saves.push(started.elapsed());
        page["revision"] = ack["version"].clone();
        volume.push(
            std::fs::metadata(root.path().join("project.sqlite3-wal"))
                .unwrap()
                .len(),
        );
        peak = peak.max(std::fs::metadata(&path).unwrap().len() + volume.last().copied().unwrap());
        if edit == 120 {
            wal_owners(&db, root.path());
        }
        let opened = Instant::now();
        let exact = store
            .read(|tx| item::read(tx, &request.id, true))
            .unwrap()
            .unwrap();
        opens.push(opened.elapsed());
        assert_eq!(exact["content"], page["content"]);
    }
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    volume.sort_unstable();
    let growth = (
        occupied(&db) - initial.0,
        occupied(&whole) - initial.1,
        occupied(&hashes) - initial.2,
    );
    eprintln!(
        "sparse_edit_hour peak_db_and_wal={peak} checkpointed_db={} bytes={} saves=240 ack_p95_ms={:.2} open_p95_ms={:.2} wal_p95={} wal_max={} occupied_growth={} whole_growth={} hash_manifest_growth={} wal_total={}",
        std::fs::metadata(&path).unwrap().len(),
        initial_body.len(),
        percentile(&mut saves),
        percentile(&mut opens),
        volume[227],
        volume[239],
        growth.0,
        growth.1,
        growth.2,
        volume.iter().sum::<u64>()
    );
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM revision", [], |row| row.get(0))
            .unwrap(),
        240
    );
    assert!(growth.0 < 32 * 1024 * 1024 && growth.0 * 5 < growth.1);
}

fn wal_owners(db: &Connection, root: &std::path::Path) {
    let pages = db
        .prepare("SELECT pageno,name FROM dbstat")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .unwrap();
    let bytes = std::fs::read(root.join("project.sqlite3-wal")).unwrap();
    let mut counts = BTreeMap::<String, usize>::new();
    for frame in bytes[32..].as_chunks::<{ 4096 + 24 }>().0 {
        let number = u32::from_be_bytes(frame[..4].try_into().unwrap());
        *counts
            .entry(
                pages
                    .get(&number)
                    .cloned()
                    .unwrap_or_else(|| "freelist".into()),
            )
            .or_default() += 1;
    }
    eprintln!("sparse save WAL frames by page owner: {counts:?}");
}

fn controls(root: &std::path::Path, page: &Value) -> (Connection, Connection, Vec<String>) {
    let whole = Connection::open(root.join("whole-control.db")).unwrap();
    whole
        .execute("CREATE TABLE bodies(payload TEXT NOT NULL)", [])
        .unwrap();
    let hashes = Connection::open(root.join("hash-control.db")).unwrap();
    hashes.execute_batch("CREATE TABLE blocks(digest TEXT PRIMARY KEY,payload TEXT NOT NULL); CREATE TABLE manifests(payload TEXT NOT NULL);").unwrap();
    let original = page["content"]["document"]["content"].as_array().unwrap();
    let manifest = original.iter().map(digest).collect::<Vec<_>>();
    for (digest, block) in manifest.iter().zip(original) {
        hashes
            .execute(
                "INSERT INTO blocks VALUES(?1,?2)",
                rusqlite::params![digest, block.to_string()],
            )
            .unwrap();
    }
    hashes
        .execute(
            "INSERT INTO manifests VALUES(?1)",
            [serde_json::to_string(&manifest).unwrap()],
        )
        .unwrap();
    let initial_body = page["content"].to_string();
    whole
        .execute("INSERT INTO bodies VALUES(?1)", [&initial_body])
        .unwrap();
    (whole, hashes, manifest)
}
fn digest(block: &Value) -> String {
    use sha2::Digest;
    crate::versioning::checkpoints::digest_hex(&sha2::Sha256::digest(
        serde_json::to_vec(block).unwrap(),
    ))
}

#[test]
#[ignore = "Durable metadata and title saves against four concurrent readers of a 1 MiB page"]
fn header_writes_with_four_readers() {
    let paragraph =
        "Read and write concurrently while the author edits a single field. ".repeat(16);
    let (_root, store, page) = start(
        (0..1024)
            .map(|_| json!({"type":"paragraph","content":[{"type":"text","text":paragraph}]}))
            .collect(),
    );
    let id = page["id"].as_str().unwrap();
    let barrier = std::sync::Barrier::new(5);
    let mut properties = Vec::new();
    let mut titles = Vec::new();
    let mut reads = std::thread::scope(|scope| {
        let readers = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let mut samples = Vec::new();
                    for _ in 0..1000 {
                        let start = Instant::now();
                        assert!(
                            store
                                .read(|tx| item::read(tx, id, false))
                                .unwrap()
                                .is_some()
                        );
                        samples.push(start.elapsed());
                    }
                    samples
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let mut revision = page["revision"].as_str().unwrap().to_owned();
        for iteration in 0..110 {
            for (key, value, samples) in [
                ("bench_rating", json!(iteration), &mut properties),
                ("title", json!(format!("Title {iteration}")), &mut titles),
            ] {
                let start = Instant::now();
                let result = crate::store::commands::metadata(
                    &store,
                    id,
                    Some(&revision),
                    serde_json::Map::from_iter([(key.into(), value)]),
                    false,
                )
                .unwrap();
                if iteration >= 10 {
                    samples.push(start.elapsed());
                }
                revision = result["revision"].as_str().unwrap().to_owned();
            }
        }
        readers
            .into_iter()
            .flat_map(|reader| reader.join().unwrap())
            .collect::<Vec<_>>()
    });
    eprintln!(
        "header_saves four_readers property_p95_ms={:.2} rename_p95_ms={:.2} header_read_p95_ms={:.2}",
        percentile(&mut properties),
        percentile(&mut titles),
        percentile(&mut reads)
    );
    store
        .read(|tx| {
            assert_eq!(
                tx.query_row::<i64, _, _>("SELECT count(*) FROM body", [], |row| row.get(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
#[ignore = "Explicit write volume and latency controls for large editor operations"]
fn non_sparse_document_edits() {
    let prose = "A writer records the changing landscape. ".repeat(24);
    let paragraph =
        |text: &str| json!({"type":"paragraph","content":[{"type":"text","text":text}]});
    let ordinary = || {
        (0..1024)
            .map(|n| paragraph(&format!("originaltoken {n} {prose}")))
            .collect::<Vec<_>>()
    };
    for case in [
        "oversized paragraph",
        "nested table",
        "reorder",
        "whole-page paste",
    ] {
        let nodes = match case {
            "oversized paragraph" => {
                vec![paragraph(&format!("originaltoken {}", prose.repeat(1024)))]
            }
            "nested table" => vec![
                json!({"type":"table","attrs":{"columnWidthsPercent":[100]},"content":(0..1024).map(|n|json!({
                "type":"table_row","content":[{"type":"table_cell","content":[paragraph(&format!("originaltoken {n} {prose}"))]}]
            })).collect::<Vec<_>>()}),
            ],
            _ => ordinary(),
        };
        let (root, store, base) = start(nodes);
        let mut intended = base["content"]["document"]["content"]
            .as_array()
            .unwrap()
            .clone();
        let mut changed = BTreeMap::new();
        match case {
            "oversized paragraph" => {
                intended[0]["content"][0]["text"] =
                    json!(format!("newmarker {}", prose.repeat(1024)));
            }
            "nested table" => {
                intended[0]["content"][504]["content"][0]["content"][0]["content"][0]["text"] =
                    json!(format!("newmarker {prose}"));
            }
            "reorder" => intended.reverse(),
            _ => {
                for node in &mut intended {
                    node["attrs"]["id"] = json!(Uuid::now_v7().to_string());
                    node["content"][0]["text"] = json!(format!("newmarker {prose}"));
                }
            }
        }
        let order = intended
            .iter()
            .map(|node| node["attrs"]["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        if case != "reorder" {
            changed.extend(order.iter().cloned().zip(intended.iter().cloned()));
        }
        let request = patch(&base, &order, &changed);
        let db = Connection::open(root.path().join(crate::store::DATABASE_NAME)).unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let before = occupied(&db);
        let started = Instant::now();
        let ack = apply(&store, &request);
        let elapsed = started.elapsed();
        let wal = std::fs::metadata(root.path().join("project.sqlite3-wal"))
            .unwrap()
            .len();
        store
            .read(|tx| {
                let result = item::read(tx, &request.id, true)?.unwrap();
                assert_eq!(result["revision"], ack["version"]);
                assert_eq!(
                    result["content"]["document"]["content"],
                    Value::Array(intended.clone())
                );
                assert_eq!(
                    item::read_revision(tx, &request.id, &request.base_revision)?.unwrap(),
                    base
                );
                for (term, present) in [
                    ("newmarker", case != "reorder"),
                    ("originaltoken", matches!(case, "reorder" | "nested table")),
                ] {
                    let ids = query::matching_ids(
                        tx,
                        &criteria([crate::structured_query::fixtures::term(term)]),
                        None,
                        None,
                        chrono_tz::UTC,
                    )?;
                    assert_eq!(ids.contains(&request.id), present, "{case}: {term}");
                }
                Ok(())
            })
            .unwrap();
        eprintln!(
            "edit-control {case}: save_ms={:.2} wal_bytes={wal} retained_bytes={}",
            elapsed.as_secs_f64() * 1000.0,
            occupied(&db) - before
        );
    }
}
