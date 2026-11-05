// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    OutsideLinks, PdfProfile,
    plan::PublicationFormat,
    preflight::{PlannedAsset, PlannedLink, PlannedSnippet},
    theme::ThemeSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PagePlan {
    pub(crate) page_id: String,
    pub(crate) page_revision: String,
    pub(crate) title: String,
    pub(crate) format: PublicationFormat,
    pub(crate) profile: PdfProfile,
    pub(crate) outside_links: OutsideLinks,
    pub(crate) theme: ThemeSnapshot,
    pub(crate) assets: Vec<PlannedAsset>,
    pub(crate) links: Vec<PlannedLink>,
    pub(crate) snippets: Vec<PlannedSnippet>,
    pub(crate) warnings: Vec<super::plan::DecisionWarning>,
    pub(crate) decisions_checked: bool,
}
