// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Runtime relationship checks for guarded removal. Bind the target outside
//! the relationship lock, then verify current indexed candidates and canonical
//! pending writes inside it. The engine owns policy and persistence.

use std::collections::HashSet;
use std::path::Path;

use tantivy::query::{BooleanQuery, Occur};
use tantivy::{
    collector::DocSetCollector,
    schema::{TantivyDocument, Value},
};

use crate::application::error::{Result, internal_error};
use crate::engine::project::item::read_project_item;
use crate::engine::project::usage::Usage;
use crate::indexing::service::project_usage_index;
use crate::relations::exact_term_query;

pub(crate) fn item_candidates(
    snapshot: &crate::indexing::coordinator::MainSnapshot,
    kind: &str,
) -> Result<Vec<String>> {
    let schema = snapshot.index.schema();
    let query = exact_term_query(schema.get_field("item_type").map_err(internal_error)?, kind);
    collect_candidates(snapshot, query.as_ref())
}

/// Select exact relationship candidates from one generation, including pending
/// canonical writes. External edits participate once observed by the watcher.
pub(crate) fn candidates(
    snapshot: &crate::indexing::coordinator::MainSnapshot,
    usage: Usage<'_>,
) -> Result<Vec<String>> {
    collect_candidates(snapshot, &reference_query(&snapshot.index.schema(), usage)?)
}

/// One relationship query is shared by removal checks, usage lists and counts.
pub(crate) fn reference_query(
    schema: &tantivy::schema::Schema,
    usage: Usage<'_>,
) -> Result<BooleanQuery> {
    let (fields, key): (&[&str], &str) = match usage {
        Usage::Tag(key) => (&["tags"], key),
        Usage::Label(key) => (&["labels"], key),
        Usage::Property(key) => (&["metadata_keys"], key),
        Usage::Snippet(key) => (&["snippet_citation_targets", "snippet_embed_targets"], key),
    };
    let references = BooleanQuery::union(
        fields
            .iter()
            .map(|field| {
                Ok(exact_term_query(
                    schema.get_field(field).map_err(internal_error)?,
                    key,
                ))
            })
            .collect::<Result<Vec<_>>>()?,
    );
    let mut clauses = vec![(
        Occur::Must,
        Box::new(references) as Box<dyn tantivy::query::Query>,
    )];
    if usage.active_owners_only() {
        clauses.push((
            Occur::Must,
            exact_term_query(schema.get_field("state").map_err(internal_error)?, "active"),
        ));
    }
    Ok(BooleanQuery::new(clauses))
}

fn collect_candidates(
    snapshot: &crate::indexing::coordinator::MainSnapshot,
    query: &dyn tantivy::query::Query,
) -> Result<Vec<String>> {
    let searcher = snapshot.reader.searcher();
    let mut ids: HashSet<String> = snapshot.dirty.keys().cloned().collect();
    ids.extend(crate::indexing::fast::matching_ids(&searcher, query).map_err(internal_error)?);
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort_unstable();
    Ok(ids)
}

/// A target-bound check. Capture the current generation only when checking
/// under the relationship lock, so writes after construction are included.
pub(crate) struct UsageCheck<'a> {
    root: &'a Path,
    usage: Usage<'a>,
}

impl<'a> UsageCheck<'a> {
    pub(crate) const fn new(root: &'a Path, usage: Usage<'a>) -> Self {
        Self { root, usage }
    }
    fn snapshot(&self) -> Result<Option<crate::indexing::coordinator::MainSnapshot>> {
        // Standalone snippet operations have always supported canonical discovery.
        // Declaration removal requires a complete runtime index.
        if matches!(self.usage, Usage::Snippet(_)) {
            crate::indexing::service::optional_project_usage_index(self.root)
        } else {
            project_usage_index(self.root).map(Some)
        }
    }

    fn candidates(&self) -> Result<Vec<String>> {
        let Some(snapshot) = self.snapshot()? else {
            return crate::engine::project::item::list_project_item_ids(self.root)
                .map_err(internal_error);
        };
        candidates(&snapshot, self.usage)
    }

    /// Stop at the first confirmed blocker, without counting every owner.
    pub(crate) fn used(&self) -> Result<bool> {
        let Some(snapshot) = self.snapshot()? else {
            return Ok(self.count_up_to(1)? != 0);
        };
        let mut seen = HashSet::new();
        for id in snapshot.dirty.keys() {
            seen.insert(id.clone());
            if self.confirmed(id)? {
                return Ok(true);
            }
        }
        let schema = snapshot.index.schema();
        let id_field = schema.get_field("id").map_err(internal_error)?;
        let query = reference_query(&schema, self.usage)?;
        let searcher = snapshot.reader.searcher();
        for address in searcher
            .search(&query, &DocSetCollector)
            .map_err(internal_error)?
        {
            let document: TantivyDocument = searcher.doc(address).map_err(internal_error)?;
            let id = document
                .get_first(id_field)
                .and_then(|value| value.as_str())
                .ok_or_else(|| internal_error("Indexed candidate has no ID"))?;
            if seen.insert(id.to_owned()) && self.confirmed(id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Snippet archive errors include the number of blocking pages.
    pub(crate) fn count(&self) -> Result<usize> {
        self.count_up_to(usize::MAX)
    }

    fn count_up_to(&self, limit: usize) -> Result<usize> {
        let mut count = 0;
        for id in self.candidates()? {
            if self.confirmed(&id)? {
                count += 1;
                if count == limit {
                    break;
                }
            }
        }
        Ok(count)
    }

    fn confirmed(&self, id: &str) -> Result<bool> {
        Ok(read_project_item(self.root, id)?
            .as_ref()
            .is_some_and(|item| self.usage.matches(item)))
    }
}

#[cfg(test)]
fn used(root: &Path, usage: Usage<'_>) -> Result<bool> {
    UsageCheck::new(root, usage).used()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::project::{
        config::open_or_create_project, facets, item_mutations, properties, write,
    };
    use crate::indexing::service;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn ready(index: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = service::index_service_status_typed(index).unwrap();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.state == "ready" && status.pending == 0 {
                return;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn goal_candidates_skip_unrelated_files_and_include_unqueued_goals() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        open_or_create_project(&root, "UTC").unwrap();
        let note = write::write_item_input(&root, json!({"meta":{"type":"page","title":"Unrelated"},"content":{"type":"doc","content":[{"type":"paragraph","content":[]}]}}), None, "2026-10-21").unwrap();
        let index = temporary.path().join("index").to_str().unwrap().to_owned();
        service::start_index_service_typed(root.to_str().unwrap().into(), &index).unwrap();
        ready(&index);
        let path = crate::engine::project::identity::ItemId::try_from(
            note["item"]["id"].as_str().unwrap(),
        )
        .unwrap()
        .path(&root);
        std::fs::write(path, b"unrelated invalid json").unwrap();
        let request = json!({"scope":"all_notes","startDate":"2026-10-21","endDate":"2026-10-30","dailyWordTarget":100});
        // Direct canonical writes notify the overlay but do not enqueue indexing.
        crate::engine::project::goal::create_writing_goal(
            &root,
            &request,
            "2026-10-21",
            |_| Ok(()),
        )
        .unwrap();
        let error = crate::engine::project::goal::create_writing_goal(
            &root,
            &request,
            "2026-10-21",
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("already has a writing goal"));
        service::shutdown_index_service(index).unwrap();
        temporary.close().unwrap();
    }

    #[test]
    fn prepared_checks_capture_later_writes_and_preserve_owner_policies() {
        use crate::engine::project::snippets;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        open_or_create_project(&root, "UTC").unwrap();
        snippets::write_snippet(
            &root,
            "shared",
            json!({"type":"paragraph","content":[]}),
            None,
        )
        .unwrap();
        properties::create_property(&root, json!({"key":"flag","valueType":"checkbox"})).unwrap();
        let index = temporary.path().join("index").to_str().unwrap().to_owned();
        service::start_index_service_typed(root.to_str().unwrap().into(), &index).unwrap();
        ready(&index);
        let checks = [
            Usage::Tag("shared"),
            Usage::Label("shared"),
            Usage::Property("flag"),
            Usage::Snippet("shared"),
        ]
        .map(|usage| UsageCheck::new(&root, usage));
        for check in &checks {
            assert!(!check.used().unwrap());
        }
        // These writes happen AFTER preparation, without notifying the worker.
        // Reusing the prepared check must capture the current dirty overlay.
        let input = json!({"meta":{"type":"page","title":"Owner","flag":false},"tags":["shared"],
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["shared"]},"content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}]}]}});
        let first = write::write_item_input(&root, input.clone(), None, "2026-10-21").unwrap();
        let second = write::write_item_input(&root, input, None, "2026-10-21").unwrap();
        for check in &checks {
            assert_eq!(check.count().unwrap(), 2);
        }
        for owner in [&first, &second] {
            item_mutations::archive_item(
                &root,
                owner["item"]["id"].as_str().unwrap(),
                "2026-10-21",
            )
            .unwrap();
        }
        for check in &checks[..3] {
            assert_eq!(check.count().unwrap(), 0);
        }
        assert_eq!(
            checks[3].count().unwrap(),
            2,
            "Archived snippet owners still block removal; occurrences count once per owner"
        );
        service::shutdown_index_service(index).unwrap();
        temporary.close().unwrap();
    }

    #[test]
    fn exact_usage_overlays_unqueued_writes_and_archives_without_scanning_unrelated_files() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        open_or_create_project(&root, "UTC").unwrap();
        for (key, value_type) in [
            ("flag", "checkbox"),
            ("amount", "number"),
            ("values", "multiSelect"),
        ] {
            properties::create_property(&root, json!({"key":key,"valueType":value_type})).unwrap();
        }
        let input = json!({
            "meta":{"type":"page", "title":"Usage", "flag":false,"amount":0,"values":[]},
            "tags":["parent/child"],
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["evidence"]},"content":[{"type":"text","text":"Example"}]}]}
        });
        let page = write::write_item_input(&root, input.clone(), None, "2026-10-21").unwrap();
        let id = page["item"]["id"].as_str().unwrap();
        let index = temporary.path().join("index").to_str().unwrap().to_owned();
        service::start_index_service_typed(root.to_str().unwrap().into(), &index).unwrap();
        ready(&index);
        let checks = [
            Usage::Tag("parent/child"),
            Usage::Label("evidence"),
            Usage::Property("flag"),
            Usage::Property("amount"),
            Usage::Property("values"),
        ];
        for check in checks {
            assert!(used(&root, check).unwrap());
        }
        // Exact tag membership must not treat descendant tags as use of a parent.
        assert!(!used(&root, Usage::Tag("parent")).unwrap());
        let archived = item_mutations::archive_item(&root, id, "2026-10-21").unwrap();
        // No application notification or watcher: the old index still contains it.
        for check in checks {
            assert!(!used(&root, check).unwrap());
        }
        let pending = write::write_item_input(&root, input, None, "2026-10-21").unwrap();
        for check in checks {
            assert!(used(&root, check).unwrap());
        }
        service::mark_project_write(&root, id, archived["item"]["revision"].as_str().unwrap())
            .unwrap();
        service::mark_project_write(
            &root,
            pending["item"]["id"].as_str().unwrap(),
            pending["item"]["revision"].as_str().unwrap(),
        )
        .unwrap();
        ready(&index);
        // Malformed unrelated canonical data would make the previous full scan
        // fail. All unused removals must succeed without opening these files.
        let unrelated = crate::engine::project::identity::ItemId::try_from(id)
            .unwrap()
            .directory(&root)
            .join("item.json");
        std::fs::write(unrelated, b"not json").unwrap();
        for kind in ["tag", "label"] {
            facets::create_facet(&root, kind, "unused").unwrap();
            facets::remove_facet(&root, kind, "unused", || {
                used(&root, Usage::facet(kind, "unused").unwrap())
                    .map_err(|e| write::MutationError::Internal(e.to_string()))
            })
            .unwrap();
        }
        properties::create_property(&root, json!({"key":"unused","valueType":"text"})).unwrap();
        properties::remove_property(&root, "unused", || {
            used(&root, Usage::Property("unused"))
                .map_err(|e| write::MutationError::Internal(e.to_string()))
        })
        .unwrap();
        service::shutdown_index_service(index).unwrap();
        temporary.close().unwrap();
    }
}

#[cfg(test)]
mod scale_test {
    use super::*;
    use crate::engine::project::{config, facets, identity::ItemId, properties, write};
    use crate::indexing::service;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn measure_services(root: &Path, index: &str) {
        let started = Instant::now();
        let tasks = crate::tasks::query_collection(
            index,
            Some(crate::query::document::QueryDocument::active_pages()),
            None,
            "open".into(),
            50,
            0,
            None,
        )
        .unwrap();
        assert_eq!(tasks.count, 0);
        println!("Empty task collection: {:?}", started.elapsed());
        let snippet = crate::engine::project::snippets::write_snippet(
            root,
            "benchmark",
            json!({"type":"paragraph","content":[{"type":"text","text":"Benchmark"}]}),
            None,
        )
        .unwrap();
        let started = Instant::now();
        crate::engine::project::snippets::archive_snippet(
            root,
            "benchmark",
            snippet["revision"].as_str().unwrap(),
        )
        .unwrap();
        println!("Indexed snippet archive: {:?}", started.elapsed());
        let started = Instant::now();
        let goal = crate::engine::project::goal::create_writing_goal(root, &json!({"scope":"all_notes","startDate":"2026-10-21","endDate":"2026-10-30","dailyWordTarget":100}), "2026-10-21", |_| Ok(())).unwrap();
        println!("Create writing goal: {:?}", started.elapsed());
        let started = Instant::now();
        config::read_workspace_context(root).unwrap();
        println!("Workspace context: {:?}", started.elapsed());
        let started = Instant::now();
        crate::engine::project::item_mutations::archive_item(
            root,
            goal["item"]["id"].as_str().unwrap(),
            "2026-10-21",
        )
        .unwrap();
        println!("Archive item: {:?}", started.elapsed());
    }

    #[test]
    #[ignore = "Explicit 50,500-note filesystem and index benchmark"]
    fn unused_removal_with_50500_notes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        config::open_or_create_project(&root, "UTC").unwrap();
        let template = write::write_item_input(&root, json!({"meta":{"type":"page","title":"Scale fixture"},"content":{"type":"doc","content":[{"type":"paragraph","content":[]}]}}),None,"2026-10-21").unwrap();
        let first_id = template["item"]["id"].as_str().unwrap().to_owned();
        for _ in 1..50_500 {
            let mut item = template["item"].clone();
            let id = uuid::Uuid::new_v4().to_string();
            item["id"] = json!(id);
            let path = ItemId::try_from(id.as_str()).unwrap().path(&root);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, serde_json::to_vec(&item).unwrap()).unwrap();
        }
        let index = temporary.path().join("index").to_str().unwrap().to_owned();
        service::start_index_service_typed(root.to_str().unwrap().into(), &index).unwrap();
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            let status = service::index_service_status_typed(&index).unwrap();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.state == "ready" && status.pending == 0 {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        }
        let (stop, stopped) = std::sync::mpsc::channel();
        let reader_root = root.clone();
        let reader = std::thread::spawn(move || {
            let mut maximum = Duration::ZERO;
            let mut reads = 0_u64;
            while stopped.try_recv().is_err() {
                let started = Instant::now();
                assert!(
                    read_project_item(&reader_root, &first_id)
                        .unwrap()
                        .is_some()
                );
                maximum = maximum.max(started.elapsed());
                reads += 1;
            }
            (reads, maximum)
        });
        for kind in ["tag", "label"] {
            facets::create_facet(&root, kind, "unused").unwrap();
            let started = Instant::now();
            facets::remove_facet(&root, kind, "unused", || {
                used(&root, Usage::facet(kind, "unused").unwrap())
                    .map_err(|error| write::MutationError::Internal(error.to_string()))
            })
            .unwrap();
            println!("{kind} removal: {:?}", started.elapsed());
        }
        properties::create_property(&root, json!({"key":"unused","valueType":"text"})).unwrap();
        let started = Instant::now();
        properties::remove_property(&root, "unused", || {
            used(&root, Usage::Property("unused"))
                .map_err(|error| write::MutationError::Internal(error.to_string()))
        })
        .unwrap();
        println!("property removal: {:?}", started.elapsed());
        measure_services(&root, &index);
        stop.send(()).unwrap();
        let (reads, maximum) = reader.join().unwrap();
        println!("Concurrent canonical reads: {reads}; maximum {maximum:?}");
        service::shutdown_index_service(index).unwrap();
        temporary.close().unwrap();
    }
}
