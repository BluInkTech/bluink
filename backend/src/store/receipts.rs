// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! A draft receipt commits with its item group; retry never replays the mutation.
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

use super::{Error, Result};

pub(crate) struct Submission<'a> {
    pub(crate) item: i64,
    pub(crate) device: &'a str,
    pub(crate) id: &'a str,
    pub(crate) hash: &'a str,
    pub(crate) base: &'a str,
}

pub(crate) fn replay(
    transaction: &Transaction<'_>,
    submission: &Submission<'_>,
) -> Result<Option<Value>> {
    let row:Option<(String,String)> = transaction.query_row(
        "SELECT request_hash,outcome_json FROM submission WHERE item_id=?1 AND device_id=?2 AND submission_uuid=?3",
        params![submission.item,submission.device,submission.id],|row|Ok((row.get(0)?,row.get(1)?)),
    ).optional()?;
    row.map(|(hash, outcome)| {
        if hash != submission.hash {
            return Err(Error::Invalid(
                "Submission ID was already used for another draft.".to_owned(),
            ));
        }
        Ok(serde_json::from_str(&outcome)?)
    })
    .transpose()
}

pub(crate) fn complete(
    transaction: &Transaction<'_>,
    submission: &Submission<'_>,
    accepted: &str,
    outcome: &Value,
) -> Result<()> {
    for revision in [submission.base, accepted] {
        require_revision(transaction, submission.item, revision)?;
    }
    transaction.execute(
        "INSERT INTO submission(item_id,device_id,submission_uuid,request_hash,base_revision,accepted_revision,outcome_json,completed_ms)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![submission.item,submission.device,submission.id,submission.hash,submission.base,accepted,serde_json::to_string(outcome)?,chrono::Utc::now().timestamp_millis()],
    )?;
    Ok(())
}

/// Pins can address a current revision or a retained one; SQL alone cannot express that union.
pub(crate) fn pin(
    transaction: &Transaction<'_>,
    owner_kind: &str,
    owner_id: &str,
    item: i64,
    revision: &str,
    expires_ms: Option<i64>,
) -> Result<()> {
    require_revision(transaction, item, revision)?;
    transaction.execute(
        "INSERT INTO revision_pin(owner_kind,owner_id,item_id,revision_uuid,expires_ms) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(owner_kind,owner_id,item_id,revision_uuid) DO UPDATE SET expires_ms=excluded.expires_ms",
        params![owner_kind,owner_id,item,revision,expires_ms],
    )?;
    Ok(())
}

fn require_revision(transaction: &Transaction<'_>, item: i64, revision: &str) -> Result<()> {
    let found:bool=transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM item WHERE id=?1 AND revision=?2 UNION ALL SELECT 1 FROM revision WHERE item_id=?1 AND revision_uuid=?2)",params![item,revision],|row|row.get(0),
    )?;
    if !found {
        return Err(Error::Invalid(
            "The exact draft revision is no longer retained.".to_owned(),
        ));
    }
    Ok(())
}
