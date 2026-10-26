// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Capture one index generation, then overlay changed canonical files. Neither
//! pending writes nor stale indexed owners may hide relationships.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::engine::document::projection::project_document;
use crate::engine::project::item::read_project_item;
use crate::indexing::fast::Strings;
use serde_json::Value as JsonValue;
use tantivy::collector::DocSetCollector;

/// Read-only theme catalogues need Publication candidates, not every page's
/// document. Include pending writes and hydrate canonical values at the caller.
/// Without a running index, the engine retains its ordinary canonical walk.
pub(crate) fn publication_candidates(root: &Path) -> Result<Option<Vec<String>>, String> {
    let Some(snapshot) =
        crate::indexing::service::project_index(root).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    crate::catalogue_usage::item_candidates(&snapshot, "publication")
        .map(Some)
        .map_err(|error| error.to_string())
}
/// All requested counts share one index generation and one read of each changed
/// canonical page. A page referring to the same snippet twice still counts once.
pub(crate) fn snippet_usage_counts(
    root: &Path,
    ids: &[&str],
) -> Result<HashMap<String, usize>, String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for id in ids {
        crate::engine::project::snippets::validate_snippet_id(id)?;
        counts.insert((*id).into(), 0);
    }
    if counts.is_empty() {
        return Ok(counts);
    }
    let snapshot =
        crate::indexing::service::project_index(root).map_err(|error| error.to_string())?;
    let mut changed = HashSet::new();
    if let Some(snapshot) = snapshot {
        changed.extend(snapshot.dirty.keys().cloned());
        let queries = ids
            .iter()
            .map(|id| {
                crate::catalogue_usage::reference_query(
                    &snapshot.index.schema(),
                    crate::engine::project::usage::Usage::Snippet(id),
                )
                .map(|query| Box::new(query) as Box<dyn tantivy::query::Query>)
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let query = tantivy::query::BooleanQuery::union(queries);
        let searcher = snapshot.reader.searcher();
        let mut hits: Vec<_> = searcher
            .search(&query, &DocSetCollector)
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect();
        hits.sort_unstable();
        for group in hits.chunk_by(|a, b| a.segment_ord == b.segment_ord) {
            let segment = searcher.segment_reader(group[0].segment_ord);
            let read =
                |name| Strings::open(segment, name)?.rows(group.iter().map(|hit| hit.doc_id));
            let owners = read("id")?;
            let stamps = read("index_stamp")?;
            let kinds = read("item_type")?;
            let citations = read("snippet_citation_targets")?;
            let embeds = read("snippet_embed_targets")?;
            for row in 0..group.len() {
                let (Some(id), Some(stamp), Some(kind)) =
                    (owners[row].first(), stamps[row].first(), kinds[row].first())
                else {
                    continue;
                };
                if kind != "page" {
                    continue;
                }
                let identity = crate::engine::project::identity::ItemId::try_from(id.as_str())?;
                let Some(current) = crate::projection::FileStamp::read(&identity.path(root))?
                else {
                    continue;
                };
                if snapshot.dirty.contains_key(id)
                    || crate::projection::FileStamp::parse_index(stamp) != Some(current)
                {
                    changed.insert(id.clone());
                    continue;
                }
                let targets: HashSet<_> = citations[row].iter().chain(&embeds[row]).collect();
                for target in targets {
                    if let Some(count) = counts.get_mut(target) {
                        *count += 1;
                    }
                }
            }
        }
    } else {
        changed.extend(crate::engine::project::item::list_project_item_ids(root)?);
    }
    for id in changed {
        let Some(item) = read_project_item(root, &id).map_err(|error| error.to_string())? else {
            continue;
        };
        if item["meta"]["type"] != "page" {
            continue;
        }
        let projection = project_document(crate::engine::publication::source::document(&item));
        let targets: HashSet<_> = projection
            .snippet_citations
            .into_iter()
            .chain(projection.snippet_embeds)
            .collect();
        for target in targets {
            if let Some(count) = counts.get_mut(&target) {
                *count += 1;
            }
        }
    }
    Ok(counts)
}

pub(crate) fn snippet_usages(root: &Path, id: &str) -> Result<Vec<JsonValue>, String> {
    crate::engine::project::snippets::validate_snippet_id(id)?;
    let snapshot =
        crate::indexing::service::project_index(root).map_err(|error| error.to_string())?;
    let Some(snapshot) = snapshot else {
        return crate::engine::project::snippets::snippet_usages(root, id);
    };
    let ids = crate::catalogue_usage::candidates(
        &snapshot,
        crate::engine::project::usage::Usage::Snippet(id),
    )
    .map_err(|error| error.to_string())?;
    // Read only matching/pending pages. Canonical content corrects removed
    // references, changed titles and files removed before the watcher catches up.
    let mut existing = Vec::new();
    for candidate in ids {
        let identity = crate::engine::project::identity::ItemId::try_from(candidate.as_str())?;
        if crate::projection::FileStamp::read(&identity.path(root))?.is_some() {
            existing.push(candidate);
        }
    }
    existing.sort();
    crate::engine::project::snippets::snippet_usages_in_pages(root, id, &existing)
}

#[cfg(test)]
mod tests {
    use crate::engine::project::{config::open_or_create_project, write::write_item_input};
    use crate::engine::publication::theme_store::{list, read};
    use serde_json::json;
    use std::fs;
    #[test]
    fn indexed_catalogue_reads_only_publications_and_pending_canonical_items() {
        struct Stop(String);
        impl Drop for Stop {
            fn drop(&mut self) {
                crate::indexing::service::shutdown_index_service(self.0.clone()).unwrap();
            }
        }
        let root = tempfile::tempdir().unwrap();
        open_or_create_project(root.path(), "UTC").unwrap();
        let page = write_item_input(
            root.path(),
            json!({"meta":{"type":"page","title":"Unrelated prose"},"content":{"type":"doc","content":[{"type":"paragraph"}]}}),
            None,
            "2026-10-17",
        )
        .unwrap();
        let mut publication: serde_json::Value = serde_json::from_str(include_str!("../../test-fixtures/pdf-project/items/01/01a0e7f5-d342-7f23-bd3d-6e27064a16cd/item.json")).unwrap();
        let saved = write_item_input(
            root.path(),
            json!({"id":publication["id"],"meta":publication["meta"],"content":publication["content"]}),
            None,
            "2026-10-17",
        )
        .unwrap();
        let path = root
            .path()
            .join(".cache/catalogue-test")
            .to_str()
            .unwrap()
            .to_owned();
        crate::indexing::service::start_index_service_typed(
            root.path().to_str().unwrap().to_owned(),
            &path,
        )
        .unwrap();
        let stop = Stop(path.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = crate::indexing::service::index_service_status_typed(&path).unwrap();
            assert_ne!(status.state, "error", "{:?}", status.error);
            if status.state == "ready" && status.pending == 0 {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // If catalogue reads scan prose, this malformed unrelated page fails the
        // request. No watcher runs in this fixture: the captured index is stable.
        fs::write(
            crate::engine::project::identity::ItemId::try_from(
                page["item"]["id"].as_str().unwrap(),
            )
            .unwrap()
            .path(root.path()),
            b"unrelated prose must not be read",
        )
        .unwrap();
        let original_id = saved["item"]["id"].as_str().unwrap();
        publication = saved["item"].clone();
        let new_id = uuid::Uuid::now_v7().to_string();
        publication["id"] = json!(new_id);
        publication["meta"]["title"] = json!("Pending archived Publication");
        publication["meta"]["state"] = json!("archived");
        let pending_path = crate::engine::project::identity::ItemId::try_from(new_id.as_str())
            .unwrap()
            .path(root.path());
        fs::create_dir_all(pending_path.parent().unwrap()).unwrap();
        fs::write(pending_path, serde_json::to_vec(&publication).unwrap()).unwrap();
        crate::indexing::service::mark_pending_write(
            root.path(),
            new_id.clone(),
            publication["revision"].as_str().unwrap().to_owned(),
        )
        .unwrap();
        let themes = list(
            root.path(),
            crate::document_graph::publication_candidates(root.path()).unwrap(),
        )
        .unwrap();
        let classic = themes.iter().find(|theme| theme.id == "classic").unwrap();
        assert_eq!(classic.usages.len(), 2);
        assert!(classic.usages.iter().any(|usage| usage.id == original_id));
        assert!(classic.usages.iter().any(|usage| usage.id == new_id
            && usage.archived
            && usage.title == "Pending archived Publication"));
        assert_eq!(
            read(
                root.path(),
                "classic",
                crate::document_graph::publication_candidates(root.path()).unwrap()
            )
            .unwrap()
            .summary
            .usages
            .len(),
            2
        );
        drop(stop);
        root.close().unwrap();
    }
}
