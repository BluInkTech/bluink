// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical SQLite storage, with a bounded writer and independent read snapshots.

pub(crate) mod activity;
pub(crate) mod assets;
pub(crate) mod backup;
mod block_facts;
pub(crate) mod body;
pub(crate) mod changes;
pub(crate) mod commands;
pub(crate) mod draft;
mod error;
pub(crate) mod fields;
pub(crate) mod fts;
pub(crate) mod git;
pub(crate) mod io;
pub(crate) mod item;
mod location;
pub(crate) mod maintenance;
pub(crate) mod migrations;
pub(crate) mod mutations;
pub(crate) mod normalize;
pub(crate) mod portable;
pub(crate) mod project;
mod projection_version;
pub(crate) mod projections;
pub(crate) mod query;
pub(crate) mod receipts;
mod reproject;
pub(crate) use projection_version::rebuild_project;
pub(crate) mod reads;
pub(crate) mod relations;
pub(crate) mod render_pins;
pub(crate) mod snapshot;
pub(crate) mod snapshots;
#[cfg(test)]
mod tests;
pub(crate) mod theme_packages;

use crossbeam_channel::{Receiver, Sender, bounded};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) use error::{Error, Result};

pub(crate) const DATABASE_NAME: &str = "project.sqlite3";
const SCHEMA_VERSION: i64 = 1;
const PROJECTOR_VERSION: i64 = 1;
const READERS: usize = 4;
const WRITER_QUEUE: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const SCHEMA: &str = include_str!("store/schema.sql");
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub(crate) struct WriteTiming {
    pub(crate) queue: Duration,
    pub(crate) begin: Duration,
    pub(crate) hold: Duration,
}

type OpenStores = std::collections::BTreeMap<PathBuf, Weak<Store>>;
static STORES: OnceLock<Mutex<OpenStores>> = OnceLock::new();

pub(crate) struct Store {
    facet_cache: Mutex<reads::catalogue::FacetCache>,
    render_leases: Mutex<std::collections::BTreeSet<String>>,
    path: PathBuf,
    readers: Mutex<Vec<Connection>>,
    available: Condvar,
    writer: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Store {
    /// One writer queue per open project in this process, shared by all callers.
    pub(crate) fn shared(root: &Path) -> Result<Arc<Self>> {
        let root = root.canonicalize()?;
        let mut stores = STORES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(error::poisoned)?;
        stores.retain(|_, store| store.strong_count() != 0);
        if let Some(store) = stores.get(&root).and_then(Weak::upgrade) {
            return Ok(store);
        }
        let store = Arc::new(Self::open(&root)?);
        stores.insert(root, Arc::downgrade(&store));
        drop(stores);
        Ok(store)
    }

    /// Create a new canonical project. Existing project state is never replaced.
    pub(crate) fn create(root: &Path, project_id: Uuid) -> Result<Self> {
        let root = location::validate(root)?;
        std::fs::create_dir_all(&root)?;
        let path = root.join(DATABASE_NAME);
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        ensure_empty(&connection)?;
        configure(&connection)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let objects: i64 = transaction.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if objects != 0 {
            return Err(Error::Invalid(
                "Project database already exists.".to_owned(),
            ));
        }
        transaction.execute_batch(SCHEMA)?;
        transaction.execute_batch(git::SCHEMA)?;
        transaction.execute(
            "INSERT INTO project_state (singleton, project_uuid, generation, change_epoch,
                change_floor, manifest_generation, projector_version)
             VALUES (1, ?1, 0, ?2, 0, 0, ?3)",
            params![
                project_id.to_string(),
                Uuid::now_v7().to_string(),
                PROJECTOR_VERSION
            ],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Self::start(path, connection)
    }

    /// Opening an unsupported schema refuses it before changing journal settings.
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let root = location::validate(root)?;
        let path = root.join(DATABASE_NAME);
        migrations::before_open(&root, &path)?;
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        projection_version::require_current(&connection)?;
        configure(&connection)?;
        Self::start(path, connection)
    }

    fn start(path: PathBuf, connection: Connection) -> Result<Self> {
        let readers = (0..READERS)
            .map(|_| {
                let reader = connect(&path, false)?;
                reader.pragma_update(None, "query_only", true)?;
                Ok(reader)
            })
            .collect::<Result<Vec<_>>>()?;
        let (sender, receiver): (Sender<Job>, Receiver<Job>) = bounded(WRITER_QUEUE);
        let worker = thread::Builder::new()
            .name("bluink-sqlite-writer".to_owned())
            .spawn(move || {
                let mut connection = connection;
                for job in receiver {
                    job(&mut connection);
                }
            })?;
        Ok(Self {
            path,
            render_leases: Mutex::default(),
            facet_cache: Mutex::default(),
            readers: Mutex::new(readers),
            available: Condvar::new(),
            writer: Some(sender),
            worker: Some(worker),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// All queries used for one response see the same short-lived snapshot.
    pub(crate) fn read<T>(&self, read: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut lease = self.reader(deadline)?;
        let connection = lease.connection.as_mut().ok_or(Error::Closed)?;
        connection.busy_timeout(deadline.saturating_duration_since(Instant::now()))?;
        connection.progress_handler(1_000, Some(move || Instant::now() >= deadline))?;
        let result = (|| {
            let transaction = connection.transaction()?;
            projection_version::require_current(&transaction)?;
            let result = read(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })();
        connection.progress_handler(0, None::<fn() -> bool>)?;
        result
    }

    /// Enqueue one atomic operation. Retryable callers retain their submission ID.
    pub(crate) fn write<T: Send + 'static>(
        &self,
        write: impl FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.timed_write(None, write).map(|(value, timing)| {
            crate::diagnostics::waits::sqlite_write(timing.queue, timing.begin, timing.hold);
            value
        })
    }

    /// Large sync operations publish once. Ordinary writers keep their normal
    /// retry deadline while WAL readers continue to see a committed snapshot.
    pub(crate) fn timed_write<T: Send + 'static>(
        &self,
        cancellation: Option<Arc<AtomicBool>>,
        write: impl FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    ) -> Result<(T, WriteTiming)> {
        let budget = if cancellation.is_some() {
            Duration::from_mins(15)
        } else {
            REQUEST_TIMEOUT
        };
        let started = Instant::now();
        let deadline = started + budget;
        let (sender, receiver) = bounded(1);
        let job = Box::new(move |connection: &mut Connection| {
            let dequeued = Instant::now();
            let cancel_requested = || {
                cancellation
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Acquire))
            };
            let mut result = (|| {
                if Instant::now() >= deadline || cancel_requested() {
                    return Err(Error::Busy);
                }
                connection.busy_timeout(
                    REQUEST_TIMEOUT.min(deadline.saturating_duration_since(Instant::now())),
                )?;
                // Interrupt at the deadline, not after it: a write that will be
                // rolled back must not keep every queued save waiting.
                let flag = cancellation.as_ref().map(Arc::clone);
                connection.progress_handler(
                    1_000,
                    Some(move || {
                        flag.as_ref()
                            .is_some_and(|flag| flag.load(Ordering::Acquire))
                            || Instant::now() >= deadline
                    }),
                )?;
                let begin = Instant::now();
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let acquired = Instant::now();
                projection_version::require_current(&transaction)?;
                let value = write(&transaction)?;
                if Instant::now() >= deadline || cancel_requested() {
                    return Err(Error::Busy);
                }
                transaction.commit()?;
                let timing = WriteTiming {
                    queue: dequeued.duration_since(started),
                    begin: acquired.duration_since(begin),
                    hold: acquired.elapsed(),
                };
                tracing::debug!(
                    operation = "sqlite.write",
                    queue_ms = timing.queue.as_secs_f64() * 1000.0,
                    begin_immediate_ms = timing.begin.as_secs_f64() * 1000.0,
                    transaction_ms = timing.hold.as_secs_f64() * 1000.0,
                    "Committed SQLite transaction"
                );
                Ok((value, timing))
            })();
            // An interrupted SQL statement drops its transaction before this
            // connection is reused by the next queued save.
            if result.is_err() && (cancel_requested() || Instant::now() >= deadline) {
                result = Err(Error::Busy);
            }
            if let Err(error) = connection.progress_handler(0, None::<fn() -> bool>) {
                result = Err(error.into());
            }
            // A timed-out caller can recover a committed result through its receipt.
            let _ = sender.send(result);
        });
        self.writer
            .as_ref()
            .ok_or(Error::Closed)?
            .send_deadline(job, started + REQUEST_TIMEOUT)
            .map_err(|error| match error {
                crossbeam_channel::SendTimeoutError::Timeout(_) => Error::Busy,
                crossbeam_channel::SendTimeoutError::Disconnected(_) => Error::Closed,
            })?;
        receiver
            .recv_deadline(deadline)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => Error::Busy,
                crossbeam_channel::RecvTimeoutError::Disconnected => Error::Closed,
            })?
    }

    fn reader(&self, deadline: Instant) -> Result<Reader<'_>> {
        let mut readers = self.readers.lock().map_err(error::poisoned)?;
        loop {
            if let Some(connection) = readers.pop() {
                return Ok(Reader {
                    store: self,
                    connection: Some(connection),
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Busy);
            }
            let (next, _) = self
                .available
                .wait_timeout(readers, remaining)
                .map_err(error::poisoned)?;
            readers = next;
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        drop(self.writer.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("SQLite writer terminated unexpectedly");
        }
    }
}

struct Reader<'a> {
    store: &'a Store,
    connection: Option<Connection>,
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            if let Ok(mut readers) = self.store.readers.lock() {
                readers.push(connection);
            } else {
                tracing::error!("SQLite reader pool was poisoned");
            }
            self.store.available.notify_one();
        }
    }
}

fn connect(path: &Path, create: bool) -> Result<Connection> {
    let flags = if create {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let connection = Connection::open_with_flags(path, flags)?;
    configure(&connection)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(REQUEST_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.set_prepared_statement_cache_capacity(64);
    Ok(())
}

fn ensure_empty(connection: &Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let objects: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if version != 0 || objects != 0 {
        return Err(Error::Invalid(
            "Project database already exists.".to_owned(),
        ));
    }
    Ok(())
}
