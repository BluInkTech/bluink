// SPDX-License-Identifier: MPL-2.0
use super::*;
use crate::store::{Store, maintenance};
use uuid::Uuid;

fn first(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE migrated_one(value TEXT); INSERT INTO migrated_one VALUES ('preserved');",
    )?;
    Ok(())
}
fn second(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ALTER TABLE migrated_one ADD COLUMN ordering INTEGER;")?;
    Ok(())
}
fn fail(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("INSERT INTO missing_table VALUES (1)")?;
    Ok(())
}
fn fixture() -> (tempfile::TempDir, Store, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    checkpoints::initialize(root.path()).unwrap();
    private_state(&store);
    let output = tempfile::tempdir().unwrap();
    (root, store, output)
}

#[test]
fn startup_validates_the_registry_before_writes_and_resumes_a_verified_snapshot() {
    let (root, old_store, output) = fixture();
    let path = old_store.path().to_owned();
    let missing = [Step {
        from: 2,
        to: 3,
        apply: second,
    }];
    assert!(upgrade(root.path(), &path, 3, &missing).is_err());
    assert!(!root.path().join(".backups").exists());
    let steps = [
        Step {
            from: 1,
            to: 2,
            apply: first,
        },
        Step {
            from: 2,
            to: 3,
            apply: second,
        },
    ];
    let snapshot = output.path().join("protected.sqlite3");
    prepare(root.path(), &snapshot, &steps, 3).unwrap();
    upgrade(root.path(), &path, 3, &steps).unwrap();
    assert!(!root.path().join(".backups").exists());
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_private_state(&db);
    // A writer opened by the previous binary must reject the upgraded schema.
    assert!(
        old_store
            .write(|tx| {
                tx.execute("DELETE FROM activity_event", [])?;
                Ok(())
            })
            .is_err()
    );
    assert_private_state(&db);
}

fn private_state(store: &Store) {
    use crate::store::{
        item::{ExpectedRevision, PreparedItem},
        receipts::{self, Submission},
    };
    use serde_json::json;
    let first=crate::store::commands::assemble_item(&json!({"meta":{"type":"page","title":"Retained base"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"}}]}}}),None,"UTC").unwrap();
    let next = crate::store::commands::assemble_item(
        &json!({"id":first["id"],"meta":{"type":"page","title":"Uncheckpointed save"}}),
        Some(&first),
        "UTC",
    )
    .unwrap();
    let before = PreparedItem::new(&first).unwrap();
    let after = PreparedItem::new(&next).unwrap();
    store
        .write(move |tx| {
            let id = before.install(tx, ExpectedRevision::New)?;
            let base = first["revision"].as_str().unwrap();
            let accepted = next["revision"].as_str().unwrap();
            after.install(tx, ExpectedRevision::Exact(base))?;
            receipts::complete(
                tx,
                &Submission {
                    item: id,
                    device: "offline",
                    id: "pending-ack",
                    hash: "identity",
                    base,
                },
                accepted,
                &json!({"accepted":accepted}),
            )?;
            tx.execute(
                "INSERT INTO activity_event VALUES (?1,?2,'content',?1,1000,'2026-11-01',3,0)",
                rusqlite::params![id, accepted],
            )?;
            Ok(())
        })
        .unwrap();
}

fn assert_private_state(connection: &Connection) {
    for table in ["item", "revision", "submission", "activity_event"] {
        assert_eq!(
            connection
                .query_row::<i64, _, _>(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get(0))
                .unwrap(),
            1
        );
    }
    let title: String = connection
        .query_row(
            "SELECT json_extract(header_json,'$.meta.title') FROM item",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(title, "Uncheckpointed save");
}

#[test]
fn ordered_upgrade_commits_all_steps_and_preserves_other_pins() {
    let (root, store, output) = fixture();
    let other = maintenance::protect(&store, "backup", &output.path().join("other")).unwrap();
    let prepared = prepare(root.path(), &output.path().join("before.sqlite3"), &[], 1).unwrap();
    let steps = [
        Step {
            from: 1,
            to: 2,
            apply: first,
        },
        Step {
            from: 2,
            to: 3,
            apply: second,
        },
    ];
    drop(store);
    apply(root.path(), &prepared, &steps, 3).unwrap();
    let connection = Connection::open(root.path().join(super::super::DATABASE_NAME)).unwrap();
    assert_eq!(
        connection
            .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row::<String, _, _>("SELECT value FROM migrated_one", [], |row| row.get(0))
            .unwrap(),
        "preserved"
    );
    assert!(
        connection
            .query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id=?1)",
                [&other.id],
                |row| row.get(0)
            )
            .unwrap()
    );
    assert!(
        !connection
            .query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id=?1)",
                [prepared.snapshot.maintenance_pin],
                |row| row.get(0)
            )
            .unwrap()
    );
    assert_private_state(&connection);
    drop(connection);
    let before = std::fs::read(root.path().join(super::super::DATABASE_NAME)).unwrap();
    assert!(Store::open(root.path()).is_err());
    assert_eq!(
        std::fs::read(root.path().join(super::super::DATABASE_NAME)).unwrap(),
        before
    );
}

#[test]
fn failed_second_step_rolls_back_schema_and_keeps_snapshot_and_cleanup_protection() {
    let (root, store, output) = fixture();
    let prepared = prepare(root.path(), &output.path().join("before.sqlite3"), &[], 1).unwrap();
    drop(store);
    let steps = [
        Step {
            from: 1,
            to: 2,
            apply: first,
        },
        Step {
            from: 2,
            to: 3,
            apply: fail,
        },
    ];
    assert!(apply(root.path(), &prepared, &steps, 3).is_err());
    let store = Store::open(root.path()).unwrap();
    store
        .read(|tx| {
            assert_eq!(
                tx.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))?,
                1
            );
            assert!(!tx.query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='migrated_one')",
                [],
                |row| row.get(0)
            )?);
            assert!(maintenance::protected(tx)?);
            assert_private_state(tx);
            Ok(())
        })
        .unwrap();
    assert_eq!(maintenance::prune(&store, i64::MAX).unwrap().bodies, 0);
    assert!(prepared.path.exists());
}

#[test]
fn migration_rejects_missing_reverse_and_out_of_order_steps_before_applying() {
    for steps in [
        vec![Step {
            from: 2,
            to: 3,
            apply: first,
        }],
        vec![Step {
            from: 1,
            to: 3,
            apply: first,
        }],
        vec![Step {
            from: 1,
            to: 0,
            apply: first,
        }],
    ] {
        assert!(validate_steps(1, 3, &steps).is_err());
    }
    assert!(validate_steps(2, 1, &[]).is_err());
    assert!(validate_steps(0, 0, &[]).is_err());
    assert!(validate_steps(1, 2, &[]).is_err());
}

#[test]
fn a_save_after_snapshot_refuses_upgrade_without_changing_schema() {
    let (root, store, output) = fixture();
    let prepared = prepare(root.path(), &output.path().join("before.sqlite3"), &[], 1).unwrap();
    store
        .write(|tx| {
            tx.execute("UPDATE project_state SET generation=generation+1", [])?;
            Ok(())
        })
        .unwrap();
    let steps = [Step {
        from: 1,
        to: 2,
        apply: first,
    }];
    drop(store);
    assert!(
        apply(root.path(), &prepared, &steps, 2)
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert!(
        Store::open(root.path())
            .unwrap()
            .read(maintenance::protected)
            .unwrap()
    );
}
