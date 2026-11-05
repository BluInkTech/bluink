// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::{plan::PublicationFormat, theme::*, theme_format};
use std::{collections::BTreeMap, path::Path};

#[cfg(test)]
mod tests;

pub(crate) fn capture_theme(
    root: &Path,
    id: &str,
    format: PublicationFormat,
) -> Result<ThemeSnapshot, String> {
    let (origin, files) = load_theme(root, id)?;
    capture_files(id, origin, &files, format)
}

pub(crate) fn assert_theme_fresh(
    root: &Path,
    prior: &ThemeSnapshot,
    format: PublicationFormat,
) -> Result<(), String> {
    if prior.format != format {
        return Err("Captured theme format does not match the requested output".into());
    }
    let current = capture_theme(root, &prior.id, format)?;
    if current.sha256 != prior.sha256
        || current.origin != prior.origin
        || current.definition != prior.definition
        || current.requires_trust != prior.requires_trust
        || current.execution_sha256 != prior.execution_sha256
    {
        return Err(format!(
            "Publication theme {} changed after planning",
            prior.id
        ));
    }
    if theme_format::resolve(&current.definition, format.name(), &prior.overrides)?
        != prior.resolved
    {
        return Err(
            "Captured theme values do not match their authored definition and overrides".into(),
        );
    }
    Ok(())
}

pub(crate) fn load_pdf_theme(
    root: &Path,
    snapshot: &ThemeSnapshot,
    title: &str,
    language: &str,
) -> Result<ThemeFiles, String> {
    assert_theme_fresh(root, snapshot, PublicationFormat::Pdf)?;
    Ok(BTreeMap::from([
        (
            "document.typ".into(),
            theme_format::typst::document_shell(title, language).into_bytes(),
        ),
        (
            "theme-values.typ".into(),
            theme_format::typst::values(&snapshot.resolved)?.into_bytes(),
        ),
    ]))
}

pub(crate) fn render_html_shell(
    root: &Path,
    snapshot: &ThemeSnapshot,
    kind: HtmlShellKind,
    context: HtmlShellContext<'_>,
    trusted: bool,
) -> Result<String, String> {
    prepare_html_shell(root, snapshot, trusted)?.render(kind, context)
}

pub(super) fn load_theme(root: &Path, id: &str) -> Result<(ThemeOrigin, ThemeFiles), String> {
    if !is_theme_id(id) {
        return Err("Theme ID must be lowercase and hyphenated.".to_owned());
    }
    if let Some(files) = builtin_theme(id) {
        return Ok((ThemeOrigin::Builtin, files));
    }
    crate::store::theme_packages::load(root, id)
        .map(|(_, files)| (ThemeOrigin::Project, files))
        .map_err(|error| error.to_string())
}

pub(crate) fn prepare_html_shell<'a>(
    root: &Path,
    snapshot: &'a ThemeSnapshot,
    trusted: bool,
) -> Result<HtmlShell<'a>, String> {
    assert_theme_fresh(root, snapshot, snapshot.format)?;
    let (_, files) = load_theme(root, &snapshot.id)?;
    HtmlShell::prepare(&files, snapshot, trusted)
}
