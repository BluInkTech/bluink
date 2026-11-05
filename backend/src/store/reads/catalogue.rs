// SPDX-License-Identifier: MPL-2.0
use crate::catalogue::{FacetEntry, MetadataField};
use crate::domain::project::types::FacetKind;
use crate::store::{Result, Store, error, fields, query};
use rusqlite::Transaction;
use std::sync::Arc;

pub(crate) fn metadata_fields(tx: &Transaction<'_>) -> Result<Vec<MetadataField>> {
    Ok(fields::schema(tx)?.metadata().to_vec())
}

pub(crate) fn list_facets(
    tx: &Transaction<'_>,
    kind: Option<FacetKind>,
) -> Result<Vec<FacetEntry>> {
    let mut result = Vec::new();
    let kinds = match kind {
        Some(ref kind) => std::slice::from_ref(kind),
        None => &[FacetKind::Label, FacetKind::Tag],
    };
    for facet in kinds {
        result.extend(query::page_catalogue(tx, *facet)?);
    }
    Ok(result)
}

#[derive(Default)]
pub(in crate::store) struct FacetCache {
    // Only complete catalogues can be cached: labels, tags, or both.
    entries: [Option<Snapshot>; 3],
}

struct Snapshot {
    version: (String, i64),
    rows: Arc<[FacetEntry]>,
}

impl Store {
    pub(crate) fn list_facets(&self, kind: Option<FacetKind>) -> Result<Vec<FacetEntry>> {
        self.read(|tx| self.facet_snapshot(tx, kind))
    }

    fn facet_snapshot(
        &self,
        tx: &Transaction<'_>,
        kind: Option<FacetKind>,
    ) -> Result<Vec<FacetEntry>> {
        let version: (String, i64) = tx.query_row(
            "SELECT change_epoch,generation FROM project_state WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let slot = match kind {
            None => 0,
            Some(FacetKind::Label) => 1,
            Some(FacetKind::Tag) => 2,
        };
        let cached = self.facet_cache.lock().map_err(error::poisoned)?.entries[slot]
            .as_ref()
            .filter(|entry| entry.version == version)
            .map(|entry| Arc::clone(&entry.rows));
        if let Some(rows) = cached {
            return Ok(rows.to_vec());
        }
        let rows = list_facets(tx, kind)?;
        let snapshot = Snapshot {
            version,
            rows: Arc::from(rows.as_slice()),
        };
        let mut cache = self.facet_cache.lock().map_err(error::poisoned)?;
        // A reader with an older WAL snapshot can finish after a newer reader.
        // It still answers its own snapshot, without replacing the newer entry.
        if !cache.entries[slot]
            .as_ref()
            .is_some_and(|s| s.version.0 == snapshot.version.0 && s.version.1 > snapshot.version.1)
        {
            cache.entries[slot] = Some(snapshot);
        }
        drop(cache);
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::commands::assemble_item;
    use crate::store::{
        changes, item,
        mutations::{self, PreparedChange},
    };
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn catalogue_cache_follows_database_snapshots_across_process_connections() {
        let root = tempfile::tempdir().unwrap();
        let reader = Store::create(root.path(), Uuid::now_v7()).unwrap();
        // Separate pools and writer threads have no shared in-memory notifications.
        let writer = Store::open(root.path()).unwrap();
        let kind = Some(FacetKind::Tag);
        let value=assemble_item(&json!({"meta":{"type":"page","title":"Source"},"tags":["history"],"content":{"document":{"type":"doc","content":[{"type":"paragraph","attrs":{"id":"p","labels":["quote"]}}]}}}),None,"UTC").unwrap();
        let id = value["id"].as_str().unwrap().to_owned();
        let create = PreparedChange::new(value, None).unwrap();
        assert_eq!(reader.list_facets(kind).unwrap().len(), 0);
        reader
            .read(|old| {
                assert_eq!(reader.facet_snapshot(old, kind)?.len(), 0);
                writer.write(move |tx| mutations::apply(tx, &[create], &[], None))?;
                assert_eq!(reader.list_facets(kind)?[0].usage_count, 1);
                assert_eq!(reader.facet_snapshot(old, kind)?.len(), 0);
                Ok(())
            })
            .unwrap();
        assert_eq!(reader.list_facets(None).unwrap().len(), 2);
        assert_eq!(
            reader.list_facets(Some(FacetKind::Label)).unwrap()[0].key,
            "quote"
        );
        let cached = || {
            Arc::clone(
                &reader.facet_cache.lock().unwrap().entries[2]
                    .as_ref()
                    .unwrap()
                    .rows,
            )
        };
        let before = cached();
        writer.write(|_| Ok(())).unwrap();
        assert_eq!(reader.list_facets(kind).unwrap()[0].key, "history");
        assert!(Arc::ptr_eq(&before, &cached()));
        writer
            .write(move |tx| {
                let header = item::read_header(tx, &id)?.unwrap();
                mutations::metadata(
                    tx,
                    &id,
                    header.value["revision"].as_str().unwrap(),
                    json!({"state":"archived"}).as_object().unwrap(),
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(reader.list_facets(kind).unwrap().len(), 0);
        let archived = cached();
        writer.write(changes::reset_after_rebuild).unwrap();
        assert_eq!(reader.list_facets(kind).unwrap().len(), 0);
        assert!(!Arc::ptr_eq(&archived, &cached()));
        let same_generation = cached();
        writer
            .write(|tx| {
                tx.execute(
                    "UPDATE project_state SET change_epoch=?1",
                    [Uuid::now_v7().to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(reader.list_facets(kind).unwrap().len(), 0);
        assert!(!Arc::ptr_eq(&same_generation, &cached()));
    }
}
