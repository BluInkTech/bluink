// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Export planning reads the ordered document links and facts from one SQLite snapshot.
use crate::{
    domain::publication::{
        LinkDepth,
        preflight::{FactIssue, PageFacts},
    },
    store::{Error, Store},
};
use rusqlite::{OptionalExtension, Transaction};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedPage {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) title: String,
    pub(crate) tags: Vec<String>,
    pub(crate) published_on: Option<String>,
    /// Link levels from the starting page, which is level 0.
    pub(crate) level: usize,
    /// Internal page links in first-occurrence order.
    pub(crate) links: Vec<String>,
    pub(crate) facts: PageFacts,
}

/// Every page a walk reached, in discovery order, and the linked IDs that are
/// not active pages: missing, archived or another kind of item.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkedPages {
    pub(crate) pages: Vec<LinkedPage>,
    pub(crate) unavailable: Vec<String>,
}

/// Walk breadth first. Batches bound the SQL parameters and cancellation interval.
pub(crate) fn linked_pages(
    root: &Path,
    page_id: &str,
    depth: LinkDepth,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<LinkedPages, String> {
    Store::shared(root)
        .and_then(|store| {
            store.read(|tx| {
                let mut walked = LinkedPages::default();
                let mut seen = HashSet::from([page_id.to_owned()]);
                let mut current = vec![page_id.to_owned()];
                let mut level = 0;
                while !current.is_empty() {
                    let mut next = Vec::new();
                    for batch in current.chunks(256) {
                        let mut pages = load_pages(tx, batch, level)?;
                        for id in batch {
                            if let Some(page) = pages.remove(id) {
                                if depth.levels().is_none_or(|levels| level < levels) {
                                    for target in &page.links {
                                        if seen.insert(target.clone()) {
                                            next.push(target.clone());
                                        }
                                    }
                                }
                                walked.pages.push(page);
                            } else if level == 0 {
                                return Err(Error::Invalid(format!(
                                    "Publication page {id} must be an active page."
                                )));
                            } else {
                                walked.unavailable.push(id.clone());
                            }
                        }
                        if !progress(walked.pages.len(), seen.len()) {
                            return Err(Error::Invalid("Export cancelled.".into()));
                        }
                    }
                    current = next;
                    level += 1;
                }
                Ok(walked)
            })
        })
        .map_err(|error| error.to_string())
}

fn load_pages(
    tx: &Transaction<'_>,
    ids: &[String],
    level: usize,
) -> crate::store::Result<HashMap<String, LinkedPage>> {
    let mut statement=tx.prepare_cached("SELECT i.uuid,i.revision,i.title,json_extract(i.header_json,'$.meta'),p.page_links_json,p.export_facts_json,
        (SELECT json_group_array(e.named_key ORDER BY e.named_key) FROM relation r JOIN entity e ON e.id=r.target_id WHERE r.source_id=i.id AND r.kind='tag')
        FROM json_each(?1) wanted JOIN item i ON i.uuid=wanted.value JOIN item_projection p ON p.item_id=i.id WHERE i.type='page' AND i.state='active'")?;
    let mut rows = statement.query([serde_json::to_string(ids)?])?;
    let mut result = HashMap::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let metadata: serde_json::Value = serde_json::from_str(&row.get::<_, String>(3)?)?;
        let facts: PageFacts = serde_json::from_str(&row.get::<_, String>(5)?)?;
        let page = LinkedPage {
            id: id.clone(),
            revision: row.get(1)?,
            title: row.get(2)?,
            published_on: metadata["published_on"].as_str().map(str::to_owned),
            tags: serde_json::from_str(&row.get::<_, String>(6)?)?,
            level,
            links: serde_json::from_str(&row.get::<_, String>(4)?)?,
            facts: facts.with_metadata(&metadata),
        };
        result.insert(id, page);
    }
    Ok(result)
}

pub(crate) fn page_export_issues(root: &Path, id: &str) -> Result<Option<Vec<FactIssue>>, String> {
    Store::shared(root).and_then(|store|store.read(|tx|{
        let record:Option<(String,String)>=tx.query_row("SELECT p.export_facts_json,json_extract(i.header_json,'$.meta') FROM item i JOIN item_projection p ON p.item_id=i.id WHERE i.uuid=?1 AND p.export_facts_json IS NOT NULL",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        record.map(|(facts,metadata)|Ok(serde_json::from_str::<PageFacts>(&facts)?.with_metadata(&serde_json::from_str(&metadata)?).issues)).transpose()
    })).map_err(|error|error.to_string())
}
