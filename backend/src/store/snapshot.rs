// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! WAL-consistent database snapshots. This alone is not a full project backup:
//! the caller must independently copy the protected Git and LFS object stores.

use super::{Error, Result, SCHEMA_VERSION, Store, body};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub(crate) project_id: String,
    pub(crate) generation: i64,
    pub(crate) schema_version: i64,
    pub(crate) maintenance_pin: String,
}

/// The durable pin precedes snapshotting and remains owned by the caller on error.
pub(crate) fn create(store: &Store, pin: &str, destination: &Path) -> Result<Snapshot> {
    create_database(store.path(), pin, destination, SCHEMA_VERSION)
}

pub(super) fn create_database(
    path: &Path,
    pin: &str,
    destination: &Path,
    schema: i64,
) -> Result<Snapshot> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(5))?;
    let protected = source.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id=?1)",
        [pin],
        |row| row.get::<_, bool>(0),
    )?;
    if !protected {
        return Err(Error::Invalid(
            "A database snapshot requires a durable maintenance pin.".to_owned(),
        ));
    }
    // Reserve only a new destination; never replace another backup or live database.
    let reserved = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    drop(reserved);
    let mut output = Connection::open_with_flags(destination, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    {
        let backup = Backup::new(&source, &mut output)?;
        let deadline = Instant::now() + Duration::from_mins(30);
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Busy);
            }
            // Copy in one step under one WAL read snapshot. Incremental steps
            // restart whenever the live writer commits between them.
            match backup.step(-1)? {
                StepResult::Done => break,
                StepResult::More | StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    return Err(Error::Invalid(
                        "Unsupported SQLite backup progress.".to_owned(),
                    ));
                }
            }
        }
    }
    // A completed snapshot is one closed file; no committed WAL is left behind.
    output.pragma_update(None, "journal_mode", "DELETE")?;
    let snapshot = verify_schema(&output, pin, schema)?;
    drop(output);
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)?
        .sync_all()?;
    Ok(snapshot)
}

/// Verify private records as well as SQL pages and foreign keys.
pub(crate) fn verify(connection: &Connection, pin: &str) -> Result<Snapshot> {
    verify_schema(connection, pin, SCHEMA_VERSION)
}

pub(super) fn verify_schema(
    connection: &Connection,
    pin: &str,
    expected_schema: i64,
) -> Result<Snapshot> {
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(Error::Invalid(format!(
            "Database snapshot integrity check failed: {integrity}"
        )));
    }
    if connection.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(Error::Invalid(
            "Database snapshot has broken foreign keys.".to_owned(),
        ));
    }
    let schema_version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if schema_version != expected_schema {
        return Err(Error::Invalid(
            "Database snapshot schema is not supported by this build.".to_owned(),
        ));
    }
    let present: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id=?1)",
        [pin],
        |row| row.get(0),
    )?;
    if !present {
        return Err(Error::Invalid(
            "Snapshot is missing its maintenance protection record.".to_owned(),
        ));
    }
    let broken: bool = connection.query_row(
        "WITH required(item_id,revision_uuid) AS (
            SELECT item_id,base_revision FROM submission
            UNION ALL SELECT item_id,accepted_revision FROM submission
            UNION ALL SELECT item_id,revision_uuid FROM revision_pin WHERE expires_ms IS NULL OR expires_ms>?1
        ) SELECT EXISTS(SELECT 1 FROM required p WHERE
            NOT EXISTS(SELECT 1 FROM item i WHERE i.id=p.item_id AND i.revision=p.revision_uuid)
            AND NOT EXISTS(SELECT 1 FROM revision r WHERE r.item_id=p.item_id AND r.revision_uuid=p.revision_uuid))",
        [chrono::Utc::now().timestamp_millis()], |row| row.get(0),
    )?;
    if broken {
        return Err(Error::Invalid(
            "Snapshot is missing a retained submission or pinned revision.".to_owned(),
        ));
    }
    verify_manifests(connection)?;
    verify_blocks(connection)?;
    let (project_id, generation) = connection.query_row(
        "SELECT project_uuid,generation FROM project_state WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Snapshot {
        project_id,
        generation,
        schema_version,
        maintenance_pin: pin.to_owned(),
    })
}

fn verify_manifests(connection: &Connection) -> Result<()> {
    let mut manifests =
        connection.prepare("SELECT block_ids FROM body WHERE block_ids IS NOT NULL ORDER BY id")?;
    let mut rows = manifests.query([])?;
    let mut exists = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM block WHERE id=?1)")?;
    while let Some(row) = rows.next()? {
        for id in body::decode(&row.get::<_, Vec<u8>>(0)?)? {
            if !exists.query_row([id], |row| row.get::<_, bool>(0))? {
                return Err(Error::Invalid(
                    "Snapshot document references a missing block.".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn verify_blocks(connection: &Connection) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut statement = connection.prepare("SELECT digest,document_json FROM block ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let expected: Vec<u8> = row.get(0)?;
        let json: String = row.get(1)?;
        if Sha256::digest(json.as_bytes()).as_slice() != expected {
            return Err(Error::Invalid(
                "Snapshot document block checksum does not match its content.".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Called only on an unpublished, fully copied backup restore. Pending Git work
/// and unrelated maintenance owners remain intact for normal startup recovery.
pub(crate) fn prepare_restore(connection: &mut Connection, snapshot: &Snapshot) -> Result<()> {
    if verify(connection, &snapshot.maintenance_pin)? != *snapshot {
        return Err(Error::Invalid(
            "Backup manifest does not match its database snapshot.".to_owned(),
        ));
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM maintenance_pin WHERE id=?1",
        [&snapshot.maintenance_pin],
    )?;
    transaction.execute(
        "UPDATE project_state SET change_epoch=?1,change_floor=generation WHERE singleton=1",
        [uuid::Uuid::now_v7().to_string()],
    )?;
    transaction.execute("DELETE FROM change_batch", [])?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::maintenance;
    use uuid::Uuid;

    #[test]
    fn snapshot_includes_uncheckpointed_wal_and_private_receipts_and_restores_new_epoch() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
        let item = crate::store::commands::assemble_item(&serde_json::json!({
            "meta":{"type":"page","title":"Private draft"},
            "content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"body"}}]}}
        }),None,"UTC").unwrap();
        let revision = item["revision"].as_str().unwrap().to_owned();
        let prepared = crate::store::item::PreparedItem::new(&item).unwrap();
        store
            .write(move |transaction| {
                let id =
                    prepared.install(transaction, crate::store::item::ExpectedRevision::New)?;
                crate::store::receipts::complete(
                    transaction,
                    &crate::store::receipts::Submission {
                        item: id,
                        device: "offline-device",
                        id: "lost-ack",
                        hash: "request-hash",
                        base: &revision,
                    },
                    &revision,
                    &serde_json::json!({"accepted":revision}),
                )?;
                transaction.execute("UPDATE project_state SET generation=17", [])?;
                Ok(())
            })
            .unwrap();
        let destination = root.path().join("backup.sqlite3");
        assert!(create(&store, "missing", &destination).is_err());
        let pin = maintenance::protect(&store, "backup", &destination).unwrap();
        let saved = create(&store, &pin.id, &destination).unwrap();
        assert_eq!(saved.generation, 17);
        assert!(create(&store, &pin.id, &destination).is_err());
        let mut copy = Connection::open(&destination).unwrap();
        assert_eq!(
            copy.query_row::<i64, _, _>("SELECT count(*) FROM submission", [], |row| row.get(0))
                .unwrap(),
            1
        );
        let old_epoch: String = copy
            .query_row("SELECT change_epoch FROM project_state", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(verify(&copy, &pin.id).unwrap(), saved);
        copy.execute("INSERT INTO maintenance_pin(id,kind,destination,created_ms) VALUES ('unrelated','migration','other',0)",[]).unwrap();
        prepare_restore(&mut copy, &saved).unwrap();
        let new_epoch: String = copy
            .query_row("SELECT change_epoch FROM project_state", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_ne!(old_epoch, new_epoch);
        assert_eq!(
            copy.query_row::<i64, _, _>(
                "SELECT count(*) FROM maintenance_pin WHERE id<>'unrelated'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
        assert!(
            copy.query_row::<bool, _, _>(
                "SELECT EXISTS(SELECT 1 FROM maintenance_pin WHERE id='unrelated')",
                [],
                |row| row.get(0)
            )
            .unwrap()
        );
        // A completed/restored backup never implicitly releases its live source pin.
        assert!(store.read(maintenance::protected).unwrap());
        maintenance::release(&store, pin).unwrap();
        assert!(!store.read(maintenance::protected).unwrap());
    }

    #[test]
    fn missing_manifest_blocks_fail_snapshot_verification_and_keep_live_pin() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
        let prepared = body::PreparedBody::new(&serde_json::json!({"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"a"}}]}})).unwrap();
        store
            .write(move |transaction| {
                prepared.install(transaction)?;
                transaction.execute("DELETE FROM block", [])?;
                Ok(())
            })
            .unwrap();
        let destination = root.path().join("broken.sqlite3");
        let pin = maintenance::protect(&store, "backup", &destination).unwrap();
        assert!(
            create(&store, &pin.id, &destination)
                .unwrap_err()
                .to_string()
                .contains("missing block")
        );
        drop(store);
        assert!(
            Store::open(root.path())
                .unwrap()
                .read(maintenance::protected)
                .unwrap()
        );
    }
    #[test]
    fn dangling_private_revision_pin_prevents_backup_completion() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
        store.write(|tx| {
            tx.execute("INSERT INTO entity(uuid) VALUES (?1)",[Uuid::now_v7().to_string()])?;
            tx.execute("INSERT INTO revision_pin(owner_kind,owner_id,item_id,revision_uuid) SELECT 'editor','broken',id,'missing' FROM entity",[])?;
            Ok(())
        }).unwrap();
        let destination = root.path().join("broken.sqlite3");
        let pin = maintenance::protect(&store, "backup", &destination).unwrap();
        assert!(
            create(&store, &pin.id, &destination)
                .unwrap_err()
                .to_string()
                .contains("pinned revision")
        );
        assert!(store.read(maintenance::protected).unwrap());
        store
            .write(|tx| {
                tx.execute("UPDATE revision_pin SET expires_ms=0", [])?;
                Ok(())
            })
            .unwrap();
        assert!(store.read(|tx| verify(tx, &pin.id)).is_ok());
    }
}
