// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Independent title/body occurrences in one index. Changing a title never
//! retrieves or tokenizes unchanged author content. Queries join by rowid / 2.
use super::{Error, Result, normalize};
use rusqlite::{Transaction, params};

#[cfg(test)]
mod tests;

pub(crate) struct Prepared {
    title: String,
    body: String,
}

impl Prepared {
    /// Normalize before entering the bounded writer queue.
    pub(crate) fn new(document: &crate::store::projections::document::SearchDocument) -> Self {
        Self {
            title: normalize::fts_text(&document.title),
            body: normalize::fts_text(&document.body),
        }
    }

    pub(crate) fn install(&self, tx: &Transaction<'_>, item: i64) -> Result<()> {
        let row = title_row(item)?;
        remove(tx, item)?;
        tx.execute(
            "INSERT INTO item_fts(rowid,title,body) VALUES(?1,?2,''),(?3,'',?4)",
            params![row, self.title, row + 1, self.body],
        )?;
        Ok(())
    }
}

fn title_row(item: i64) -> Result<i64> {
    item.checked_mul(2)
        .filter(|row| *row > 0 && *row < i64::MAX)
        .ok_or_else(|| Error::Invalid("Item ID exceeds the search occurrence range.".into()))
}

pub(crate) fn update_title(tx: &Transaction<'_>, item: i64, title: &str) -> Result<()> {
    let row = title_row(item)?;
    tx.execute("DELETE FROM item_fts WHERE rowid=?1", [row])?;
    tx.execute(
        "INSERT INTO item_fts(rowid,title,body) VALUES(?1,?2,'')",
        params![row, normalize::fts_text(title)],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn update_body(tx: &Transaction<'_>, item: i64, body: &str) -> Result<()> {
    install_body(tx, item, &normalize::fts_text(body))
}

pub(crate) fn install_body(tx: &Transaction<'_>, item: i64, prepared: &str) -> Result<()> {
    let row = title_row(item)? + 1;
    tx.execute("DELETE FROM item_fts WHERE rowid=?1", [row])?;
    tx.execute(
        "INSERT INTO item_fts(rowid,title,body) VALUES(?1,'',?2)",
        params![row, prepared],
    )?;
    Ok(())
}

pub(crate) fn remove(tx: &Transaction<'_>, item: i64) -> Result<()> {
    let row = title_row(item)?;
    tx.execute(
        "DELETE FROM item_fts WHERE rowid IN (?1,?2)",
        params![row, row + 1],
    )?;
    Ok(())
}
