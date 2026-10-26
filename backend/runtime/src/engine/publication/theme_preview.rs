// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Disposable, canonical sample sources for the production export pipeline.
use super::{
    plan::PublicationFormat,
    theme::{self, ThemeSnapshot},
    theme_format::Definition,
    theme_store,
};
use crate::engine::project::{
    config::open_or_create_project_with_workspace,
    snippets::write_snippet,
    write::{MutationError, write_item_input_typed},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub(crate) const THEME_ID: &str = "theme-preview";
pub(crate) const PUBLICATION_ID: &str = "00000000-0000-7000-8000-000000000901";
pub(crate) const OVERVIEW_ID: &str = "00000000-0000-7000-8000-000000000902";
pub(crate) const STYLES_ID: &str = "00000000-0000-7000-8000-000000000903";
pub(crate) const DETAILS_ID: &str = "00000000-0000-7000-8000-000000000904";

pub(crate) struct Source {
    pub(crate) folder: TempDir,
    pub(crate) guard: Guard,
}

pub(crate) struct Guard {
    pub(crate) project: PathBuf,
    pub(crate) revision: String,
    pub(crate) theme: ThemeSnapshot,
}

impl Guard {
    pub(crate) fn assert_fresh(&self) -> Result<(), String> {
        let current = theme::capture_theme(&self.project, &self.theme.id, self.theme.format)?;
        if current.sha256 != self.revision {
            return Err("The theme package changed after preview capture. Reload it before preparing another preview.".into());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    overview: Value,
    styles: Value,
    details: Value,
    reference: Value,
    diagram: String,
}

pub(crate) fn capture(
    root: &Path,
    id: &str,
    revision: &str,
    definition: &Definition,
    format: PublicationFormat,
) -> Result<Source, MutationError> {
    let (snapshot, mut files) = theme_store::draft(root, id, revision, definition, format)?;
    let folder = tempfile::Builder::new()
        .prefix("bluink-theme-preview-")
        .tempdir()
        .map_err(|error| MutationError::Internal(error.to_string()))?;
    // A private physical ID lets drafts of reserved built-ins use the same real
    // package loader. Approval retains the original identity/configuration hash.
    let mut physical = definition.clone();
    physical.id = THEME_ID.into();
    files.insert(
        "manifest.json".into(),
        serde_json::to_vec(&physical)
            .map_err(|error| MutationError::Internal(error.to_string()))?,
    );
    theme_store::write_files(&folder.path().join("themes").join(THEME_ID), &files)?;
    sample_project(folder.path(), format)?;
    let guard = Guard {
        project: root.to_owned(),
        revision: revision.into(),
        theme: snapshot,
    };
    guard.assert_fresh().map_err(MutationError::Validation)?;
    Ok(Source { folder, guard })
}

fn sample_project(root: &Path, format: PublicationFormat) -> Result<(), MutationError> {
    let sample: Sample =
        serde_json::from_str(include_str!("../../../../vendor/theme-preview.json"))
            .map_err(|error| MutationError::Internal(error.to_string()))?;
    open_or_create_project_with_workspace(root, "UTC", &json!({
        "query": crate::engine::project::query::QueryDocument::active_pages(),
        "widgets": [{"id": uuid::Uuid::now_v7().to_string(), "title": "Preview", "topBar": true,
            "home": {"span": 12, "hidden": true},
            "config": {"kind": "layout", "mode": "cards", "query": crate::engine::project::query::QueryDocument::default()}}]
    })).map_err(MutationError::Internal)?;
    write_snippet(root, "theme_reference", sample.reference, None)?;
    for (id, title, document) in [
        (OVERVIEW_ID, "Publication theme preview", sample.overview),
        (STYLES_ID, "Typography and structure", sample.styles),
        (DETAILS_ID, "Boxes and references", sample.details),
    ] {
        write_item_input_typed(
            root,
            json!({"id":id,"meta":{"type":"page","title":title,"lang":"en-GB"},"content":document}),
            None,
            "2026-10-14",
        )?;
    }
    crate::engine::project::asset::write_asset(
        root,
        STYLES_ID,
        "diagram.svg",
        sample.diagram.as_bytes(),
    )?;
    write_item_input_typed(
        root,
        json!({
            "id":PUBLICATION_ID,"meta":{"type":"publication","title":"Publication theme preview"},
            "content":{"kind":"publication","preset":"book","selectedOutput":format,"rootPageId":OVERVIEW_ID,
                "outline":[{"kind":"page","pageId":STYLES_ID,"children":[{"kind":"page","pageId":DETAILS_ID,"children":[]}]}],
            "selectionRules":[],"themeId":THEME_ID,"targets":{"pdf":{"profile":"default","outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},"html":{"outsideLinks":"error"},"site":{"layout":"documentation","outsideLinks":"error"}}}
        }),
        None,
        "2026-10-14",
    )?;
    Ok(())
}
