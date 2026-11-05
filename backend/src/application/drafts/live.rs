// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::merge::validation::Snapshot;
use crate::domain::project::{
    candidate::{Dependency, NamedKind},
    identity::ItemId,
};
use crate::store::{self, item};
use rusqlite::Transaction;
use serde_json::Value;
use std::collections::BTreeSet;

/// Reads and reverse dependencies come from one SQLite snapshot.
pub(super) struct Live<'a, 'db>(pub(super) &'a Transaction<'db>);

impl Snapshot for Live<'_, '_> {
    fn read(&self, id: &str) -> Result<Option<Value>, String> {
        ItemId::try_from(id)?;
        item::read(self.0, id, true).map_err(|error| error.to_string())
    }
    fn dependents(&self, dependency: &Dependency) -> Result<BTreeSet<String>, String> {
        self.owners(dependency).map_err(|error| error.to_string())
    }
}

impl Live<'_, '_> {
    fn owners(&self, dependency: &Dependency) -> store::Result<BTreeSet<String>> {
        let (sql, key) = match dependency {
            Dependency::Named {
                kind: NamedKind::Property,
                key,
            } => (
                "SELECT i.uuid FROM field_definition f JOIN field_presence p ON p.field_id=f.id JOIN item i ON i.id=p.item_id WHERE f.key=?1",
                key.as_str(),
            ),
            Dependency::Named { kind, key } => {
                let id = crate::domain::project::named_items::id(kind.as_str(), key);
                let result=self.0.prepare_cached("SELECT e.uuid FROM relation r JOIN entity t ON t.id=r.target_id JOIN entity e ON e.id=r.source_id WHERE t.uuid=?1 AND r.kind IN ('tag','label','snippet_citation','snippet_embed')")?.query_map([id],|row|row.get(0))?.collect::<rusqlite::Result<_>>()?;
                return Ok(result);
            }
            Dependency::Item(id) => (
                "SELECT e.uuid FROM relation r JOIN entity t ON t.id=r.target_id JOIN entity e ON e.id=r.source_id WHERE t.uuid=?1 AND r.kind IN ('pin','task_owner','publication_member')",
                id.as_str(),
            ),
        };
        Ok(self
            .0
            .prepare_cached(sql)?
            .query_map([key], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}
