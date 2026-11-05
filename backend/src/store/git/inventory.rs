// SPDX-License-Identifier: MPL-2.0
//! Coverage of immutable reachability is separate from mutable remote availability.
use crate::store::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

pub(crate) type Pointers = BTreeMap<String, u64>;
pub(crate) struct Commit {
    pub(crate) id: String,
    pub(crate) parents: Vec<String>,
    pub(crate) pointers: Pointers,
}

pub(crate) fn status(db: &Connection, id: &str) -> Result<Option<(bool, Vec<String>)>> {
    let row=db.query_row("SELECT ancestry_covered,parents_json FROM git_inventory_commit WHERE oid=?1 AND validator_version=1",[id],|row|Ok((row.get::<_,bool>(0)?,row.get::<_,String>(1)?))).optional()?;
    row.map(|(covered, parents)| Ok((covered, serde_json::from_str(&parents)?)))
        .transpose()
}

pub(crate) fn cover_parents(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute("UPDATE git_inventory_commit SET ancestry_covered=NOT EXISTS (
        SELECT 1 FROM json_each(git_inventory_commit.parents_json) p LEFT JOIN git_inventory_commit parent ON parent.oid=p.value
        WHERE parent.oid IS NULL OR parent.validator_version<>1 OR parent.ancestry_covered=0) WHERE oid=?1",[id])?;
    Ok(())
}

pub(crate) fn install(tx: &Transaction<'_>, commit: &Commit) -> Result<()> {
    tx.execute(
        "DELETE FROM git_inventory_commit WHERE oid=?1 AND validator_version<>1",
        [&commit.id],
    )?;
    let inserted=tx.execute("INSERT INTO git_inventory_commit(oid,parents_json,validator_version,ancestry_covered) VALUES(?1,?2,1,0) ON CONFLICT DO NOTHING",params![commit.id,serde_json::to_string(&commit.parents)?])?;
    if inserted != 0 {
        let mut write = tx.prepare_cached(
            "INSERT INTO git_asset_delta(commit_oid,object_oid,size) VALUES(?1,?2,?3)",
        )?;
        for (oid, size) in &commit.pointers {
            write.execute(params![
                commit.id,
                oid,
                i64::try_from(*size)
                    .map_err(|_| Error::Invalid("Attachment exceeds SQLite size range".into()))?
            ])?;
        }
    }
    cover_parents(tx, &commit.id)
}

/// The recursive query accesses only covered parent IDs and indexed pointer
/// deltas. An incomplete head must be filled from Git before returning results.
pub(crate) fn reachable(
    db: &Connection,
    heads: &[String],
    excluded: &[String],
) -> Result<Option<Pointers>> {
    let mut direct_child = false;
    for head in heads.iter().chain(excluded) {
        let Some((true, parents)) = status(db, head)? else {
            return Ok(None);
        };
        // The usual push advances the remote by one commit. Its entire
        // ancestry stops at the advertised parent, regardless of repo age.
        if heads == [head.as_str()] && excluded.len() == 1 && parents == excluded {
            direct_child = true;
        }
    }
    if heads == excluded {
        return Ok(Some(Pointers::new()));
    }
    if direct_child {
        let mut query =
            db.prepare_cached("SELECT object_oid,size FROM git_asset_delta WHERE commit_oid=?1")?;
        let mut result = Pointers::new();
        for row in query.query_map([&heads[0]], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })? {
            let (oid, size) = row?;
            result.insert(
                oid,
                u64::try_from(size).map_err(|_| Error::corrupt("Invalid attachment size"))?,
            );
        }
        return Ok(Some(result));
    }
    let mut query=db.prepare_cached("WITH RECURSIVE
      wanted(oid) AS (SELECT value FROM json_each(?1) UNION SELECT p.value FROM wanted w JOIN git_inventory_commit c ON c.oid=w.oid JOIN json_each(c.parents_json) p),
      known(oid) AS (SELECT value FROM json_each(?2) UNION SELECT p.value FROM known k JOIN git_inventory_commit c ON c.oid=k.oid JOIN json_each(c.parents_json) p),
      required(oid) AS (SELECT oid FROM wanted EXCEPT SELECT oid FROM known)
      SELECT d.object_oid,min(d.size),max(d.size) FROM required r JOIN git_asset_delta d ON d.commit_oid=r.oid GROUP BY d.object_oid")?;
    let mut result = Pointers::new();
    for row in query.query_map(
        params![
            serde_json::to_string(heads)?,
            serde_json::to_string(excluded)?
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )? {
        let (oid, minimum, maximum) = row?;
        if minimum != maximum {
            return Err(Error::corrupt(
                "Git history declares inconsistent attachment sizes",
            ));
        }
        result.insert(
            oid,
            u64::try_from(minimum).map_err(|_| Error::corrupt("Invalid attachment size"))?,
        );
    }
    Ok(Some(result))
}

pub(crate) fn tree(db: &Connection, oid: &str, prefix: &str) -> Result<Option<Pointers>> {
    let row:Option<String>=db.query_row("SELECT pointers_json FROM git_tree_inventory WHERE tree_oid=?1 AND path_prefix=?2 AND validator_version=1",params![oid,prefix],|row|row.get(0)).optional()?;
    row.map(|value| {
        serde_json::from_str::<Vec<(String, u64)>>(&value)
            .map(|rows| rows.into_iter().collect())
            .map_err(Into::into)
    })
    .transpose()
}

pub(crate) fn install_tree(
    tx: &Transaction<'_>,
    oid: &str,
    prefix: &str,
    pointers: &Pointers,
) -> Result<()> {
    let values: Vec<_> = pointers.iter().collect();
    tx.execute("INSERT INTO git_tree_inventory(tree_oid,path_prefix,validator_version,pointers_json) VALUES(?1,?2,1,?3) ON CONFLICT(tree_oid,path_prefix) DO UPDATE SET validator_version=1,pointers_json=excluded.pointers_json",params![oid,prefix,serde_json::to_string(&values)?])?;
    Ok(())
}
