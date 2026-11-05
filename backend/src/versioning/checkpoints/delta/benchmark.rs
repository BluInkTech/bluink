// SPDX-License-Identifier: MPL-2.0
//! Explicit workload gates around the production SQL/ref acceptance path.
use super::{Publication, apply, prepare};
use crate::domain::project::{canonical_json, identity::ItemId};
use crate::store::{self, Store, commands::assemble_item, mutations};
use crate::versioning::{checkpoints, repository};
use git2::{Oid, Repository};
use rusqlite::hooks::Action;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Project {
    root: tempfile::TempDir,
    store: Arc<Store>,
    repo: Repository,
    head: Oid,
    pages: Vec<Value>,
    autosave: Value,
}

fn prose() -> String {
    let mut value = String::with_capacity(16_384);
    for index in 0.. {
        write!(
            value,
            "Harbor document paragraph {} contains ordinary words. ",
            index % 96
        )
        .unwrap();
        if value.len() >= 16_384 {
            value.truncate(16_384);
            return value;
        }
    }
    unreachable!()
}

fn page(text: &str) -> Value {
    assemble_item(&json!({"meta":{"type":"page","title":"Original"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"prose"},"content":[{"type":"text","text":text}]}]}}}),None,"UTC").unwrap()
}

fn project() -> Project {
    let root = tempfile::tempdir().unwrap();
    let template: Value = serde_json::from_slice(include_bytes!(
        "../../../../resources/workspace-starter.json"
    ))
    .unwrap();
    let (store, _) = store::project::open_or_create(root.path(), "UTC", &template).unwrap();
    checkpoints::initialize(root.path()).unwrap();
    let text = prose();
    let pages = (0..100).map(|_| page(&text)).collect::<Vec<_>>();
    let autosave = page("Live editor");
    for batch in pages
        .chunks(25)
        .chain(std::iter::once(std::slice::from_ref(&autosave)))
    {
        let changes = batch
            .iter()
            .map(|item| mutations::PreparedChange::new(item.clone(), None).unwrap())
            .collect::<Vec<_>>();
        store
            .write(move |tx| mutations::apply(tx, &changes, &[], None))
            .unwrap();
    }
    let head =
        crate::versioning::test_support::checkpoint(root.path(), &store, "device", "Benchmark")
            .unwrap()
            .unwrap();
    let repo = checkpoints::open(root.path()).unwrap();
    Project {
        root,
        store,
        repo,
        head,
        pages,
        autosave,
    }
}

fn changed(mut item: Value, sequence: usize, text: &str) -> Value {
    item["revision"] = json!(Uuid::now_v7().to_string());
    item["meta"]["title"] = json!(format!("Applied version {sequence}"));
    item["content"]["document"]["content"][0]["content"][0]["text"] =
        json!(format!("liveset{sequence} {} {text}", item["id"]));
    item
}

fn candidate(project: &Project, count: usize, sequence: usize) -> Oid {
    let text = prose();
    let mut paths = BTreeMap::new();
    for index in 0..count {
        let original = project
            .pages
            .get(index)
            .cloned()
            .unwrap_or_else(|| page(&text));
        let item = changed(original, sequence, &text);
        let id = ItemId::try_from(item["id"].as_str().unwrap()).unwrap();
        let blob = project
            .repo
            .blob(&canonical_json::to_vec(&item).unwrap())
            .unwrap();
        paths.insert(
            id.path(Path::new("")).to_string_lossy().replace('\\', "/"),
            Some(blob),
        );
    }
    let tree = repository::changed_tree(&project.repo, Some(project.head), &paths).unwrap();
    let signature = git2::Signature::now("Source", "source@bluink.invalid").unwrap();
    project
        .repo
        .commit(
            None,
            &signature,
            &signature,
            "Apply fixture",
            &project.repo.find_tree(tree).unwrap(),
            &[&project.repo.find_commit(project.head).unwrap()],
        )
        .unwrap()
}

fn generation(store: &Store) -> i64 {
    store
        .read(|tx| Ok(tx.query_row("SELECT generation FROM project_state", [], |row| row.get(0))?))
        .unwrap()
}

fn publication(project: &Project, candidate: Oid, cancelled: Arc<AtomicBool>) -> Publication {
    Publication {
        previous: project.head,
        candidate,
        generation: generation(&project.store),
        tags: BTreeMap::new(),
        cancelled,
    }
}

fn p95(times: &mut [f64]) -> f64 {
    times.sort_by(f64::total_cmp);
    times[(times.len() * 95).div_ceil(100) - 1]
}

/// Signals only after the real writer has changed an item in its uncommitted transaction.
fn first_update(store: &Store) -> mpsc::Receiver<()> {
    let (started, receiver) = mpsc::sync_channel(1);
    let once = AtomicBool::new(false);
    store
        .write(move |tx| {
            tx.update_hook(Some(move |_: Action, _: &str, table: &str, _: i64| {
                if table == "item" && !once.swap(true, Ordering::AcqRel) {
                    let _ = started.try_send(());
                }
            }))?;
            Ok(())
        })
        .unwrap();
    receiver
}

fn read_pages(store: &Store, sequence: usize, count: i64, stop: &AtomicBool) -> usize {
    let mut reads = 0;
    while !stop.load(Ordering::Acquire) {
        store
            .read(|tx| {
                let rows = tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM item WHERE title=?1",
                    [format!("Applied version {sequence}")],
                    |row| row.get(0),
                )?;
                let indexed = tx.query_row::<i64, _, _>(
                    "SELECT count(*) FROM item_fts WHERE item_fts MATCH ?1",
                    [store::normalize::fts_token(&format!("liveset{sequence}"))],
                    |row| row.get(0),
                )?;
                assert!(rows == 0 || rows == count);
                assert_eq!(rows, indexed);
                Ok(())
            })
            .unwrap();
        reads += 1;
        thread::sleep(Duration::from_millis(1));
    }
    reads
}

#[test]
#[ignore = "Explicit live sync benchmark with 100 x 16 KiB changed pages, four WAL readers and queued autosaves"]
fn measure_live_sync_with_contending_autosaves() {
    let mut project = project();
    let mut held = Vec::new();
    let mut saved = Vec::new();
    let mut queued = Vec::new();
    let mut begin = Vec::new();
    let mut reads = 0;
    let mut retries = 0;
    for sequence in 1..=100 {
        let next = candidate(&project, 100, sequence);
        let delta = prepare(project.root.path(), &project.repo, project.head, next).unwrap();
        assert!(!delta.needs_maintenance());
        let pending = publication(&project, next, Arc::new(AtomicBool::new(false)));
        let started = first_update(&project.store);
        let mut next_save = project.autosave.clone();
        next_save["revision"] = json!(Uuid::now_v7().to_string());
        next_save["content"]["document"]["content"][0]["content"][0]["text"] =
            json!(format!("Editor save {sequence}"));
        let change = mutations::PreparedChange::new(
            next_save.clone(),
            Some(project.autosave["revision"].as_str().unwrap().into()),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let readers = (0..4)
                .map(|_| scope.spawn(|| read_pages(&project.store, sequence, 100, &stop)))
                .collect::<Vec<_>>();
            let store = Arc::clone(&project.store);
            let autosave = scope.spawn(move || {
                started.recv_timeout(Duration::from_secs(10)).unwrap();
                let start = Instant::now();
                let result = store.timed_write(None, move |tx| {
                    mutations::apply(tx, &[change], &[], Some("2026-11-01"))
                });
                (result, start.elapsed().as_secs_f64() * 1000.0)
            });
            let result = apply(&project.repo, &project.store, delta, pending);
            stop.store(true, Ordering::Release);
            reads += readers
                .into_iter()
                .map(|reader| reader.join().unwrap())
                .sum::<usize>();
            held.push(result.unwrap().hold.as_secs_f64() * 1000.0);
            let (ack, elapsed) = autosave.join().unwrap();
            if matches!(ack, Err(store::Error::Busy)) {
                retries += 1;
            }
            let (_, timing) = ack.unwrap();
            queued.push(timing.queue.as_secs_f64() * 1000.0);
            begin.push(timing.begin.as_secs_f64() * 1000.0);
            saved.push(elapsed);
        });
        project.autosave = next_save;
        project.head = crate::versioning::test_support::checkpoint(
            project.root.path(),
            &project.store,
            "device",
            "Benchmark",
        )
        .unwrap()
        .unwrap();
    }
    let report = json!({"samples":held.len(),"changedPages":100,"bodyBytesPerPage":16384,"concurrentReaders":4,"readerSnapshots":reads,"busyRetries":retries,"writerMaxMs":held.iter().copied().fold(0.0,f64::max),"writerP95Ms":p95(&mut held),"autosaveP95Ms":p95(&mut saved),"autosaveQueueP95Ms":p95(&mut queued),"autosaveBeginImmediateP95Ms":p95(&mut begin),"autosaveBeginImmediateMs":begin,"autosaveMaxMs":saved.last(),"writerMs":held,"autosaveMs":saved,"scope":"SQL writer acquisition through commit and queued canonical body autosave acknowledgement; preparation, Git object construction, HTTP and browser are outside those intervals"});
    println!("LIVE_SYNC_REPORT={report}");
}

#[test]
#[ignore = "Explicit 10k-page maintenance apply and interrupted writer rollback"]
fn measure_large_sync_and_interrupt_before_publication() {
    let project = project();
    let next = candidate(&project, 10_000, 1);
    let before = generation(&project.store);
    let token = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::clone(&token);
    project
        .store
        .write(move |tx| {
            let rows = std::sync::atomic::AtomicUsize::new(0);
            tx.update_hook(Some(move |_: Action, _: &str, table: &str, _: i64| {
                if table == "item" && rows.fetch_add(1, Ordering::AcqRel) == 249 {
                    cancelled.store(true, Ordering::Release);
                }
            }))?;
            Ok(())
        })
        .unwrap();
    let aborted = prepare(project.root.path(), &project.repo, project.head, next).unwrap();
    assert!(aborted.needs_maintenance());
    assert!(
        apply(
            &project.repo,
            &project.store,
            aborted,
            publication(&project, next, token)
        )
        .is_err()
    );
    assert_eq!(generation(&project.store), before);
    assert_eq!(repository::head(&project.repo).unwrap(), Some(project.head));
    project
        .store
        .write(|tx| {
            tx.update_hook(None::<fn(Action, &str, &str, i64)>)?;
            Ok(())
        })
        .unwrap();
    let preparing = Instant::now();
    let delta = prepare(project.root.path(), &project.repo, project.head, next).unwrap();
    let prepare_ms = preparing.elapsed().as_secs_f64() * 1000.0;
    let result = apply(
        &project.repo,
        &project.store,
        delta,
        publication(&project, next, Arc::new(AtomicBool::new(false))),
    )
    .unwrap();
    let changed = project
        .store
        .read(|tx| {
            Ok(tx.query_row::<i64, _, _>(
                "SELECT count(*) FROM item WHERE title='Applied version 1'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(changed, 10_000);
    assert_eq!(repository::head(&project.repo).unwrap(), Some(next));
    println!(
        "LARGE_SYNC_REPORT={}",
        json!({"pages":10000,"bodyBytesPerPage":16384,"prepareMs":prepare_ms,"writerHoldMs":result.hold.as_secs_f64()*1000.0,"interruptedAfterRows":250,"partialRowsPublished":0,"successfulRetry":true})
    );
}
