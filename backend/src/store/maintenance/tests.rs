// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::store::{
    body::PreparedBody,
    item::{ExpectedRevision, PreparedItem},
    receipts::{self, Submission},
};
use serde_json::{Value, json};
use uuid::Uuid;

fn content(text: &str) -> Value {
    json!({"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"block"},"content":[{"type":"text","text":text}]}]}})
}

fn install(store: &Store, text: &str) -> i64 {
    let prepared = PreparedBody::new(&content(text)).unwrap();
    store
        .write(move |transaction| prepared.install(transaction))
        .unwrap()
}

fn delete_body(store: &Store, id: i64) {
    store
        .write(move |transaction| {
            transaction.execute("DELETE FROM body WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
}

fn block_id(store: &Store, body: i64) -> i64 {
    store
        .read(|transaction| {
            let bytes: Vec<u8> =
                transaction.query_row("SELECT block_ids FROM body WHERE id=?1", [body], |row| {
                    row.get(0)
                })?;
            Ok(body::decode(&bytes)?[0])
        })
        .unwrap()
}

fn finish(store: &Store, mut sweep: BlockSweep) {
    for _ in 0..200 {
        let (next, progress) = sweep.step(store).unwrap();
        sweep = next;
        if progress == SweepStep::Complete {
            return;
        }
    }
    panic!("Bounded sweep did not finish");
}

#[test]
fn reused_orphan_is_marked_under_writer_and_deleted_ids_never_reappear() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let other = Store::open(root.path()).unwrap();
    let orphan = install(&store, "Orphan");
    let old_block = block_id(&store, orphan);
    delete_body(&store, orphan);
    let surviving = install(&store, "Surviving");
    let (sweep, progress) = BlockSweep::begin(&store).unwrap().step(&store).unwrap();
    assert!(matches!(progress, SweepStep::Marked { .. }));
    // A second connection/process can reuse the old digest after the initial scan.
    let rescued = install(&other, "Orphan");
    assert_eq!(block_id(&other, rescued), old_block);
    finish(&store, sweep);
    assert_eq!(
        store
            .read(|transaction| body::read(transaction, rescued))
            .unwrap(),
        content("Orphan")
    );
    delete_body(&other, rescued);
    finish(&store, BlockSweep::begin(&store).unwrap());
    assert_eq!(
        store
            .read(|transaction| body::read(transaction, surviving))
            .unwrap(),
        content("Surviving")
    );
    let replaced = install(&other, "Orphan");
    assert!(replaced > rescued);
    assert!(block_id(&other, replaced) > old_block);
}

#[test]
fn large_writer_tail_yields_without_deleting_then_catches_up() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let orphan = install(&store, "Unused");
    delete_body(&store, orphan);
    let (sweep, _) = BlockSweep::begin(&store).unwrap().step(&store).unwrap();
    let prepared = (0..=WRITER_ROWS)
        .map(|i| PreparedBody::new(&content(&format!("Tail {i}"))).unwrap())
        .collect::<Vec<_>>();
    let bodies = store
        .write(move |transaction| {
            prepared
                .iter()
                .map(|body| body.install(transaction))
                .collect::<Result<Vec<_>>>()
        })
        .unwrap();
    let (sweep, progress) = sweep.step(&store).unwrap();
    assert_eq!(progress, SweepStep::Yielded);
    let count: i64 = store
        .read(|transaction| {
            Ok(transaction.query_row("SELECT count(*) FROM block", [], |row| row.get(0))?)
        })
        .unwrap();
    assert_eq!(count, i64::try_from(WRITER_ROWS + 2).unwrap());
    finish(&store, sweep);
    for id in bodies {
        store
            .read(|transaction| body::read(transaction, id))
            .unwrap();
    }
}

#[test]
fn deleted_manifests_and_new_saves_between_read_batches_preserve_every_live_block() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let other = Store::open(root.path()).unwrap();
    let prepared = (0..READ_ROWS + 2)
        .map(|i| PreparedBody::new(&content(&format!("Body {i}"))).unwrap())
        .collect::<Vec<_>>();
    let bodies = store
        .write(move |transaction| {
            prepared
                .iter()
                .map(|body| body.install(transaction))
                .collect::<Result<Vec<_>>>()
        })
        .unwrap();
    let (mut sweep, _) = BlockSweep::begin(&store).unwrap().step(&store).unwrap();
    delete_body(&other, *bodies.last().unwrap());
    let mut saves = Vec::new();
    for i in 0..8 {
        saves.push(install(&other, &format!("Concurrent {i}")));
        let (next, _) = sweep.step(&store).unwrap();
        sweep = next;
    }
    finish(&store, sweep);
    for id in bodies.into_iter().take(READ_ROWS + 1).chain(saves) {
        store
            .read(|transaction| body::read(transaction, id))
            .unwrap();
    }
}

#[test]
fn interrupted_sweep_is_discarded_and_a_fresh_mark_includes_later_manifests() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let old = install(&store, "Old");
    let (sweep, _) = BlockSweep::begin(&store).unwrap().step(&store).unwrap();
    let temporary = std::path::Path::new(sweep.marks.path().unwrap())
        .parent()
        .unwrap()
        .to_owned();
    drop(sweep);
    assert!(!temporary.exists());
    delete_body(&store, old);
    drop(store);
    let store = Store::open(root.path()).unwrap();
    let latest = install(&store, "Latest");
    finish(&store, BlockSweep::begin(&store).unwrap());
    assert_eq!(
        store
            .read(|transaction| body::read(transaction, latest))
            .unwrap(),
        content("Latest")
    );
}

#[test]
fn expired_base_and_receipt_prune_together_but_a_live_pin_preserves_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let now = 3 * DRAFT_WINDOW_MS;
    let first = crate::store::commands::assemble_item(
        &json!({"meta":{"type":"page","title":"First"},"content":content("Text")}),
        None,
        "UTC",
    )
    .unwrap();
    let next = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"type":"page","title":"Next"},"content":content("Next")}),
        Some(&first),
        "UTC",
    )
    .unwrap();
    // Distinct content gives each retained revision its own body to reclaim.
    let current = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"type":"page","title":"Current"},"content":content("Current")}),
        Some(&next),
        "UTC",
    )
    .unwrap();
    let r1 = first["revision"].as_str().unwrap().to_owned();
    let r2 = next["revision"].as_str().unwrap().to_owned();
    let p1 = PreparedItem::new(&first).unwrap();
    let p2 = PreparedItem::new(&next).unwrap();
    let p3 = PreparedItem::new(&current).unwrap();
    let (id, r1, r2) = store
        .write(move |transaction| {
            let id = p1.install(transaction, ExpectedRevision::New)?;
            p2.install(transaction, ExpectedRevision::Exact(&r1))?;
            p3.install(transaction, ExpectedRevision::Exact(&r2))?;
            transaction.execute("UPDATE revision SET superseded_ms=0", [])?;
            receipts::complete(
                transaction,
                &Submission {
                    item: id,
                    device: "device",
                    id: "receipt",
                    hash: "hash",
                    base: &r1,
                },
                &r2,
                &json!({"accepted":r2}),
            )?;
            receipts::pin(transaction, "editor", "open", id, &r1, Some(now + 10))?;
            Ok((id, r1, r2))
        })
        .unwrap();
    let counts = prune(&store, now).unwrap();
    assert_eq!(counts.receipts, 0);
    store
        .read(|transaction| {
            assert!(base_is_admissible(transaction, id, &r1, now)?);
            assert!(!base_is_admissible(transaction, id, &r2, now)?);
            assert!(
                receipts::replay(
                    transaction,
                    &Submission {
                        item: id,
                        device: "device",
                        id: "receipt",
                        hash: "hash",
                        base: &r1
                    }
                )?
                .is_some()
            );
            Ok(())
        })
        .unwrap();
    let counts = prune(&store, now + 11).unwrap();
    assert_eq!(counts.pins, 1);
    assert_eq!(counts.receipts, 1);
    assert_eq!(counts.revisions, 2);
    assert_eq!(counts.bodies, 2);
    assert!(
        !store
            .read(|transaction| base_is_admissible(transaction, id, &r1, now + 11))
            .unwrap()
    );
}

#[test]
fn backup_pin_survives_restart_and_blocks_every_destructive_maintenance_phase() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let orphan = install(&store, "Pinned backup data");
    let pin = protect(&store, "backup", &root.path().join("complete-backup")).unwrap();
    assert_eq!(prune(&store, 3 * DRAFT_WINDOW_MS).unwrap().bodies, 0);
    delete_body(&store, orphan);
    let (sweep, _) = BlockSweep::begin(&store).unwrap().step(&store).unwrap();
    let (sweep, step) = sweep.step(&store).unwrap();
    assert_eq!(step, SweepStep::Yielded);
    drop(sweep);
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert!(store.read(protected).unwrap());
    release(&store, pin).unwrap();
    finish(&store, BlockSweep::begin(&store).unwrap());
    let remaining: i64 = store
        .read(|transaction| {
            Ok(transaction.query_row("SELECT count(*) FROM block", [], |row| row.get(0))?)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn orphan_selected_before_a_backup_pin_is_rechecked_under_writer() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let body = install(&store, "Selected for cleanup");
    let rows = candidates(&store, 3 * DRAFT_WINDOW_MS, &ORPHAN_BODIES).unwrap();
    assert_eq!(rows.len(), 1);
    let pin = protect(&store, "backup", &root.path().join("in-progress")).unwrap();
    assert_eq!(
        store
            .write(move |transaction| delete_candidates(
                transaction,
                3 * DRAFT_WINDOW_MS,
                &ORPHAN_BODIES,
                rows
            ))
            .unwrap(),
        0
    );
    release(&store, pin).unwrap();
    let exists: bool = store
        .read(|transaction| {
            Ok(transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM body WHERE id=?1)",
                [body],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(exists);
}

#[test]
fn editor_lease_moves_without_readmitting_an_expired_base() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::create(directory.path(), Uuid::now_v7()).unwrap();
    let first = crate::store::commands::assemble_item(
        &json!({"meta":{"type":"page"},"content":content("Original")}),
        None,
        "UTC",
    )
    .unwrap();
    let next = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"title":"Updated"}}),
        Some(&first),
        "UTC",
    )
    .unwrap();
    store.write(move|tx| {
        let id=first["id"].as_str().unwrap(); let old=first["revision"].as_str().unwrap(); let new=next["revision"].as_str().unwrap();
        let now=3*DRAFT_WINDOW_MS;
        PreparedItem::new(&first)?.install(tx,ExpectedRevision::New)?;
        assert!(renew_editor_base(tx,"editor",id,old,now)?);
        PreparedItem::new(&next)?.install(tx,ExpectedRevision::Exact(old))?;
        tx.execute("UPDATE revision SET superseded_ms=0",[])?;
        assert!(renew_editor_base(tx,"editor",id,old,now+1)?);
        assert!(renew_editor_base(tx,"editor",id,new,now+2)?);
        assert!(!renew_editor_base(tx,"editor",id,old,now+3)?);
        let pinned:String=tx.query_row("SELECT revision_uuid FROM revision_pin WHERE owner_kind='editor' AND owner_id='editor'",[],|row|row.get(0))?;
        assert_eq!(pinned,new);
        Ok(())
    }).unwrap();
}

#[test]
fn a_pin_left_by_a_stopped_copy_is_reclaimed_and_a_running_one_is_kept() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let running = protect(&store, "backup", &root.path().join("running")).unwrap();
    let Pin { lock, .. } = protect(&store, "export", &root.path().join("crashed")).unwrap();
    // The process that held this lock has gone; its row stays behind.
    drop(lock);
    reclaim(&store).unwrap();
    let pins: Vec<String> = store
        .read(|transaction| {
            let mut statement = transaction.prepare("SELECT id FROM maintenance_pin")?;
            let ids = statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(pins, std::slice::from_ref(&running.id));
    release(&store, running).unwrap();
    assert!(!store.read(protected).unwrap());
    assert_eq!(
        std::fs::read_dir(root.path().join(".maintenance"))
            .unwrap()
            .count(),
        0
    );
}
