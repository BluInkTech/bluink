// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Immutable content and packed document manifests. Local IDs never leave storage.
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::project::canonical_json;

use super::{Error, Result};

const MAGIC: &[u8; 4] = b"BIB1";
const HEADER: usize = 12;

pub(crate) struct PreparedBlock {
    stable_id: String,
    digest: [u8; 32],
    json: String,
    pub(super) facts: super::block_facts::Facts,
}

impl PreparedBlock {
    pub(crate) fn new(block: &Value) -> Result<Self> {
        let stable_id = block["attrs"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::Invalid("Document block has no stable ID.".to_owned()))?
            .to_owned();
        let bytes = canonical_json::compact(block)?;
        Ok(Self {
            facts: super::block_facts::Facts::new(block),
            stable_id,
            digest: Sha256::digest(&bytes).into(),
            json: String::from_utf8(bytes).map_err(|error| Error::Invalid(error.to_string()))?,
        })
    }

    /// Every block row gains its facts in the transaction that inserts it, so an
    /// existing digest needs only the byte comparison that rules out a collision.
    fn install(&self, transaction: &Transaction<'_>) -> Result<i64> {
        let inserted: Option<i64> = transaction
            .prepare_cached(
                "INSERT INTO block(digest,document_json) VALUES (?1,?2) ON CONFLICT(digest) DO NOTHING RETURNING id",
            )?
            .query_row(params![self.digest.as_slice(), self.json], |row| row.get(0))
            .optional()?;
        if let Some(id) = inserted {
            transaction
                .prepare_cached("INSERT INTO block_fact(block_id,facts_json) VALUES (?1,?2)")?
                .execute(params![id, serde_json::to_string(&self.facts)?])?;
            return Ok(id);
        }
        // Compare bytes as well: a digest collision must never substitute content.
        let (id, same): (i64, bool) = transaction
            .prepare_cached("SELECT id,document_json=?2 FROM block WHERE digest=?1")?
            .query_row(params![self.digest.as_slice(), self.json], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        if !same {
            return Err(Error::Invalid(
                "Document block digest collision.".to_owned(),
            ));
        }
        Ok(id)
    }
}

pub(crate) struct PreparedBody {
    content: String,
    blocks: Option<Vec<PreparedBlock>>,
}

impl PreparedBody {
    /// Called after domain validation, outside the writer's critical section.
    pub(crate) fn new(content: &Value) -> Result<Self> {
        let mut envelope = content.clone();
        let blocks = if let Some(document) = envelope.get_mut("document") {
            let root = document
                .as_object_mut()
                .ok_or_else(|| Error::Invalid("Document must be an object.".to_owned()))?;
            let content = root
                .remove("content")
                .ok_or_else(|| Error::Invalid("Document has no content array.".to_owned()))?;
            let nodes = content
                .as_array()
                .ok_or_else(|| Error::Invalid("Document content must be an array.".to_owned()))?;
            let blocks = nodes
                .iter()
                .map(PreparedBlock::new)
                .collect::<Result<Vec<_>>>()?;
            let mut seen = BTreeSet::new();
            if blocks.iter().any(|block| !seen.insert(&block.stable_id)) {
                return Err(Error::Invalid("Duplicate document block ID.".to_owned()));
            }
            Some(blocks)
        } else {
            None
        };
        Ok(Self {
            content: serde_json::to_string(&envelope)?,
            blocks,
        })
    }

    #[cfg(test)]
    pub(crate) fn install(&self, transaction: &Transaction<'_>) -> Result<i64> {
        self.install_over(transaction, None)
    }

    /// Reuse `current` when its outer content and block references are equal, so
    /// an unchanged body neither adds a row nor advances the manifest generation.
    pub(crate) fn install_over(
        &self,
        transaction: &Transaction<'_>,
        current: Option<i64>,
    ) -> Result<i64> {
        let ids = self
            .blocks
            .as_ref()
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|block| block.install(transaction))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let packed = ids.as_deref().map(encode).transpose()?;
        if let Some(current) = current {
            let same: Option<bool> = transaction
                .prepare_cached("SELECT content_json=?2 AND block_ids IS ?3 FROM body WHERE id=?1")?
                .query_row(params![current, self.content, packed], |row| row.get(0))
                .optional()?;
            if same == Some(true) {
                return Ok(current);
            }
        }
        insert(transaction, &self.content, packed.as_deref())
    }
}

fn insert(transaction: &Transaction<'_>, content: &str, packed: Option<&[u8]>) -> Result<i64> {
    let generation = if packed.is_some() {
        Some(transaction.query_row::<i64, _, _>(
            "UPDATE project_state SET manifest_generation = manifest_generation + 1 WHERE singleton = 1 RETURNING manifest_generation", [], |row| row.get(0),
        )?)
    } else {
        None
    };
    Ok(transaction.query_row(
        "INSERT INTO body(content_json,block_ids,installed_generation) VALUES (?1,?2,?3) RETURNING id",
        params![content, packed, generation], |row| row.get(0),
    )?)
}

/// Assemble all body consumers through the same checked manifest decoder.
pub(crate) fn read(transaction: &Transaction<'_>, id: i64) -> Result<Value> {
    let (content, packed): (String, Option<Vec<u8>>) = transaction.query_row(
        "SELECT content_json,block_ids FROM body WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let mut content: Value = serde_json::from_str(&content)?;
    if let Some(packed) = packed {
        let ids = decode(&packed)?;
        // One join in manifest order; positions place rows without sorting payloads.
        let mut slots = vec![None; ids.len()];
        let mut statement = transaction.prepare_cached(
            "SELECT ordered.key,b.document_json FROM json_each(?1) ordered CROSS JOIN block b ON b.id=ordered.value",
        )?;
        let mut rows = statement.query([serde_json::to_string(&ids)?])?;
        while let Some(row) = rows.next()? {
            let slot = usize::try_from(row.get::<_, i64>(0)?)
                .ok()
                .and_then(|position| slots.get_mut(position))
                .ok_or_else(|| Error::Invalid("Invalid document manifest position.".to_owned()))?;
            let json = row
                .get_ref(1)?
                .as_str()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            *slot = Some(serde_json::from_str::<Value>(json)?);
        }
        let blocks = slots
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                Error::Invalid("Document manifest references a missing block.".to_owned())
            })?;
        let root = content
            .get_mut("document")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| Error::Invalid("Document manifest has no root.".to_owned()))?;
        root.insert("content".to_owned(), Value::Array(blocks));
    }
    Ok(content)
}

pub(super) struct Layout {
    pub(super) outer: String,
    pub(super) ids: Vec<i64>,
    pub(super) keys: Vec<String>,
}

/// Resolve all stable IDs through the covering index without fetching any subtree.
pub(super) fn layout(tx: &Transaction<'_>, base: i64) -> Result<Layout> {
    let (outer, packed): (String, Option<Vec<u8>>) = tx.query_row(
        "SELECT content_json,block_ids FROM body WHERE id=?1",
        [base],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let ids = decode(&packed.ok_or_else(|| Error::Invalid("Draft base has no manifest.".into()))?)?;
    let keys=tx.prepare_cached("SELECT b.stable_id FROM json_each(?1) ordered CROSS JOIN block b INDEXED BY block_identity ON b.id=ordered.value ORDER BY ordered.key")?
        .query_map([serde_json::to_string(&ids)?],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if keys.len() != ids.len() {
        return Err(Error::Invalid(
            "Document manifest references a missing block.".into(),
        ));
    }
    Ok(Layout { outer, ids, keys })
}

/// Reuse unchanged block versions by editor ID after the writer rechecks the base.
pub(crate) fn patch(
    transaction: &Transaction<'_>,
    base: i64,
    content_without_blocks: &str,
    order: &[String],
    changed: &BTreeMap<String, PreparedBlock>,
) -> Result<i64> {
    let Layout {
        outer: previous_outer,
        ids,
        keys,
    } = layout(transaction, base)?;
    let original: BTreeMap<_, _> = keys.into_iter().zip(ids.iter().copied()).collect();
    if original.len() != ids.len() {
        return Err(Error::Invalid("Duplicate base block ID.".into()));
    }
    let mut seen = BTreeSet::new();
    let mut next = Vec::with_capacity(order.len());
    for key in order {
        if key.is_empty() || !seen.insert(key) {
            return Err(Error::Invalid(
                "Duplicate or empty patch block ID.".to_owned(),
            ));
        }
        next.push(if let Some(block) = changed.get(key) {
            if block.stable_id != *key {
                return Err(Error::Invalid(
                    "Patch block ID differs from its key.".to_owned(),
                ));
            }
            block.install(transaction)?
        } else {
            *original
                .get(key)
                .ok_or_else(|| Error::Invalid("Patch references an unknown block.".to_owned()))?
        });
    }
    if changed.keys().any(|key| !seen.contains(key)) {
        return Err(Error::Invalid("Patch includes an unused block.".to_owned()));
    }
    if next == ids && previous_outer == content_without_blocks {
        return Ok(base);
    }
    insert(transaction, content_without_blocks, Some(&encode(&next)?))
}

pub(crate) fn encode(ids: &[i64]) -> Result<Vec<u8>> {
    let count = u64::try_from(ids.len()).map_err(|error| Error::Invalid(error.to_string()))?;
    let mut bytes = Vec::with_capacity(HEADER + ids.len() * 8);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&count.to_le_bytes());
    for id in ids {
        if *id <= 0 {
            return Err(Error::Invalid(
                "Invalid document block reference.".to_owned(),
            ));
        }
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<i64>> {
    if bytes.len() < HEADER || bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(Error::Invalid(
            "Unsupported document manifest encoding.".to_owned(),
        ));
    }
    let count = u64::from_le_bytes(
        bytes[4..HEADER]
            .try_into()
            .map_err(|error: std::array::TryFromSliceError| Error::Invalid(error.to_string()))?,
    );
    if count
        != u64::try_from((bytes.len() - HEADER) / 8)
            .map_err(|error| Error::Invalid(error.to_string()))?
        || !(bytes.len() - HEADER).is_multiple_of(8)
    {
        return Err(Error::Invalid(
            "Invalid document manifest length.".to_owned(),
        ));
    }
    bytes[HEADER..]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| {
            let id = i64::from_le_bytes(*chunk);
            if id <= 0 {
                return Err(Error::Invalid(
                    "Invalid document block reference.".to_owned(),
                ));
            }
            Ok(id)
        })
        .collect()
}
