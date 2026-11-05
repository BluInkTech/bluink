// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Reverse references over immutable records. The reader owns the captured
//! bytes (for example Git blobs); this adapter retains only identities.
use super::validation::Snapshot;
use crate::domain::project::{
    candidate::{Dependency, dependencies},
    item,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Captured<F> {
    read: F,
    incoming: BTreeMap<Dependency, BTreeSet<String>>,
}

impl<F: Fn(&str) -> Result<Option<Value>, String>> Captured<F> {
    pub(crate) fn new(ids: impl IntoIterator<Item = String>, read: F) -> Result<Self, String> {
        let mut incoming: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                return Err("Duplicate captured item identity".into());
            }
            let value = read(&id)?.ok_or("Captured item disappeared")?;
            item::validate_item(&value, &id)?;
            for dependency in dependencies(&value)? {
                incoming.entry(dependency).or_default().insert(id.clone());
            }
        }
        Ok(Self { read, incoming })
    }
}

impl<F: Fn(&str) -> Result<Option<Value>, String>> Snapshot for Captured<F> {
    fn read(&self, id: &str) -> Result<Option<Value>, String> {
        (self.read)(id)
    }
    fn dependents(&self, dependency: &Dependency) -> Result<BTreeSet<String>, String> {
        Ok(self.incoming.get(dependency).cloned().unwrap_or_default())
    }
}
