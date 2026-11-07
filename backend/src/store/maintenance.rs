// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded retention and a resumable mark/sweep over immutable document manifests.
//! The disposable mark database is never a source of truth after a restart.

#[cfg(test)]
mod tests;

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusqlite::{Connection, Transaction, params};
use tempfile::TempDir;

use super::{Error, Result, Store, body};

pub(crate) const DRAFT_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
const PRUNE_ROWS: i64 = 128;
const READ_ROWS: usize = 128;
const READ_BYTES: usize = 1_048_576;
const WRITER_ROWS: usize = 16;
const WRITER_BYTES: usize = 131_072;
const WRITER_TIME: Duration = Duration::from_millis(25);
const BLOCK_ROWS: i64 = 256;
const PRUNE_BUDGET: Duration = Duration::from_millis(250);

/// One bounded step per worker tick. Queued saves can run between steps. The
/// completed-sweep timestamp survives a restart so opening a project is cheap.
#[derive(Default)]
pub(crate) struct Housekeeping {
    last_prune: Option<Instant>,
    sweep: Option<BlockSweep>,
}

impl Housekeeping {
    pub(crate) fn tick(&mut self, store: &Store) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        if self
            .last_prune
            .is_none_or(|last| last.elapsed() >= Duration::from_mins(5))
        {
            self.last_prune = Some(Instant::now());
            reclaim(store)?;
            super::render_pins::maintain(store, now)?;
            let pruned = prune(store, now)?;
            tracing::debug!(
                operation = "sqlite.retention",
                pins = pruned.pins,
                receipts = pruned.receipts,
                revisions = pruned.revisions,
                bodies = pruned.bodies,
                "Pruned expired project records"
            );
            let due = store.write(move |tx| {
                if protected(tx)? {
                    return Ok(false);
                }
                let config = super::project::read_config(tx)?;
                let days = config["analytics"]["eventRetentionDays"]
                    .as_i64()
                    .ok_or_else(|| Error::corrupt("Project has invalid activity retention."))?;
                let before = now.saturating_sub(days.saturating_mul(DRAFT_WINDOW_MS));
                super::activity::prune(tx, before)?;
                let last: i64 = tx.query_row(
                    "SELECT last_block_sweep_ms FROM maintenance_state",
                    [],
                    |row| row.get(0),
                )?;
                Ok(pruned.bodies > 0 || now.saturating_sub(last) >= DRAFT_WINDOW_MS)
            })?;
            // One editor publishes a batch per autosave; drain the expired feed
            // in short writes rather than a fixed handful per tick.
            let started = Instant::now();
            while store.write(move |tx| {
                if protected(tx)? {
                    return Ok(false);
                }
                super::changes::prune(tx, now.saturating_sub(DRAFT_WINDOW_MS))
            })? && started.elapsed() < PRUNE_BUDGET
            {}
            if due && self.sweep.is_none() {
                self.sweep = Some(BlockSweep::begin(store)?);
            }
        }
        if let Some(sweep) = self.sweep.take() {
            let (sweep, progress) = sweep.step(store)?;
            if progress == SweepStep::Complete {
                store.write(move |tx| {
                    tx.execute("UPDATE maintenance_state SET last_block_sweep_ms=?1", [now])?;
                    Ok(())
                })?;
            } else {
                self.sweep = Some(sweep);
            }
        }
        Ok(())
    }
}

/// Physical retention for another owner does not extend a stale draft's lifetime.
pub(crate) fn renew_editor_base(
    tx: &Transaction<'_>,
    lease: &str,
    uuid: &str,
    revision: &str,
    now_ms: i64,
) -> Result<bool> {
    use rusqlite::OptionalExtension;
    let item: Option<i64> = tx
        .query_row(
            "SELECT id FROM item WHERE uuid=?1 AND type IN ('page','template')",
            [uuid],
            |row| row.get(0),
        )
        .optional()?;
    let Some(item) = item else { return Ok(false) };
    if !base_is_admissible(tx, item, revision, now_ms)? {
        return Ok(false);
    }
    tx.execute("DELETE FROM revision_pin WHERE owner_kind='editor' AND owner_id=?1 AND (item_id!=?2 OR revision_uuid!=?3)",params![lease,item,revision])?;
    super::receipts::pin(
        tx,
        "editor",
        lease,
        item,
        revision,
        Some(now_ms.saturating_add(DRAFT_WINDOW_MS)),
    )?;
    Ok(true)
}

/// A base is admissible while current, inside the recovery window, or held by
/// an unexpired editor lease. Physical retention for another owner (a Git
/// capture, render or backup pin) does not extend a stale draft's lifetime.
pub(crate) fn base_is_admissible(
    transaction: &Transaction<'_>,
    item: i64,
    revision: &str,
    now_ms: i64,
) -> Result<bool> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM item WHERE id=?1 AND revision=?2
         UNION ALL SELECT 1 FROM revision WHERE item_id=?1 AND revision_uuid=?2 AND superseded_ms>=?3
         UNION ALL SELECT 1 FROM revision_pin WHERE owner_kind='editor' AND item_id=?1
             AND revision_uuid=?2 AND (expires_ms IS NULL OR expires_ms>?4))",
        params![item, revision, now_ms.saturating_sub(DRAFT_WINDOW_MS), now_ms],
        |row| row.get(0),
    )?)
}

#[derive(Debug, Default)]
pub(crate) struct Pruned {
    pub(crate) pins: usize,
    pub(crate) receipts: usize,
    pub(crate) revisions: usize,
    pub(crate) bodies: usize,
}

/// Candidate scans use read snapshots. Each writer phase rechecks no more than
/// 128 exact keys, including pins installed since that snapshot was collected.
pub(crate) fn prune(store: &Store, now_ms: i64) -> Result<Pruned> {
    let pins = prune_target(store, now_ms, &EXPIRED_PINS)?;
    let receipts = prune_target(store, now_ms, &EXPIRED_RECEIPTS)?;
    let revisions = prune_target(store, now_ms, &EXPIRED_REVISIONS)?;
    let bodies = prune_target(store, now_ms, &ORPHAN_BODIES)?;
    Ok(Pruned {
        pins,
        receipts,
        revisions,
        bodies,
    })
}

struct PruneTarget {
    table: &'static str,
    keys: &'static [&'static str],
    predicate: &'static str,
}

const EXPIRED_PINS: PruneTarget = PruneTarget {
    table: "revision_pin",
    keys: &["owner_kind", "owner_id", "item_id", "revision_uuid"],
    predicate: "expires_ms<=?2",
};
const EXPIRED_RECEIPTS: PruneTarget = PruneTarget {
    table: "submission",
    keys: &["item_id", "device_id", "submission_uuid"],
    predicate: "NOT EXISTS(SELECT 1 FROM item i WHERE i.id=submission.item_id AND i.revision=submission.base_revision)
        AND NOT EXISTS(SELECT 1 FROM revision r WHERE r.item_id=submission.item_id
            AND r.revision_uuid=submission.base_revision AND r.superseded_ms>=?1)
        AND NOT EXISTS(SELECT 1 FROM revision_pin p WHERE p.item_id=submission.item_id
            AND p.revision_uuid=submission.base_revision AND (p.expires_ms IS NULL OR p.expires_ms>?2))",
};
const EXPIRED_REVISIONS: PruneTarget = PruneTarget {
    table: "revision",
    keys: &["item_id", "revision_uuid"],
    predicate: "superseded_ms<?1
        AND NOT EXISTS(SELECT 1 FROM revision_pin p WHERE p.item_id=revision.item_id
            AND p.revision_uuid=revision.revision_uuid AND (p.expires_ms IS NULL OR p.expires_ms>?2))
        AND NOT EXISTS(SELECT 1 FROM submission s WHERE s.item_id=revision.item_id
            AND (s.accepted_revision=revision.revision_uuid OR s.base_revision=revision.revision_uuid))",
};
const ORPHAN_BODIES: PruneTarget = PruneTarget {
    table: "body",
    keys: &["id"],
    predicate: "NOT EXISTS(SELECT 1 FROM item i WHERE i.body_id=body.id)
        AND NOT EXISTS(SELECT 1 FROM revision r WHERE r.body_id=body.id)",
};

fn candidates(
    store: &Store,
    now: i64,
    target: &PruneTarget,
) -> Result<Vec<Vec<rusqlite::types::Value>>> {
    let query = format!(
        "SELECT {} FROM {} WHERE {} LIMIT ?3",
        target.keys.join(","),
        target.table,
        target.predicate
    );
    store.read(|transaction| {
        if protected(transaction)? {
            return Ok(Vec::new());
        }
        let mut statement = transaction.prepare(&query)?;
        let rows = statement.query_map(
            params![now.saturating_sub(DRAFT_WINDOW_MS), now, PRUNE_ROWS],
            |row| (0..target.keys.len()).map(|index| row.get(index)).collect(),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

/// Repeat bounded batches until the backlog is gone or this tick's budget is
/// spent, so retention keeps pace with sustained writes and bulk syncs.
/// Queued saves still run between batches.
fn prune_target(store: &Store, now: i64, target: &'static PruneTarget) -> Result<usize> {
    let started = Instant::now();
    let mut total = 0;
    loop {
        let rows = candidates(store, now, target)?;
        let full = i64::try_from(rows.len()).is_ok_and(|count| count >= PRUNE_ROWS);
        if rows.is_empty() {
            return Ok(total);
        }
        let deleted =
            store.write(move |transaction| delete_candidates(transaction, now, target, rows))?;
        total += deleted;
        if !full || deleted == 0 || started.elapsed() >= PRUNE_BUDGET {
            return Ok(total);
        }
    }
}

fn delete_candidates(
    transaction: &Transaction<'_>,
    now: i64,
    target: &PruneTarget,
    rows: Vec<Vec<rusqlite::types::Value>>,
) -> Result<usize> {
    if protected(transaction)? {
        return Ok(0);
    }
    let keys = target
        .keys
        .iter()
        .enumerate()
        .map(|(index, key)| format!("{key}=?{}", index + 3))
        .collect::<Vec<_>>()
        .join(" AND ");
    let query = format!(
        "DELETE FROM {} WHERE {keys} AND {}",
        target.table, target.predicate
    );
    let mut statement = transaction.prepare(&query)?;
    let mut deleted = 0;
    for row in rows {
        let values = [
            rusqlite::types::Value::Integer(now.saturating_sub(DRAFT_WINDOW_MS)),
            rusqlite::types::Value::Integer(now),
        ]
        .into_iter()
        .chain(row);
        deleted += statement.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(deleted)
}

#[derive(Clone, Copy, Debug, Default)]
struct Position {
    generation: i64,
    body: i64,
}

struct Manifest {
    position: Position,
    bytes: Vec<u8>,
}

/// Owns a disk-backed set so a large corpus does not grow the server's heap.
pub(crate) struct BlockSweep {
    marks: Connection,
    _directory: TempDir,
    ceiling: i64,
    target: i64,
    position: Position,
    scan_after: i64,
    marked: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SweepStep {
    Marked { manifests: usize, bytes: usize },
    Yielded,
    Deleted { scanned: usize, deleted: usize },
    Complete,
}

impl BlockSweep {
    pub(crate) fn begin(store: &Store) -> Result<Self> {
        let (ceiling, target) = store.read(|transaction| {
            Ok(transaction.query_row(
                "SELECT coalesce((SELECT max(id) FROM block),0),manifest_generation
                 FROM project_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })?;
        let directory = tempfile::Builder::new()
            .prefix("bluink-block-sweep-")
            .tempdir()?;
        let marks = Connection::open(directory.path().join("marks.sqlite3"))?;
        marks.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
             CREATE TABLE marked(id INTEGER PRIMARY KEY) STRICT;",
        )?;
        Ok(Self {
            marks,
            _directory: directory,
            ceiling,
            target,
            position: Position::default(),
            scan_after: 0,
            marked: false,
        })
    }

    /// Advance one bounded read or write. Repeated yields retain all earlier marks.
    pub(crate) fn step(mut self, store: &Store) -> Result<(Self, SweepStep)> {
        if !self.marked {
            let (manifests, complete) = store.read(|transaction| {
                read_manifests(
                    transaction,
                    self.position,
                    self.target,
                    READ_ROWS,
                    READ_BYTES,
                    true,
                )
            })?;
            let count = manifests.len();
            let bytes = manifests.iter().map(|manifest| manifest.bytes.len()).sum();
            self.mark(&manifests, None)?;
            if complete {
                self.caught_up(self.target);
                self.marked = true;
            }
            return Ok((
                self,
                SweepStep::Marked {
                    manifests: count,
                    bytes,
                },
            ));
        }
        store.write(move |transaction| {
            if protected(transaction)? {
                return Ok((self, SweepStep::Yielded));
            }
            let started = Instant::now();
            let target = transaction.query_row(
                "SELECT manifest_generation FROM project_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let (tail, complete) = read_manifests(
                transaction,
                self.position,
                target,
                WRITER_ROWS,
                WRITER_BYTES,
                false,
            )?;
            if !complete || started.elapsed() >= WRITER_TIME {
                self.target = target;
                self.marked = false;
                return Ok((self, SweepStep::Yielded));
            }
            if !self.mark(&tail, Some(started + WRITER_TIME))? {
                self.target = target;
                self.marked = false;
                return Ok((self, SweepStep::Yielded));
            }
            self.caught_up(target);
            if started.elapsed() >= WRITER_TIME {
                return Ok((self, SweepStep::Yielded));
            }
            let mut statement = transaction.prepare_cached(
                "SELECT id FROM block WHERE id>?1 AND id<=?2 ORDER BY id LIMIT ?3",
            )?;
            let ids = statement
                .query_map(params![self.scan_after, self.ceiling, BLOCK_ROWS], |row| {
                    row.get::<_, i64>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if ids.is_empty() {
                return Ok((self, SweepStep::Complete));
            }
            let mut deleted = 0;
            let mut scanned = 0;
            let mut marked = self
                .marks
                .prepare_cached("SELECT EXISTS(SELECT 1 FROM marked WHERE id=?1)")?;
            for id in ids {
                if started.elapsed() >= WRITER_TIME {
                    break;
                }
                if !marked.query_row([id], |row| row.get::<_, bool>(0))? {
                    deleted += transaction.execute("DELETE FROM block WHERE id=?1", [id])?;
                }
                self.scan_after = id;
                scanned += 1;
            }
            drop(marked);
            Ok((self, SweepStep::Deleted { scanned, deleted }))
        })
    }

    fn mark(&mut self, manifests: &[Manifest], deadline: Option<Instant>) -> Result<bool> {
        let transaction = self.marks.transaction()?;
        let mut complete = true;
        {
            let mut insert =
                transaction.prepare_cached("INSERT OR IGNORE INTO marked(id) VALUES (?1)")?;
            'manifests: for manifest in manifests {
                for id in body::decode(&manifest.bytes)? {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        complete = false;
                        break 'manifests;
                    }
                    insert.execute([id])?;
                }
                self.position = manifest.position;
            }
        }
        transaction.commit()?;
        Ok(complete)
    }

    const fn caught_up(&mut self, generation: i64) {
        self.position = Position {
            generation,
            body: i64::MAX,
        };
    }
}

/// Git/LFS pruning must consult the same durable guard before deleting objects.
pub(crate) fn protected(transaction: &Transaction<'_>) -> Result<bool> {
    Ok(
        transaction.query_row("SELECT EXISTS(SELECT 1 FROM maintenance_pin)", [], |row| {
            row.get(0)
        })?,
    )
}

/// A backup or export pin. Its owner holds an exclusive lock on a file named
/// after the pin for as long as it copies, so housekeeping in any process can
/// tell a running copy from one whose process stopped without releasing it.
pub(crate) struct Pin {
    pub(crate) id: String,
    lock: File,
    path: PathBuf,
}

fn lock_path(store: &Store, id: &str) -> Result<PathBuf> {
    let root = store
        .path()
        .parent()
        .ok_or_else(|| Error::Invalid("Project database has no parent directory.".to_owned()))?;
    Ok(root.join(".maintenance").join(format!("{id}.lock")))
}

fn open_lock(path: &std::path::Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

pub(crate) fn protect(store: &Store, kind: &str, destination: &std::path::Path) -> Result<Pin> {
    let id = uuid::Uuid::now_v7().to_string();
    // Lock before the row exists, so no reader ever sees an unlocked live pin.
    let path = lock_path(store, &id)?;
    let lock = open_lock(&path)?;
    lock.lock()?;
    let row = id.clone();
    let kind = kind.to_owned();
    let destination = destination.to_string_lossy().into_owned();
    let inserted = store.write(move |transaction| {
        transaction.execute(
            "INSERT INTO maintenance_pin(id,kind,destination,created_ms) VALUES (?1,?2,?3,?4)",
            params![
                row,
                kind,
                destination,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        Ok(())
    });
    if let Err(error) = inserted {
        drop(lock);
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(Pin { id, lock, path })
}

/// Deliberate completion only; dropping a pin keeps its row until a later
/// housekeeping pass finds the lock free.
pub(crate) fn release(store: &Store, pin: Pin) -> Result<()> {
    let id = pin.id.clone();
    store.write(move |transaction| {
        transaction.execute("DELETE FROM maintenance_pin WHERE id=?1", [id])?;
        Ok(())
    })?;
    drop(pin.lock);
    let _ = fs::remove_file(&pin.path);
    Ok(())
}

/// Any pin blocks retention, sweeping and packing, so a backup or export whose
/// process crashed must not keep its pin forever. Migration pins are left for
/// upgrade recovery.
fn reclaim(store: &Store) -> Result<()> {
    let ids: Vec<String> = store.read(|transaction| {
        let mut statement = transaction
            .prepare("SELECT id FROM maintenance_pin WHERE kind IN ('backup','export')")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    })?;
    for id in ids {
        let path = lock_path(store, &id)?;
        let Ok(lock) = open_lock(&path) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue;
        }
        let row = id.clone();
        store.write(move |transaction| {
            transaction.execute("DELETE FROM maintenance_pin WHERE id=?1", [row])?;
            Ok(())
        })?;
        tracing::warn!(operation = "sqlite.maintenance", pin = %id,
            "Released a backup or export pin left by a stopped process");
        drop(lock);
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

/// Inspect lengths first: a large tail never loads unbounded bytes under the writer.
fn read_manifests(
    transaction: &Transaction<'_>,
    after: Position,
    through: i64,
    rows: usize,
    bytes: usize,
    allow_first_large: bool,
) -> Result<(Vec<Manifest>, bool)> {
    let mut statement = transaction.prepare_cached(
        "SELECT installed_generation,id,length(block_ids) FROM body
         WHERE installed_generation<=?3 AND (installed_generation>?1 OR (installed_generation=?1 AND id>?2))
         ORDER BY installed_generation,id LIMIT ?4",
    )?;
    let row_limit = i64::try_from(rows + 1).map_err(|error| Error::Invalid(error.to_string()))?;
    let mut found = statement.query(params![after.generation, after.body, through, row_limit])?;
    let mut manifests = Vec::new();
    let mut total = 0;
    while let Some(row) = found.next()? {
        let position = Position {
            generation: row.get(0)?,
            body: row.get(1)?,
        };
        let length = usize::try_from(row.get::<_, i64>(2)?)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        if manifests.len() == rows
            || (total + length > bytes && !(allow_first_large && manifests.is_empty()))
        {
            return Ok((manifests, false));
        }
        let payload = transaction.query_row(
            "SELECT block_ids FROM body WHERE id=?1",
            [position.body],
            |row| row.get(0),
        )?;
        manifests.push(Manifest {
            position,
            bytes: payload,
        });
        total += length;
    }
    Ok((manifests, true))
}
