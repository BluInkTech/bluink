// SPDX-License-Identifier: MPL-2.0
//! Indexed history membership is a projection of reachable immutable Git objects.
use crate::store::{Error, Result};
use rusqlite::{OptionalExtension, Transaction, params};

#[derive(Clone)]
pub(crate) struct Commit {
    pub(crate) oid: String,
    pub(crate) seconds: i64,
    pub(crate) message: String,
    pub(crate) items: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Name {
    pub(crate) reference: String,
    pub(crate) tag: String,
    pub(crate) value: crate::versioning::model::NamedVersion,
}

pub(crate) enum Order {
    Unchanged,
    Prepend(String),
    Replace(Vec<String>),
}

pub(crate) struct Projection {
    pub(crate) head: Option<String>,
    pub(crate) refs_digest: String,
    pub(crate) commits: Vec<Commit>,
    pub(crate) names: Vec<Name>,
    pub(crate) order: Order,
}

pub(crate) fn coverage(tx: &Transaction<'_>) -> Result<Option<(Option<String>, String)>> {
    Ok(tx.query_row("SELECT head_oid,refs_digest FROM history_coverage WHERE singleton=1 AND validator_version=1",[],|row|Ok((row.get(0)?,row.get(1)?))).optional()?)
}

/// The subset of `oids` without an ingested commit row, in their given order.
pub(crate) fn unknown<'a>(tx: &Transaction<'_>, oids: &'a [String]) -> Result<Vec<&'a str>> {
    let mut known =
        tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM history_commit WHERE oid=?1)")?;
    let mut missing = Vec::new();
    for oid in oids {
        if !known.query_row([oid], |row| row.get::<_, bool>(0))? {
            missing.push(oid.as_str());
        }
    }
    Ok(missing)
}

pub(crate) fn cached_name(
    tx: &Transaction<'_>,
    reference: &str,
    tag: &str,
) -> Result<Option<Name>> {
    Ok(tx.query_row("SELECT version_id,title,item_uuid,commit_oid FROM history_name WHERE ref_name=?1 AND tag_oid=?2 AND validator_version=1",params![reference,tag],|row|Ok(Name{reference:reference.to_owned(),tag:tag.to_owned(),value:crate::versioning::model::NamedVersion{id:row.get(0)?,title:row.get(1)?,item_id:row.get(2)?,commit_id:row.get(3)?}})).optional()?)
}

pub(crate) fn install(tx: &Transaction<'_>, projection: &Projection) -> Result<()> {
    for commit in &projection.commits {
        tx.execute("INSERT INTO history_commit(oid,committed_seconds,message) VALUES (?1,?2,?3) ON CONFLICT(oid) DO NOTHING",params![commit.oid,commit.seconds,commit.message])?;
        for item in &commit.items {
            tx.execute("INSERT INTO history_item(item_uuid,commit_oid) VALUES (?1,?2) ON CONFLICT DO NOTHING",params![item,commit.oid])?;
        }
    }
    match &projection.order {
        Order::Unchanged => {}
        Order::Prepend(oid) => {
            // Negative order keys allow one local child to prepend without
            // rewriting the existing 10k-commit traversal after every save.
            tx.execute("INSERT INTO history_reachable(commit_oid,position) SELECT ?1,coalesce(min(position),0)-1 FROM history_reachable",[oid])?;
        }
        Order::Replace(order) => {
            tx.execute("DELETE FROM history_reachable", [])?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO history_reachable(commit_oid,position) VALUES (?1,?2)",
            )?;
            for (index, oid) in order.iter().enumerate() {
                insert.execute(params![
                    oid,
                    i64::try_from(index)
                        .map_err(|_| Error::Invalid("History is too large.".to_owned()))?
                ])?;
            }
        }
    }
    tx.execute("DELETE FROM history_name", [])?;
    for name in &projection.names {
        tx.execute("INSERT INTO history_name(ref_name,tag_oid,version_id,title,item_uuid,commit_oid,validator_version) VALUES (?1,?2,?3,?4,?5,?6,1)",params![name.reference,name.tag,name.value.id,name.value.title,name.value.item_id,name.value.commit_id])?;
    }
    tx.execute("INSERT INTO history_coverage(singleton,head_oid,refs_digest,validator_version) VALUES (1,?1,?2,1) ON CONFLICT(singleton) DO UPDATE SET head_oid=excluded.head_oid,refs_digest=excluded.refs_digest,validator_version=1",params![projection.head,projection.refs_digest])?;
    Ok(())
}

pub(crate) struct Entry {
    pub(crate) oid: String,
    pub(crate) message: String,
    pub(crate) seconds: i64,
    pub(crate) names: Vec<crate::versioning::model::NamedVersion>,
}

/// Coverage is established at open/ingest/publication, never repaired by a query.
pub(crate) fn page(
    tx: &Transaction<'_>,
    head: Option<&str>,
    request: &crate::versioning::model::HistoryRequest,
) -> Result<(Vec<Entry>, bool)> {
    if coverage(tx)?.is_none_or(|value| value.0.as_deref() != head) {
        return Err(Error::Invalid(
            "History projection needs coverage before reading.".to_owned(),
        ));
    }
    let before = request
        .before
        .as_deref()
        .map(|oid| {
            tx.query_row::<i64, _, _>(
                "SELECT position FROM history_reachable WHERE commit_oid=?1",
                [oid],
                |row| row.get(0),
            )
            .optional()
        })
        .transpose()?
        .flatten();
    if request.before.is_some() && before.is_none() {
        return Err(Error::Invalid(
            "History cursor is not reachable.".to_owned(),
        ));
    }
    let limit = request.limit.unwrap_or(40).clamp(1, 100);
    let query = if request.item_id.is_some() {
        "SELECT c.oid,c.message,c.committed_seconds FROM history_commit c
        JOIN history_reachable r ON r.commit_oid=c.oid WHERE r.position>?1
        AND c.oid IN (SELECT commit_oid FROM history_item WHERE item_uuid=?2
            UNION SELECT commit_oid FROM history_name WHERE item_uuid=?2)
        ORDER BY r.position LIMIT ?3"
    } else {
        "SELECT c.oid,c.message,c.committed_seconds FROM history_reachable r
        JOIN history_commit c ON c.oid=r.commit_oid WHERE r.position>?1 AND ?2 IS NULL
        ORDER BY r.position LIMIT ?3"
    };
    let mut statement = tx.prepare_cached(query)?;
    let mut entries = statement
        .query_map(
            params![
                before.unwrap_or(i64::MIN),
                request.item_id,
                i64::try_from(limit + 1)
                    .map_err(|_| Error::Invalid("Invalid history limit.".to_owned()))?
            ],
            |row| {
                Ok(Entry {
                    oid: row.get(0)?,
                    message: row.get(1)?,
                    seconds: row.get(2)?,
                    names: Vec::new(),
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = entries.len() > limit;
    entries.truncate(limit);
    let mut names=tx.prepare_cached("SELECT version_id,title,item_uuid,commit_oid FROM history_name WHERE commit_oid=?1 ORDER BY ref_name")?;
    for entry in &mut entries {
        entry.names = names
            .query_map([&entry.oid], |row| {
                Ok(crate::versioning::model::NamedVersion {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    item_id: row.get(2)?,
                    commit_id: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok((entries, more))
}
