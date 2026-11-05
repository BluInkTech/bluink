// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! A validated portable delta joins canonical writes and ref recovery in one transaction.
use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;
use uuid::Uuid;

use crate::store::changes::ChangeDomain;
use crate::store::{Error, Result, git, mutations, project};
use crate::versioning::checkpoints::{self, VerifiedAsset};

pub(crate) struct PreparedDelta {
    items: Vec<mutations::PreparedChange>,
    deletes: Vec<mutations::Delete>,
    records: Vec<(String, Option<Value>)>,
    assets: Vec<(String, Option<VerifiedAsset>)>,
}

impl PreparedDelta {
    pub(crate) fn new(
        items: Vec<(Value, Option<String>)>,
        deletes: Vec<mutations::Delete>,
        records: Vec<(String, Option<Value>)>,
        assets: Vec<(String, Option<VerifiedAsset>)>,
    ) -> Result<Self> {
        let mut paths = BTreeSet::new();
        for (path, value) in &records {
            checkpoints::validate_record_path(path)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            if path == "bluink.json" && value.is_none() {
                return Err(Error::Invalid(
                    "Project settings cannot be deleted.".to_owned(),
                ));
            }
            if !paths.insert(path) {
                return Err(Error::Invalid(
                    "Portable path occurs twice in a change.".to_owned(),
                ));
            }
        }
        for (path, _) in &assets {
            checkpoints::validate_asset_path(path)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            if !paths.insert(path) {
                return Err(Error::Invalid(
                    "Portable path occurs twice in a change.".to_owned(),
                ));
            }
        }
        let items = items
            .into_iter()
            .map(|(value, expected)| mutations::PreparedChange::new(value, expected))
            .collect::<Result<_>>()?;
        Ok(Self {
            items,
            deletes,
            records,
            assets,
        })
    }

    /// Caller prepares immutable objects outside the writer and records the ref
    /// operation in this same transaction. Generation prevents stale tree plans.
    pub(crate) fn apply(&self, tx: &Transaction<'_>, expected_generation: i64) -> Result<i64> {
        let current: i64 = tx.query_row(
            "SELECT generation FROM project_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if current != expected_generation {
            return Err(Error::RevisionConflict);
        }
        let next = current
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Project generation overflow.".to_owned()))?;
        for (path, value) in &self.records {
            if let Some(value) = value {
                git::put_record(tx, path, value, &Uuid::now_v7().to_string(), next)?;
            } else {
                tx.execute("DELETE FROM project_record WHERE path=?1", [path])?;
                git::mark_deleted(tx, path, next)?;
            }
        }
        let mut domains = BTreeSet::from([ChangeDomain::History]);
        if !self.records.is_empty() || !self.assets.is_empty() {
            domains.insert(ChangeDomain::Settings);
        }
        let generation =
            mutations::apply_with_domains(tx, &self.items, &self.deletes, None, &domains)?;
        for (path, asset) in &self.assets {
            if let Some(asset) = asset {
                let owner = if path.starts_with("assets/") {
                    let uuid = path
                        .split('/')
                        .nth(2)
                        .ok_or_else(|| Error::Invalid("Attachment has no owner.".to_owned()))?;
                    Some(
                        tx.query_row("SELECT id FROM item WHERE uuid=?1", [uuid], |row| {
                            row.get::<_, i64>(0)
                        })
                        .optional()?
                        .ok_or_else(|| {
                            Error::Invalid("Attachment owner does not exist.".to_owned())
                        })?,
                    )
                } else {
                    None
                };
                git::put_asset(tx, path, owner, asset, generation)?;
            } else {
                tx.execute("DELETE FROM asset_path WHERE path=?1", [path])?;
                git::mark_deleted(tx, path, generation)?;
            }
        }
        let config = project::read_config(tx)?;
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM item WHERE uuid=?1 AND type='workspace' AND state='active')", [config["defaultWorkspaceId"].as_str()], |row| row.get(0))?;
        if !valid {
            return Err(Error::Invalid(
                "Default workspace is missing or inactive.".to_owned(),
            ));
        }
        let orphan: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM asset_path a LEFT JOIN item i ON i.id=a.owner_id WHERE a.owner_id IS NOT NULL AND i.id IS NULL)", [], |row| row.get(0))?;
        if orphan {
            return Err(Error::Invalid(
                "Deleting an item must delete its attachment mappings.".to_owned(),
            ));
        }
        crate::store::theme_packages::guard_deleted_records(tx, &self.records)?;
        Ok(generation)
    }
}
