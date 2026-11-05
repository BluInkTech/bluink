// SPDX-License-Identifier: MPL-2.0
//! Application reads share one SQLite snapshot for selection and hydration.
pub(crate) mod analytics;
pub(crate) mod calendar;
pub(crate) mod capabilities;
pub(crate) mod catalogue;
pub(crate) mod collections;
pub(crate) mod document_graph;
pub(crate) mod items;
pub(crate) mod snippets;

pub(crate) fn timezone(tx: &rusqlite::Transaction<'_>) -> super::Result<chrono_tz::Tz> {
    let config = super::project::read_config(tx)?;
    config["analytics"]["timezone"]
        .as_str()
        .ok_or_else(|| super::Error::Invalid("Project timezone is missing.".into()))?
        .parse()
        .map_err(|error: chrono_tz::ParseError| super::Error::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests;
