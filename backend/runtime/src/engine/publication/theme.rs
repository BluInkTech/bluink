// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::plan::PublicationFormat;
use super::theme_format::{self, Definition, Overrides, ResolvedTheme, WebControl};
use crate::engine::project::asset::valid_filename;
use minijinja::value::Value as TemplateValue;
use minijinja::{AutoEscape, Environment, UndefinedBehavior, context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

pub(super) const MAX_THEME_FILES: usize = 256;
pub(super) const MAX_THEME_BYTES: usize = 16 * 1024 * 1024;
const MAX_THEME_DEPTH: usize = 8;
pub(super) type ThemeFiles = BTreeMap<String, Vec<u8>>;

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeSnapshot {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) origin: ThemeOrigin,
    pub(crate) sha256: String,
    pub(crate) execution_sha256: String,
    pub(crate) format: PublicationFormat,
    pub(crate) definition: Definition,
    pub(crate) overrides: Overrides,
    pub(crate) resolved: ResolvedTheme,
    pub(crate) options: BTreeMap<String, ThemeOptionValue>,
    pub(crate) requires_trust: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemeOrigin {
    Builtin,
    Project,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HtmlShellKind {
    Article,
    Site,
}

#[derive(Clone)]
pub(crate) struct HtmlShellContext<'a> {
    pub(crate) title: &'a str,
    pub(crate) language: &'a str,
    pub(crate) body: &'a str,
    pub(crate) navigation: &'a str,
    pub(crate) contents: &'a str,
    pub(crate) role: &'a str,
    pub(crate) page: Value,
}

pub(crate) fn capture_theme(
    root: &Path,
    id: &str,
    format: PublicationFormat,
) -> Result<ThemeSnapshot, String> {
    let (origin, files) = load_theme(root, id)?;
    capture_files(id, origin, &files, format)
}

pub(super) fn capture_files(
    id: &str,
    origin: ThemeOrigin,
    files: &ThemeFiles,
    format: PublicationFormat,
) -> Result<ThemeSnapshot, String> {
    let definition: Definition = serde_json::from_slice(
        files
            .get("manifest.json")
            .ok_or("Theme has no manifest.json")?,
    )
    .map_err(|error| format!("Invalid theme manifest: {error}"))?;
    validate_manifest(&definition, files, id)?;
    let overrides = Overrides::default();
    let resolved = theme_format::resolve(&definition, format.name(), &overrides)?;
    let executable = match format {
        PublicationFormat::Pdf | PublicationFormat::Docx | PublicationFormat::Epub => false,
        PublicationFormat::Html => definition.web.html.is_some(),
        PublicationFormat::Site => {
            definition.web.site.is_some() || !definition.web.site_roles.is_empty()
        }
    };
    let requires_trust = origin == ThemeOrigin::Project && executable;
    let options = definition
        .web
        .controls
        .iter()
        .map(|control| match control {
            WebControl::Toggle { id, default, .. } => {
                (id.clone(), ThemeOptionValue::Boolean(*default))
            }
            WebControl::Choice { id, default, .. } => {
                (id.clone(), ThemeOptionValue::Text(default.clone()))
            }
        })
        .collect();
    Ok(ThemeSnapshot {
        id: definition.id.clone(),
        version: definition.version.clone(),
        name: definition.name.clone(),
        origin,
        sha256: hash_files(files),
        execution_sha256: execution_hash(&definition, files)?,
        format,
        definition,
        overrides,
        resolved,
        options,
        requires_trust,
    })
}

pub(crate) fn apply_theme_overrides(
    snapshot: &mut ThemeSnapshot,
    overrides: &Overrides,
) -> Result<(), String> {
    for format in PublicationFormat::ALL {
        let target = format.name();
        let label = match format {
            PublicationFormat::Html => "HTML",
            PublicationFormat::Site => "Website",
            PublicationFormat::Pdf => "PDF",
            PublicationFormat::Docx => "Word",
            PublicationFormat::Epub => "EPUB",
        };
        theme_format::resolve(&snapshot.definition, target, overrides)
            .map_err(|error| format!("{label}: {error}"))?;
    }
    snapshot.resolved =
        theme_format::resolve(&snapshot.definition, snapshot.format.name(), overrides)?;
    snapshot.overrides = overrides.clone();
    Ok(())
}

pub(crate) fn validate_theme_override(value: &Value) -> Result<(), String> {
    let changes: Overrides = serde::Deserialize::deserialize(value)
        .map_err(|error| format!("Invalid appearance changes: {error}"))?;
    if changes.targets.keys().any(|target| {
        !PublicationFormat::ALL
            .iter()
            .any(|format| format.name() == target)
    }) {
        return Err("Unknown appearance target".into());
    }
    Ok(())
}

pub(crate) fn apply_theme_options(
    snapshot: &mut ThemeSnapshot,
    overrides: &BTreeMap<String, Value>,
) -> Result<(), String> {
    for (id, value) in overrides {
        let control = snapshot
            .definition
            .web
            .controls
            .iter()
            .find(|control| match control {
                WebControl::Toggle { id: key, .. } | WebControl::Choice { id: key, .. } => {
                    key == id
                }
            })
            .ok_or_else(|| format!("Theme has no setting named {id}"))?;
        let valid = match control {
            WebControl::Toggle { .. } => value.is_boolean(),
            WebControl::Choice { choices, .. } => value
                .as_str()
                .is_some_and(|value| choices.iter().any(|choice| choice == value)),
        };
        if !valid {
            return Err(format!("Theme setting {id} has an unsupported value"));
        }
        snapshot.options.insert(
            id.clone(),
            serde::Deserialize::deserialize(value).map_err(|error| error.to_string())?,
        );
    }
    Ok(())
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
            theme_format::typst::document_shell(title, language)?.into_bytes(),
        ),
        (
            "theme-values.typ".into(),
            theme_format::typst::values(&snapshot.resolved)?.into_bytes(),
        ),
    ]))
}

/// Templates are compiled once for a complete export; each page supplies its
/// own navigation, title and running-header context.
pub(crate) struct HtmlShell<'a> {
    snapshot: &'a ThemeSnapshot,
    custom: Environment<'static>,
    defaults: Environment<'static>,
    css: String,
}

impl<'a> HtmlShell<'a> {
    pub(crate) fn prepare(
        root: &Path,
        snapshot: &'a ThemeSnapshot,
        trusted: bool,
    ) -> Result<Self, String> {
        assert_theme_fresh(root, snapshot, snapshot.format)?;
        if snapshot.requires_trust && !trusted {
            return Err(format!(
                "Project theme {} requires explicit trust for web code hash {}",
                snapshot.id, snapshot.execution_sha256
            ));
        }
        let (_, files) = load_theme(root, &snapshot.id)?;
        if hash_files(&files) != snapshot.sha256 {
            return Err(format!(
                "Publication theme {} changed while loading templates",
                snapshot.id
            ));
        }

        Ok(Self {
            snapshot,
            css: theme_format::css::base_stylesheet(&snapshot.resolved)?,
            custom: html_environment(&files)?,
            defaults: html_environment(
                &builtin_theme("classic").ok_or("Missing built-in web shells")?,
            )?,
        })
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Rendering consumes the completed template context."
    )]
    pub(crate) fn render(
        &self,
        kind: HtmlShellKind,
        context: HtmlShellContext<'_>,
    ) -> Result<String, String> {
        let snapshot = self.snapshot;
        let web = &snapshot.definition.web;
        let selected = match kind {
            HtmlShellKind::Article => web.html.as_deref(),
            HtmlShellKind::Site => web
                .site_roles
                .get(context.role)
                .map(String::as_str)
                .or(web.site.as_deref()),
        };
        let (environment, path) = if let Some(path) = selected {
            (&self.custom, path.to_owned())
        } else {
            let path = match kind {
                HtmlShellKind::Article => "html/article.html",
                HtmlShellKind::Site if matches!(context.role, "listing" | "archive") => {
                    "site/listing.html"
                }
                HtmlShellKind::Site => "site/page.html",
            };
            (&self.defaults, path.to_owned())
        };
        let mut css = self.css.clone();
        theme_format::css::append_page(&mut css, &snapshot.resolved, context.title);
        css.push_str(include_str!("theme_format/web-shell.css"));
        let mut options = snapshot.options.clone();
        // The built-in fallback shell has one fixed navigation control. It is a
        // renderer default, not an obsolete persisted theme shape.
        if selected.is_none() {
            options.insert("sidebar".into(), ThemeOptionValue::Boolean(true));
        }
        environment.get_template(&path).map_err(|error|format!("Cannot load theme template {path}: {error}"))?.render(context!{
        title=>context.title,language=>context.language,body=>TemplateValue::from_safe_string(format!("<div class=\"publication-content\">{}</div>",context.body)),
        navigation=>TemplateValue::from_safe_string(context.navigation.to_owned()),contents=>TemplateValue::from_safe_string(context.contents.to_owned()),
        theme_css=>TemplateValue::from_safe_string(css),theme_options=>options,page=>context.page,role=>context.role,
    }).map_err(|error|format!("Theme template {path} failed: {error}"))
    }
}

pub(crate) fn render_html_shell(
    root: &Path,
    snapshot: &ThemeSnapshot,
    kind: HtmlShellKind,
    context: HtmlShellContext<'_>,
    trusted: bool,
) -> Result<String, String> {
    HtmlShell::prepare(root, snapshot, trusted)?.render(kind, context)
}

pub(super) fn validate_manifest(
    definition: &Definition,
    files: &ThemeFiles,
    id: &str,
) -> Result<(), String> {
    if definition.id != id {
        return Err("Theme manifest ID does not match the requested theme".into());
    }
    for format in PublicationFormat::ALL {
        let target = format.name();
        theme_format::resolve(definition, target, &Overrides::default())?;
    }
    html_environment(files)?;
    for (role, path) in [
        ("html", definition.web.html.as_deref()),
        ("site", definition.web.site.as_deref()),
    ] {
        if let Some(path) = path {
            validate_shell_slots(role, template_source(files, path)?)?;
        }
    }
    for (role, path) in &definition.web.site_roles {
        if !matches!(
            role.as_str(),
            "home" | "page" | "article" | "listing" | "archive"
        ) {
            return Err(format!("Unknown site template role: {role}"));
        }
        validate_shell_slots(
            if matches!(role.as_str(), "listing" | "archive") {
                "site_listing"
            } else {
                "site"
            },
            template_source(files, path)?,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Theme package paths are portable and case sensitive."
)]
fn html_environment(files: &ThemeFiles) -> Result<Environment<'static>, String> {
    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|_| AutoEscape::Html);
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.set_fuel(Some(50_000));
    for (path, bytes) in files {
        if path.ends_with(".html") || path.ends_with(".jinja") || path.ends_with(".j2") {
            let source = std::str::from_utf8(bytes)
                .map_err(|_| format!("Theme template {path} must be UTF-8"))?;
            environment
                .add_template_owned(path.clone(), source.to_owned())
                .map_err(|error| format!("Invalid theme template {path}: {error}"))?;
        }
    }
    Ok(environment)
}

fn template_source<'a>(files: &'a ThemeFiles, path: &str) -> Result<&'a str, String> {
    if !safe_theme_path(path) {
        return Err(format!("Unsafe theme template path: {path}"));
    }
    std::str::from_utf8(
        files
            .get(path)
            .ok_or_else(|| format!("Theme template {path} is missing"))?,
    )
    .map_err(|_| format!("Theme template {path} must be UTF-8"))
}

fn validate_shell_slots(role: &str, source: &str) -> Result<(), String> {
    let slots: &[&str] = match role {
        "site" => &[
            "{{ body }}",
            "{{ navigation }}",
            "{{ contents }}",
            "{{ theme_css }}",
        ],
        "site_listing" => &["{{ navigation }}", "{{ contents }}", "{{ theme_css }}"],
        _ => &["{{ body }}", "{{ theme_css }}"],
    };
    for slot in slots {
        if source.matches(slot).count() != 1 {
            return Err(format!(
                "Theme {role} template needs exactly one {slot} slot"
            ));
        }
    }
    Ok(())
}

pub(super) fn safe_theme_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|part| {
            valid_filename(part) && !part.starts_with('.') && !matches!(part, "." | "..")
        })
}

pub(crate) fn is_theme_id(value: &str) -> bool {
    let mut parts = value.split('-');
    value.len() <= 80
        && valid_filename(value)
        && parts.next().is_some_and(valid_segment)
        && parts.all(valid_segment)
}
fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}
pub(super) fn hash_files(files: &ThemeFiles) -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    super::digest_hex(&digest.finalize())
}

// Portable style data never executes. Bind approval to identity, web control /
// shell configuration, and every other package file, including template includes.
// The independent package hash still invalidates plans on any byte change.
pub(super) fn execution_hash(
    definition: &Definition,
    files: &ThemeFiles,
) -> Result<String, String> {
    let manifest =
        serde_json::to_vec(&serde_json::json!({"id":definition.id,"web":definition.web}))
            .map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    for (path, original) in files {
        let bytes = if path == "manifest.json" {
            &manifest
        } else {
            original
        };
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(super::digest_hex(&digest.finalize()))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ThemeOptionValue {
    Boolean(bool),
    Text(String),
}

pub(super) fn load_theme(root: &Path, id: &str) -> Result<(ThemeOrigin, ThemeFiles), String> {
    if !is_theme_id(id) {
        return Err("Theme ID must be lowercase and hyphenated.".to_owned());
    }
    let path = root.join("themes").join(id);
    if let Some(files) = builtin_theme(id) {
        if fs::symlink_metadata(&path).is_ok() {
            return Err(format!(
                "Theme ID {id} is reserved for a read-only built-in."
            ));
        }
        return Ok((ThemeOrigin::Builtin, files));
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!("Project theme {id} must be a real directory."));
            }
            let themes_dir = root.join("themes");
            if fs::symlink_metadata(&themes_dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err("Project themes directory cannot be a symbolic link.".to_owned());
            }
            let mut files = ThemeFiles::new();
            let mut remaining = MAX_THEME_BYTES;
            load_project_files(&path, &path, 0, &mut files, &mut remaining)?;
            Ok((ThemeOrigin::Project, files))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("Publication theme {id} does not exist."))
        }
        Err(error) => Err(format!("Cannot inspect theme {id}: {error}")),
    }
}

fn load_project_files(
    base: &Path,
    directory: &Path,
    depth: usize,
    files: &mut ThemeFiles,
    remaining: &mut usize,
) -> Result<(), String> {
    if depth > MAX_THEME_DEPTH {
        return Err("Theme folder nesting is too deep.".to_owned());
    }
    for entry in
        fs::read_dir(directory).map_err(|error| format!("Cannot read theme folder: {error}"))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "Theme paths must be UTF-8.".to_owned())?;
        if !valid_filename(&name) || name.starts_with('.') {
            return Err(format!("Invalid theme path component: {name}"));
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Theme files cannot be symbolic links: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            load_project_files(base, &path, depth + 1, files, remaining)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!("Theme entry is not a file: {}", path.display()));
        }
        if files.len() >= MAX_THEME_FILES {
            return Err("Theme has too many files.".to_owned());
        }
        let relative = path.strip_prefix(base).map_err(|error| error.to_string())?;
        let key = relative
            .to_str()
            .ok_or("Theme paths must be UTF-8.")?
            .replace('\\', "/");
        if metadata.len() > *remaining as u64 {
            return Err("Theme files exceed the 16 MiB package limit.".to_owned());
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| file.take(*remaining as u64 + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("Cannot read theme file {key}: {error}"))?;
        if bytes.len() > *remaining {
            return Err("Theme files exceed the 16 MiB package limit.".to_owned());
        }
        *remaining -= bytes.len();
        files.insert(key, bytes);
    }
    Ok(())
}

pub(super) fn builtin_theme(id: &str) -> Option<ThemeFiles> {
    let sources: &[(&str, &[u8])] = match id {
        "classic" => &[
            (
                "manifest.json",
                include_bytes!("../../../../vendor/themes/classic/manifest.json"),
            ),
            (
                "html/article.html",
                include_bytes!("../../../../vendor/themes/classic/html/article.html"),
            ),
            (
                "site/page.html",
                include_bytes!("../../../../vendor/themes/classic/site/page.html"),
            ),
            (
                "site/listing.html",
                include_bytes!("../../../../vendor/themes/classic/site/listing.html"),
            ),
        ],
        "editorial" => &[
            (
                "manifest.json",
                include_bytes!("../../../../vendor/themes/editorial/manifest.json"),
            ),
            (
                "html/article.html",
                include_bytes!("../../../../vendor/themes/editorial/html/article.html"),
            ),
            (
                "site/page.html",
                include_bytes!("../../../../vendor/themes/editorial/site/page.html"),
            ),
            (
                "site/listing.html",
                include_bytes!("../../../../vendor/themes/editorial/site/listing.html"),
            ),
        ],
        _ => return None,
    };
    Some(
        sources
            .iter()
            .map(|(path, source)| ((*path).to_owned(), source.to_vec()))
            .collect(),
    )
}
