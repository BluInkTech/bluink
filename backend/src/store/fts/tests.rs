// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;

#[test]
fn title_changes_keep_body_postings_and_removal_clears_both() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch(crate::store::SCHEMA).unwrap();
    let tx = connection.transaction().unwrap();
    update_title(&tx, 1, "old heading").unwrap();
    update_body(&tx, 1, "unchanged article words").unwrap();
    let before: Vec<(String, i64)> = tx
        .prepare("SELECT term,offset FROM item_fts_terms WHERE doc=3 ORDER BY offset")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    update_title(&tx, 1, "new heading").unwrap();
    let after: Vec<(String, i64)> = tx
        .prepare("SELECT term,offset FROM item_fts_terms WHERE doc=3 ORDER BY offset")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        tx.query_row(
            "SELECT count(*) FROM item_fts WHERE item_fts MATCH ?1",
            [normalize::fts_token("old")],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    remove(&tx, 1).unwrap();
    assert_eq!(
        tx.query_row("SELECT count(*) FROM item_fts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// FTS-only control for the full immutable-body autosave gate. Run explicitly;
/// prints durable WAL bytes separately from normalized input and wall time.
#[test]
#[ignore = "Disk-backed 1 MiB sparse-edit FTS prototype; run explicitly for the storage gate."]
fn compare_page_and_block_fts_write_volume() {
    use std::time::Instant;
    const EDITS: usize = 100;
    let paragraph = "The writer follows a winding road through the quiet forest and records the changing landscape. ".repeat(11);
    for block_rows in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("probe.sqlite3");
        let mut connection = rusqlite::Connection::open(&database).unwrap();
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0; CREATE VIRTUAL TABLE probe USING fts5(body,content='',contentless_delete=1);").unwrap();
        let mut blocks = vec![paragraph.clone(); 1024];
        let tx = connection.transaction().unwrap();
        if block_rows {
            for (position, block) in blocks.iter().enumerate() {
                tx.execute(
                    "INSERT INTO probe(rowid,body) VALUES(?1,?2)",
                    params![
                        i64::try_from(position + 1).unwrap(),
                        normalize::fts_text(block)
                    ],
                )
                .unwrap();
            }
        } else {
            tx.execute(
                "INSERT INTO probe(rowid,body) VALUES(1,?1)",
                [normalize::fts_text(&blocks.join("\n"))],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let mut durations = Vec::new();
        let mut volumes = Vec::new();
        for edit in 0..EDITS {
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let start = Instant::now();
            blocks[edit].push('x');
            let tx = connection.transaction().unwrap();
            let (row, text) = if block_rows {
                (
                    i64::try_from(edit + 1).unwrap(),
                    normalize::fts_text(&blocks[edit]),
                )
            } else {
                (1, normalize::fts_text(&blocks.join("\n")))
            };
            tx.execute("DELETE FROM probe WHERE rowid=?1", [row])
                .unwrap();
            tx.execute(
                "INSERT INTO probe(rowid,body) VALUES(?1,?2)",
                params![row, text],
            )
            .unwrap();
            tx.commit().unwrap();
            durations.push(start.elapsed());
            volumes.push(
                std::fs::metadata(database.with_extension("sqlite3-wal"))
                    .unwrap()
                    .len(),
            );
        }
        durations.sort();
        volumes.sort_unstable();
        eprintln!(
            "FTS block_rows={block_rows} body_bytes={} p95_ms={:.2} p95_wal_bytes={} max_wal_bytes={}",
            paragraph.len() * 1024,
            durations[94].as_secs_f64() * 1000.0,
            volumes[94],
            volumes[99]
        );
    }
}
