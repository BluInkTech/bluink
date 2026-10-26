// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use crossbeam_channel::Receiver;
use tantivy::{Index, IndexReader, IndexWriter};

use crate::indexing::coordinator::{IndexCoordinator, IndexSnapshot};
use crate::indexing::index::{open_reader, open_writer};
use crate::indexing::state::Work;

mod events;
mod projections;
mod reconcile;

const COMMIT_BATCH_SIZE: usize = 512;

/// Readers already acquired by queries remain at their original generation.
fn publish_reader(coordinator: &IndexCoordinator, index: &Index) -> Result<IndexReader, String> {
    let reader = open_reader(index).map_err(|error| error.to_string())?;
    coordinator.publish(IndexSnapshot {
        index: index.clone(),
        reader: reader.clone(),
    })?;
    Ok(reader)
}

pub(super) fn acquire_writer<'a>(
    slot: &'a mut Option<IndexWriter>,
    index: &Index,
) -> Result<&'a mut IndexWriter, String> {
    if slot.is_none() {
        *slot = Some(open_writer(index).map_err(|error| error.to_string())?);
    }
    slot.as_mut()
        .ok_or_else(|| "Index writer was not acquired".to_owned())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "The worker thread owns its receiver and coordinator until shutdown."
)]
pub(crate) fn service_worker(
    root: PathBuf,
    index_path: PathBuf,
    receiver: Receiver<Work>,
    coordinator: Arc<IndexCoordinator>,
) {
    let result = reconcile::reconcile(&root, &index_path, &coordinator)
        .and_then(|ready| {
            events::EventWorker::new(root, index_path, Arc::clone(&coordinator), ready)
        })
        .and_then(|mut worker| worker.run(&receiver));
    if let Err(error) = result {
        tracing::error!(
            error_class = "index_reconciliation_failed",
            "index reconciliation failed"
        );
        let report = coordinator.update_status(|current| {
            "error".clone_into(&mut current.state);
            "Indexing failed".clone_into(&mut current.phase);
            current.error = Some(error);
        });
        if let Err(report_error) = report {
            tracing::error!(error = %report_error, "Unable to record index failure");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexing::index::build_schema;
    use crate::query::schema::QuerySchema;

    #[test]
    fn invalid_item_preserves_projection_and_pending_revision_but_deletion_removes_it() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(root.path(), "UTC").unwrap();
        let item = crate::engine::project::write::write_item_input(
            root.path(),
            serde_json::json!({
                "meta": {"type": "page", "title": "Keep the last valid projection"},
                "content": {"type": "doc", "content": [{"type": "paragraph"}]}
            }),
            None,
            "2026-10-20",
        )
        .unwrap()["item"]
            .clone();
        let id = item["id"].as_str().unwrap();
        let revision = item["revision"].as_str().unwrap();
        let path = crate::engine::project::identity::ItemId::try_from(id)
            .unwrap()
            .path(root.path());
        let index_path = root.path().join(".cache/read-error-test");
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), true));
        let ready = reconcile::reconcile(root.path(), &index_path, &coordinator).unwrap();
        let before = coordinator.query_snapshot().unwrap().unwrap();
        let mut worker = events::EventWorker::new(
            root.path().to_owned(),
            index_path,
            Arc::clone(&coordinator),
            ready,
        )
        .unwrap();
        std::fs::write(&path, b"{invalid canonical JSON").unwrap();
        let (sender, receiver) = crossbeam_channel::unbounded();
        coordinator
            .submit(&sender, id.to_owned(), Some("pending-revision".to_owned()))
            .unwrap();
        drop(sender);
        assert!(worker.run(&receiver).is_err());
        let failed = coordinator.query_snapshot().unwrap().unwrap();
        assert_eq!(failed.dirty.get(id).unwrap(), "pending-revision");
        assert_eq!(
            crate::indexing::index::indexed_revision(&failed.reader, &failed.index.schema(), id,)
                .unwrap()
                .as_deref(),
            Some(revision)
        );
        assert_eq!(
            failed.reader.searcher().num_docs(),
            before.reader.searcher().num_docs()
        );
        std::fs::remove_file(&path).unwrap();
        let (sender, receiver) = crossbeam_channel::unbounded();
        coordinator.submit(&sender, id.to_owned(), None).unwrap();
        drop(sender);
        worker.run(&receiver).unwrap();
        let deleted = coordinator.query_snapshot().unwrap().unwrap();
        assert!(deleted.dirty.is_empty());
        assert_eq!(
            deleted.reader.searcher().num_docs() + 1,
            before.reader.searcher().num_docs()
        );
        drop((worker, before, failed, deleted, coordinator));
        root.close().unwrap();
    }

    #[test]
    fn disconnected_work_channel_commits_accepted_work_and_joins() {
        let root = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(root.path(), "UTC").unwrap();
        let index_path = root.path().join(".cache/disconnect-test");
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), true));
        let ready = reconcile::reconcile(root.path(), &index_path, &coordinator).unwrap();
        let before = coordinator.query_snapshot().unwrap().unwrap();
        let item = crate::engine::project::write::write_item_input(
            root.path(),
            serde_json::json!({
                "meta": {"type": "page", "title": "Accepted before disconnect"},
                "content": {"type": "doc", "content": [{"type": "paragraph"}]}
            }),
            None,
            "2026-10-20",
        )
        .unwrap()["item"]
            .clone();
        let id = item["id"].as_str().unwrap();
        let revision = item["revision"].as_str().unwrap();
        let mut worker = events::EventWorker::new(
            root.path().to_owned(),
            index_path,
            Arc::clone(&coordinator),
            ready,
        )
        .unwrap();
        let (sender, receiver) = crossbeam_channel::unbounded();
        coordinator
            .submit(&sender, id.to_owned(), Some(revision.to_owned()))
            .unwrap();
        let published = coordinator.submit_mutation(&sender, &[]).unwrap();
        drop(sender);
        let (finished, completion) = crossbeam_channel::bounded(1);
        let thread = std::thread::spawn(move || {
            let result = worker.run(&receiver);
            drop(worker);
            finished.send(result).unwrap();
        });
        published
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let visible = coordinator.query_snapshot().unwrap().unwrap();
        assert_eq!(
            visible.reader.searcher().num_docs(),
            before.reader.searcher().num_docs() + 1
        );
        drop(visible);
        completion
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        thread.join().unwrap();
        let after = coordinator.query_snapshot().unwrap().unwrap();
        assert!(after.dirty.is_empty());
        assert_eq!(
            crate::indexing::index::indexed_revision(&after.reader, &after.index.schema(), id,)
                .unwrap()
                .as_deref(),
            Some(revision)
        );
        assert_eq!(
            after.reader.searcher().num_docs(),
            before.reader.searcher().num_docs() + 1
        );
        drop(before);
        drop(after);
        drop(coordinator);
        root.close().unwrap();
    }

    fn assert_indexed_revision(
        snapshot: &crate::indexing::coordinator::MainSnapshot,
        item: &serde_json::Value,
    ) {
        assert_eq!(
            crate::indexing::index::indexed_revision(
                &snapshot.reader,
                &snapshot.index.schema(),
                item["id"].as_str().unwrap()
            )
            .unwrap()
            .as_deref(),
            item["revision"].as_str()
        );
    }

    #[test]
    fn one_parent_event_publishes_the_entire_archived_task_group() {
        use crate::engine::project::{config, task, transaction, write};
        use serde_json::json;
        let root = tempfile::tempdir().unwrap();
        config::open_or_create_project(root.path(), "UTC").unwrap();
        let page = write::write_item_input(
            root.path(),
            json!({
                "meta":{"type":"page","title":"Parent"},
                "content":{"type":"doc","content":[{"type":"paragraph"}]}
            }),
            None,
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let id = page["id"].as_str().unwrap();
        let child = task::mutate_note_task(
            root.path(),
            id,
            &json!({
                "kind":"create","text":"Owned task"
            }),
            "2026-10-24",
        )
        .unwrap()["item"]
            .clone();
        let index_path = root.path().join(".cache/group-test");
        let coordinator = Arc::new(IndexCoordinator::new(QuerySchema::default(), true));
        let ready = reconcile::reconcile(root.path(), &index_path, &coordinator).unwrap();
        let before = coordinator.query_snapshot().unwrap().unwrap();
        let archived = write::mutate_existing_item(
            root.path(),
            id,
            page["revision"].as_str(),
            "2026-10-24",
            |_| Ok(json!({"id":id,"meta":{"state":"archived"}})),
        )
        .unwrap();
        assert_eq!(archived["relatedItems"].as_array().unwrap().len(), 1);
        assert_eq!(transaction::pending(root.path()).unwrap().len(), 2);
        let mut worker = events::EventWorker::new(
            root.path().to_owned(),
            index_path,
            Arc::clone(&coordinator),
            ready,
        )
        .unwrap();
        let (sender, receiver) = crossbeam_channel::unbounded();
        // A watcher may observe just the parent first. The durable group must
        // bring its child into the same commit rather than wait for another event.
        coordinator.submit(&sender, id.to_owned(), None).unwrap();
        drop(sender);
        worker.run(&receiver).unwrap();
        let after = coordinator.query_snapshot().unwrap().unwrap();
        for item in [&page, &child] {
            let item_id = item["id"].as_str().unwrap();
            assert_indexed_revision(&before, item);
            let current = crate::engine::project::item::read_project_item(root.path(), item_id)
                .unwrap()
                .unwrap();
            assert_eq!(current["meta"]["state"], "archived");
            assert_indexed_revision(&after, &current);
        }
        assert_eq!(
            transaction::pending(root.path()).unwrap(),
            Vec::<(String, String)>::new()
        );
        assert!(after.dirty.is_empty());
        drop((worker, before, after, coordinator));
        root.close().unwrap();
    }

    #[test]
    fn commit_and_compaction_do_not_reload_acquired_readers() {
        let main = Index::create_in_ram(build_schema());
        let coordinator = IndexCoordinator::new(QuerySchema::default(), false);
        let acquired = publish_reader(&coordinator, &main).unwrap();
        let mut slot = None;
        let writer = acquire_writer(&mut slot, &main).unwrap();
        let field = main.schema().get_field("id").unwrap();
        writer.add_document(tantivy::doc!(field => "note")).unwrap();
        writer.commit().unwrap();
        slot.take().unwrap().wait_merging_threads().unwrap();
        assert_eq!(acquired.searcher().num_docs(), 0);
        let published = publish_reader(&coordinator, &main).unwrap();
        assert_eq!(published.searcher().num_docs(), 1);
        assert_eq!(acquired.searcher().num_docs(), 0);
    }
}
