// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Keep the exact source revisions and immutable attachments of a prepared render.
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Error, Result, Store, maintenance::DRAFT_WINDOW_MS};
use crate::export_jobs::model::Plan;
use rusqlite::params;

pub(crate) struct Guard {
    store: Arc<Store>,
    owner: String,
}

impl Guard {
    pub(crate) fn hold(store: Arc<Store>, owner: &str, plan: &Plan) -> Result<Self> {
        let mut revisions = BTreeMap::new();
        let (assets, snippets) = match plan {
            Plan::Page(page) => {
                revisions.insert(page.page_id.clone(), page.page_revision.clone());
                (&page.assets, &page.snippets)
            }
            Plan::Publication(book) => {
                revisions.insert(
                    book.publication_id.clone(),
                    book.publication_revision.clone(),
                );
                revisions.extend(
                    book.pages
                        .iter()
                        .map(|page| (page.id.clone(), page.revision.clone())),
                );
                (&book.assets, &book.snippets)
            }
        };
        for snippet in snippets {
            revisions.insert(
                crate::domain::project::named_items::id("snippet", &snippet.id),
                snippet.revision.clone(),
            );
        }
        let mut mappings = BTreeMap::new();
        for asset in assets {
            let path = super::assets::owner_path(&asset.owner_id, &asset.filename)?;
            mappings.insert(path, &asset.sha256);
        }
        let mappings = serde_json::to_string(&mappings)?;
        let count = revisions.len();
        let revisions = serde_json::to_string(&revisions)?;
        let id = owner.to_owned();
        let theme = plan.theme().id.clone();
        store.write(move |tx| {
            tx.execute(
                "INSERT INTO render_lease(id,expires_ms) VALUES(?1,?2)",
                params![id, deadline()],
            )?;
            let pinned = tx.execute(
                "INSERT INTO revision_pin(owner_kind,owner_id,item_id,revision_uuid)
                 SELECT 'render',?1,i.id,i.revision FROM json_each(?2) r
                 JOIN item i ON i.uuid=r.key AND i.revision=r.value",
                params![id, revisions],
            )?;
            if pinned != count {
                return Err(Error::RevisionConflict);
            }
            // Validate path identity as well as the digest. Several portable
            // paths can point to one immutable object and share its pin.
            let available: bool = tx.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM json_each(?1) r
                 LEFT JOIN asset_path p ON p.path=r.key AND p.oid=r.value
                 LEFT JOIN asset a ON a.oid=p.oid AND a.local_available=1
                 WHERE a.oid IS NULL)",
                [&mappings],
                |row| row.get(0),
            )?;
            if !available {
                return Err(Error::Invalid(
                    "A planned attachment changed or is unavailable.".into(),
                ));
            }
            tx.execute(
                "INSERT INTO object_pin(owner_kind,owner_id,object_kind,object_id)
                 SELECT DISTINCT 'render',?1,'lfs',value FROM json_each(?2)",
                params![id, mappings],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO object_pin(owner_kind,owner_id,object_kind,object_id)
                 SELECT 'render',?1,'lfs',oid FROM asset_path WHERE path GLOB ?2",
                params![id, format!("themes/{theme}/*")],
            )?;
            Ok(())
        })?;
        store
            .render_leases
            .lock()
            .map_err(super::error::poisoned)?
            .insert(owner.to_owned());
        Ok(Self {
            store,
            owner: owner.to_owned(),
        })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.store
            .render_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.owner);
        let id = self.owner.clone();
        if let Err(error) = self.store.write(move |tx| {
            tx.execute("DELETE FROM render_lease WHERE id=?1", [id])?;
            Ok(())
        }) {
            tracing::warn!(operation="export.release",%error,"Render pins will expire during housekeeping");
        }
    }
}

fn deadline() -> i64 {
    chrono::Utc::now()
        .timestamp_millis()
        .saturating_add(DRAFT_WINDOW_MS)
}

/// Each process renews its own live jobs. Failed processes stop renewing. A
/// single expired owner is reclaimed per tick, with backups retaining their snapshot.
pub(crate) fn maintain(store: &Store, now: i64) -> Result<()> {
    let owners =
        serde_json::to_string(&*store.render_leases.lock().map_err(super::error::poisoned)?)?;
    store.write(move |tx| {
        tx.execute(
            "UPDATE render_lease SET expires_ms=?1 WHERE id IN (SELECT value FROM json_each(?2))",
            params![deadline(), owners],
        )?;
        if !super::maintenance::protected(tx)? {
            tx.execute(
                "DELETE FROM render_lease WHERE id=(SELECT id FROM render_lease
                 WHERE expires_ms<=?1 ORDER BY expires_ms LIMIT 1)",
                [now],
            )?;
        }
        Ok(())
    })
}
