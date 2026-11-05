// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Disposable, canonical sample sources for the production export pipeline.
use crate::domain::publication::{
    plan::PublicationFormat, theme::ThemeSnapshot, theme_format::Definition,
};
use crate::export_jobs::{error::Error, theme_store};
use crate::store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;

pub(crate) const THEME_ID: &str = "theme-preview";
pub(crate) const PUBLICATION_ID: &str = "00000000-0000-7000-8000-000000000901";
pub(crate) const OVERVIEW_ID: &str = "00000000-0000-7000-8000-000000000902";
pub(crate) const STYLES_ID: &str = "00000000-0000-7000-8000-000000000903";
pub(crate) const DETAILS_ID: &str = "00000000-0000-7000-8000-000000000904";

/// The sample project stays open for the source's lifetime, so its writes and
/// the export that follows share one store. Fields drop in order: the store
/// closes before its folder is removed.
pub(crate) struct Source {
    pub(crate) store: Arc<Store>,
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
        let current = crate::export_jobs::theme::capture_theme(
            &self.project,
            &self.theme.id,
            self.theme.format,
        )?;
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
) -> Result<Source, Error> {
    let (snapshot, mut files) = theme_store::draft(root, id, revision, definition, format)?;
    let folder = tempfile::Builder::new()
        .prefix("bluink-theme-preview-")
        .tempdir()
        .map_err(|error| Error::Internal(error.into()))?;
    // A private physical ID lets drafts of reserved built-ins use the same real
    // package loader. Approval retains the original identity/configuration hash.
    let mut physical = definition.clone();
    physical.id = THEME_ID.into();
    files.insert(
        "manifest.json".into(),
        serde_json::to_vec(&physical).map_err(|error| Error::Internal(error.into()))?,
    );
    let store = sample_project(folder.path(), format, &files)?;
    let guard = Guard {
        project: root.to_owned(),
        revision: revision.into(),
        theme: snapshot,
    };
    guard.assert_fresh().map_err(Error::InvalidRequest)?;
    Ok(Source {
        store,
        folder,
        guard,
    })
}

fn sample_project(
    root: &Path,
    format: PublicationFormat,
    files: &crate::domain::publication::theme::ThemeFiles,
) -> Result<Arc<Store>, Error> {
    let sample: Sample = serde_json::from_str(include_str!("../../resources/sample-pages.json"))
        .map_err(|error| Error::Internal(error.into()))?;
    // Holding the store keeps helpers that resolve it by root from reopening it.
    let (store, _) = crate::store::project::open_or_create(
        root,
        "UTC",
        &json!({
            "query": crate::domain::project::query::QueryDocument::active_pages(),
            "widgets": [{"id": uuid::Uuid::now_v7().to_string(), "title": "Preview", "topBar": true,
                "home": {"span": 12, "hidden": true},
                "config": {"kind": "layout", "mode": "cards", "query": crate::domain::project::query::QueryDocument::default()}}]
        }),
    )?;
    crate::versioning::checkpoints::initialize(root)
        .map_err(|error| Error::Internal(error.into()))?;
    crate::store::theme_packages::save(root, THEME_ID, None, None, files)
        .map_err(|error| Error::Internal(error.into()))?;
    crate::store::commands::write_snippet(&store, "theme_reference", sample.reference, None)?;
    for (id, title, document) in [
        (OVERVIEW_ID, "Publication theme preview", sample.overview),
        (STYLES_ID, "Typography and structure", sample.styles),
        (DETAILS_ID, "Boxes and references", sample.details),
    ] {
        crate::store::commands::write(
            &store,
            json!({"id":id,"meta":{"type":"page","title":title,"lang":"en-GB"},"content":{"document":document}}),
            None,
            "2026-10-14",
        )?;
    }
    crate::store::assets::upload(root, STYLES_ID, "diagram.svg", sample.diagram.as_bytes())?;
    crate::store::commands::write(
        &store,
        json!({
            "id":PUBLICATION_ID,"meta":{"type":"publication","title":"Publication theme preview"},
            "content":{"kind":"publication","selectedOutput":format,"rootPageId":OVERVIEW_ID,
                "outline":[{"kind":"page","pageId":STYLES_ID,"children":[{"kind":"page","pageId":DETAILS_ID,"children":[]}]}],
            "selectionRules":[],"themeId":THEME_ID,"targets":{"pdf":{"profile":"default","outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},"html":{"outsideLinks":"error"},"site":{"layout":"documentation","outsideLinks":"error"}}}
        }),
        None,
        "2026-10-14",
    )?;
    Ok(store)
}
