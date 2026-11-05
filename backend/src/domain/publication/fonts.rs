// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Resolve the shared @bluink/fonts catalogue and package offline HTML assets.
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub(crate) struct Font {
    pub(crate) id: String,
    pub(crate) family: String,
}

pub(crate) fn family(id: &str) -> Result<&'static str, String> {
    static FONTS: OnceLock<Result<Vec<Font>, String>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../../fonts/catalogue.json"))
                .map_err(|error| format!("Invalid bundled font catalogue: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)?
        .iter()
        .find(|font| font.id == id)
        .map(|font| font.family.as_str())
        .ok_or_else(|| format!("Unknown bundled font: {id}"))
}
