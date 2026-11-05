// SPDX-License-Identifier: MPL-2.0
//! Typed projections can be regenerated from saved content in one WAL transaction.
use super::{DATABASE_NAME, Error, PROJECTOR_VERSION, Result, configure, location, migrations};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::path::Path;

fn version(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT projector_version FROM project_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

fn require_supported(version: i64) -> Result<()> {
    if version > PROJECTOR_VERSION {
        return Err(Error::Invalid(format!(
            "This project uses newer query projections ({version}). Open it with a newer Bluink build."
        )));
    }
    Ok(())
}

pub(super) fn require_current(connection: &Connection) -> Result<()> {
    migrations::require_current(connection)?;
    let current = version(connection)?;
    require_supported(current)?;
    if current != PROJECTOR_VERSION {
        return Err(Error::Invalid(format!(
            "Project query projections need rebuilding (version {current}). Run bluink rebuild-projections --project <folder>."
        )));
    }
    Ok(())
}

/// The writer lock excludes mutations from existing processes, and readers
/// retain their previous complete snapshot until the rebuilt generation commits.
pub(crate) fn rebuild_project(root: &Path) -> Result<i64> {
    let path = location::validate(root)?.join(DATABASE_NAME);
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    migrations::require_current(&db)?;
    require_supported(version(&db)?)?;
    configure(&db)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrations::require_current(&tx)?;
    require_supported(version(&tx)?)?;
    if super::maintenance::protected(&tx)? {
        return Err(Error::Invalid(
            "Finish the existing maintenance operation before rebuilding projections.".into(),
        ));
    }
    super::reproject::rebuild(&tx)?;
    let generation = tx.query_row(
        "UPDATE project_state SET projector_version=?1 WHERE singleton=1 RETURNING generation",
        [PROJECTOR_VERSION],
        |row| row.get(0),
    )?;
    tx.execute_batch("PRAGMA optimize")?;
    tx.commit()?;
    Ok(generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use uuid::Uuid;

    #[test]
    fn guard_existing_processes_and_never_downgrade_newer_projections() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path(), Uuid::now_v7()).unwrap();
        let db = Connection::open(dir.path().join(DATABASE_NAME)).unwrap();
        db.execute("UPDATE project_state SET projector_version=2", [])
            .unwrap();
        assert!(store.read(|_| Ok(())).is_err());
        assert!(store.write(|_| Ok(())).is_err());
        assert!(Store::open(dir.path()).is_err());
        assert!(rebuild_project(dir.path()).is_err());
        assert_eq!(version(&db).unwrap(), 2);
    }

    #[test]
    fn stale_projection_rebuild_switches_complete_snapshots_and_resets_clients() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path(), Uuid::now_v7()).unwrap();
        let cursor = store.read_changes(None).unwrap().cursor;
        let mut db = Connection::open(dir.path().join(DATABASE_NAME)).unwrap();
        db.execute("UPDATE project_state SET projector_version=0", [])
            .unwrap();
        assert!(store.read(|_| Ok(())).is_err());
        let snapshot = db.transaction().unwrap();
        assert_eq!(version(&snapshot).unwrap(), 0);
        assert_eq!(rebuild_project(dir.path()).unwrap(), 1);
        assert_eq!(version(&snapshot).unwrap(), 0);
        snapshot.commit().unwrap();
        assert_eq!(version(&db).unwrap(), PROJECTOR_VERSION);
        let changes = store.read_changes(Some(&cursor)).unwrap();
        assert_eq!(changes.cursor.generation, "1");
        assert_eq!(changes.batches.len(), 1);
        assert!(changes.batches[0].reset);
    }
}
