// SPDX-License-Identifier: MPL-2.0
//! Explicit measurement fixture; not a timer assertion in ordinary unit tests.
use crate::store::{
    Store, git,
    item::{ExpectedRevision, PreparedItem},
};
use crate::versioning::{
    checkpoints,
    model::{HistoryRequest, NamedVersionRequest},
    repository,
};
use git2::{Oid, Repository};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Instant};
use uuid::Uuid;

fn seed(store: &Store) -> Value {
    let value=crate::store::commands::assemble_item(&json!({"meta":{"type":"page","title":"History probe"},"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"first"},"content":[{"type":"text","text":"History fixture"}]}]}}}),None,"UTC").unwrap();
    let saved = value.clone();
    store.write(move |tx|{
        PreparedItem::new(&saved)?.install(tx,ExpectedRevision::New)?;
        tx.execute("UPDATE project_state SET generation=1",[])?;
        git::mark_item_dirty(tx,saved["id"].as_str().unwrap(),saved["revision"].as_str().unwrap(),1)?;
        let project:String=tx.query_row("SELECT project_uuid FROM project_state",[],|row|row.get(0))?;
        git::put_record(tx,"bluink.json",&json!({"format":"bluink-project","formatVersion":20,"id":project,"defaultWorkspaceId":Uuid::now_v7().to_string(),"analytics":{"timezone":"UTC","eventRetentionDays":14}}),&Uuid::now_v7().to_string(),1)
    }).unwrap();
    value
}

fn item_path(value: &Value) -> String {
    crate::domain::project::identity::ItemId::try_from(value["id"].as_str().unwrap())
        .unwrap()
        .path(Path::new(""))
        .to_string_lossy()
        .replace('\\', "/")
}

fn commit(
    repo: &Repository,
    previous: Oid,
    other: Option<Oid>,
    value: &mut Value,
    index: usize,
) -> Oid {
    value["meta"]["title"] = json!(format!("Revision {index}"));
    value["revision"] = json!(Uuid::now_v7().to_string());
    let blob = repo
        .blob(&crate::domain::project::canonical_json::to_vec(value).unwrap())
        .unwrap();
    let tree = repository::changed_tree(
        repo,
        Some(previous),
        &BTreeMap::from([(item_path(value), Some(blob))]),
    )
    .unwrap();
    let mut parents = vec![repo.find_commit(previous).unwrap()];
    if let Some(other) = other {
        parents.push(repo.find_commit(other).unwrap());
    }
    let parents = parents.iter().collect::<Vec<_>>();
    let signature = git2::Signature::new(
        "Fixture",
        "fixture@bluink.invalid",
        &git2::Time::new(1_700_000_000 + i64::try_from(index).unwrap(), 0),
    )
    .unwrap();
    repo.commit(
        None,
        &signature,
        &signature,
        "History fixture",
        &repo.find_tree(tree).unwrap(),
        &parents,
    )
    .unwrap()
}

fn walk_page(repo: &Repository, head: Oid, path: &Path, named: Oid) -> Vec<String> {
    let mut walk = repo.revwalk().unwrap();
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .unwrap();
    walk.push(head).unwrap();
    walk.filter_map(|oid| {
        let oid = oid.unwrap();
        let commit = repo.find_commit(oid).unwrap();
        let current = commit
            .tree()
            .unwrap()
            .get_path(path)
            .ok()
            .map(|entry| entry.id());
        let previous = commit
            .parent(0)
            .ok()
            .and_then(|parent| parent.tree().ok())
            .and_then(|tree| tree.get_path(path).ok())
            .map(|entry| entry.id());
        (current != previous || oid == named).then(|| oid.to_string())
    })
    .take(40)
    .collect()
}

fn p95(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() * 95).div_ceil(100) - 1]
}

fn build_history(root: &Path, store: &Store) -> (Repository, Oid, Value, f64) {
    let mut target = seed(store);
    let mut other = target.clone();
    other["id"] = json!(Uuid::now_v7().to_string());
    checkpoints::initialize(root).unwrap();
    let mut head = crate::versioning::test_support::checkpoint(root, store, "fixture", "Fixture")
        .unwrap()
        .unwrap();
    let repo = checkpoints::open(root).unwrap();
    // Fixture construction only: durability is not part of history-query timing.
    repo.config()
        .unwrap()
        .set_bool("core.fsyncObjectFiles", false)
        .unwrap();
    let generated = Instant::now();
    for index in 1..=9990 {
        let branch = if index % 1000 == 0 {
            Some(commit(
                &repo,
                repo.find_commit(head).unwrap().parent_id(0).unwrap(),
                None,
                &mut target,
                index,
            ))
        } else {
            None
        };
        head = commit(
            &repo,
            head,
            branch,
            if index % 101 == 0 {
                &mut target
            } else {
                &mut other
            },
            index,
        );
    }
    repo.reference(repository::BRANCH, head, true, "Fixture construction")
        .unwrap();
    let project = store
        .read(|tx| {
            Ok(tx.query_row::<String, _, _>(
                "SELECT project_uuid FROM project_state",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    crate::versioning::history::write_named_tag(
        &repo,
        head,
        &NamedVersionRequest {
            request_id: Uuid::now_v7().to_string(),
            title: "Scoped version without a page edit".into(),
            item_id: Some(target["id"].as_str().unwrap().to_owned()),
        },
        &project,
    )
    .unwrap();
    let fixture_ms = generated.elapsed().as_secs_f64() * 1000.0;
    (repo, head, target, fixture_ms)
}

#[test]
#[ignore = "Explicit 10k Git history comparison; creates retained fixture when BLUINK_HISTORY_BENCHMARK_DIRECTORY is set"]
fn ten_thousand_commit_history_parity_and_latency() {
    let root = std::env::var("BLUINK_HISTORY_BENCHMARK_DIRECTORY").map_or_else(
        |_| tempfile::tempdir().unwrap(),
        |parent| {
            std::fs::create_dir_all(&parent).unwrap();
            tempfile::tempdir_in(parent).unwrap()
        },
    );
    let store = Store::create(root.path(), Uuid::now_v7()).unwrap();
    let (repo, head, target, fixture_ms) = build_history(root.path(), &store);
    let started = Instant::now();
    let projection = super::prepare(&repo, &store).unwrap().unwrap();
    let count = projection.commits.len() + 1;
    store
        .write(move |tx| {
            git::history::install(tx, &projection)?;
            tx.execute("UPDATE git_state SET expected_head=?1", [head.to_string()])?;
            Ok(())
        })
        .unwrap();
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(count, 10000);
    let request = HistoryRequest {
        item_id: Some(target["id"].as_str().unwrap().to_owned()),
        before: None,
        limit: Some(40),
    };
    let expected = walk_page(&repo, head, Path::new(&item_path(&target)), head);
    let mut indexed = Vec::new();
    let mut walked = Vec::new();
    for _ in 0..100 {
        let started = Instant::now();
        let page = super::read(root.path(), &store, &request).unwrap();
        indexed.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(
            page.entries
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            expected
        );
        let started = Instant::now();
        assert_eq!(
            walk_page(&repo, head, Path::new(&item_path(&target)), head),
            expected
        );
        walked.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let report = json!({"commits":count,"merges":9,"scopedTags":1,"samples":100,"fixtureMs":fixture_ms,"ingestMs":ingest_ms,"indexedP95Ms":p95(&mut indexed),"gitWalkP95Ms":p95(&mut walked),"indexedSamplesMs":indexed,"gitWalkSamplesMs":walked,"semanticParity":true,"profile":if cfg!(debug_assertions){"debug"}else{"optimized"},"scope":"Sparse page, 40 results, no attachments; historical body content is not loaded by SQL query"});
    std::fs::write(
        root.path().join("history-benchmark.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string(&report).unwrap());
    drop(repo);
    drop(store);
    if std::env::var_os("BLUINK_HISTORY_BENCHMARK_DIRECTORY").is_some() {
        println!("Retained fixture: {}", root.keep().display());
    }
}
