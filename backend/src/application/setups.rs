// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Workspace setup admission and atomic installation.
use super::error::{Result, internal_message, invalid_request};
use crate::domain::project::setup;
use crate::store::{Error, Store};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn create_workspace_from_setup(
    store: &Store,
    input: &Value,
    title: &str,
) -> Result<Value> {
    prepare(store, input, title)?.commit(store)
}

pub(super) struct PreparedSetup {
    plan: setup::SetupPlan,
    changes: Vec<crate::store::mutations::PreparedChange>,
    expected: BTreeMap<String, Option<String>>,
    timezone: String,
}

pub(super) fn prepare(store: &Store, input: &Value, title: &str) -> Result<PreparedSetup> {
    let setup = setup::parse_setup(input, title).map_err(invalid_request)?;
    let ids = setup.required_records().map_err(invalid_request)?;
    let (config, records) = store.read(|tx| {
        let records = ids
            .iter()
            .map(|id| Ok((id.clone(), crate::store::item::read(tx, id, true)?)))
            .collect::<crate::store::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(id, value)| value.map(|value| (id, value)))
            .collect::<BTreeMap<_, _>>();
        Ok((crate::store::project::read_config(tx)?, records))
    })?;
    let timezone = config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| internal_message("Project has no timezone"))?;
    let plan = crate::store::commands::with_authoring_values(|values| {
        setup::prepare_workspace_from_setup(&setup, title, &records, timezone, values)
    })
    .map_err(invalid_request)?;
    let changes = plan
        .related
        .iter()
        .chain(std::iter::once(&plan.workspace))
        .map(|value| {
            let id = value["id"]
                .as_str()
                .ok_or_else(|| Error::Invalid("Setup item has no ID".into()))?;
            let expected = records
                .get(id)
                .and_then(|record| record["revision"].as_str())
                .map(str::to_owned);
            crate::store::mutations::PreparedChange::new(value.clone(), expected)
        })
        .collect::<crate::store::Result<Vec<_>>>()?;

    let expected = ids
        .into_iter()
        .map(|id| {
            let revision = records
                .get(&id)
                .and_then(|value| value["revision"].as_str())
                .map(str::to_owned);
            (id, revision)
        })
        .collect();
    Ok(PreparedSetup {
        plan,
        changes,
        expected,
        timezone: timezone.to_owned(),
    })
}

impl PreparedSetup {
    pub(super) fn commit(self, store: &Store) -> Result<Value> {
        store.write(move |tx| {
            if crate::store::project::read_config(tx)?["analytics"]["timezone"].as_str()
                != Some(self.timezone.as_str())
            {
                return Err(Error::RevisionConflict);
            }
            // Reused declarations and snippets must still be the captured facts.
            for (id, expected) in self.expected {
                let current = crate::store::item::read_header(tx, &id)?;
                if current
                    .as_ref()
                    .and_then(|header| header.value["revision"].as_str())
                    != expected.as_deref()
                {
                    return Err(Error::RevisionConflict);
                }
            }
            crate::store::mutations::apply(tx, &self.changes, &[], None)
        })?;
        Ok(json!({"item":self.plan.workspace,"relatedItems":self.plan.related}))
    }
}
