// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use minijinja::value::Value as TemplateValue;
use minijinja::{AutoEscape, Environment, UndefinedBehavior, context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::engine::project::asset::valid_filename;
use crate::engine::publication::macros::RenderMacros;
use crate::engine::publication::plan::PublicationFormat;
use crate::engine::publication::style::css_stylesheet;

const MAX_THEME_FILES: usize = 256;
const MAX_THEME_BYTES: usize = 16 * 1024 * 1024;
const MAX_THEME_DEPTH: usize = 8;

type ThemeFiles = BTreeMap<String, Vec<u8>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeManifest {
    pub(crate) schema_version: u8,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) settings: ThemeSettings,
    #[serde(default)]
    pub(crate) controls: Vec<ThemeControl>,
    pub(crate) templates: ThemeTemplates,
    #[serde(default)]
    pub(crate) macros: ThemeMacros,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeControl {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: ThemeControlKind,
    pub(crate) default: ThemeOptionValue,
    #[serde(default)]
    pub(crate) choices: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemeControlKind {
    Toggle,
    Choice,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeSettings {
    pub(crate) colors: ThemeColors,
    pub(crate) typography: ThemeTypography,
    pub(crate) layout: ThemeLayout,
}

macro_rules! settings_section {
    ($name:ident, $partial:ident { $($field:ident: $ty:ty),+ $(,)? }) => {
        #[derive(Clone, Debug, Deserialize, Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[cfg_attr(test, derive(ts_rs::TS))]
        pub(crate) struct $name { $(pub(crate) $field: $ty),+ }
        #[derive(Clone, Debug, Default, Deserialize, Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
        pub(crate) struct $partial { $(#[serde(skip_serializing_if = "Option::is_none")] pub(crate) $field: Option<$ty>),+ }
    };
}
settings_section!(
    ThemeColors,
    ThemeColorsOverride {
        ink: String,
        muted: String,
        link: String,
        accent: String,
        surface: String,
        paper: String
    }
);
settings_section!(
    ThemeTypography,
    ThemeTypographyOverride {
        body_family: String,
        heading_family: String,
        body_scale: f64,
        leading: f64
    }
);
settings_section!(
    ThemeLayout,
    ThemeLayoutOverride {
        measure_ch: u8,
        spacing: f64
    }
);
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ThemeSettingsOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    colors: Option<ThemeColorsOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    typography: Option<ThemeTypographyOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<ThemeLayoutOverride>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeTemplates {
    pub(crate) pdf: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) site: Option<String>,
    #[serde(default)]
    pub(crate) site_roles: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeMacros {
    pub(crate) pdf: Option<String>,
    pub(crate) html: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeSnapshot {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) origin: ThemeOrigin,
    pub(crate) sha256: String,
    pub(crate) settings: ThemeSettings,
    pub(crate) controls: Vec<ThemeControl>,
    pub(crate) options: BTreeMap<String, ThemeOptionValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
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
    /// Markup already escaped and assembled by Bluink's document renderer.
    pub(crate) body: &'a str,
    /// Markup already escaped and assembled by Bluink's site renderer.
    pub(crate) navigation: &'a str,
    /// Current page heading links assembled by Bluink's site renderer.
    pub(crate) contents: &'a str,
    pub(crate) role: &'a str,
    pub(crate) page: Value,
}

pub(crate) struct PdfThemePackage {
    pub(crate) shell_path: String,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) macros: RenderMacros,
}

pub(crate) fn load_pdf_theme(
    root: &Path,
    snapshot: &ThemeSnapshot,
    trust_project_theme: bool,
) -> Result<PdfThemePackage, String> {
    let (manifest, files) = checked_theme(root, snapshot, ThemeTarget::Pdf, trust_project_theme)?;
    let shell_path = manifest.templates.pdf.ok_or("Theme has no PDF shell.")?;
    let override_source = manifest
        .macros
        .pdf
        .as_deref()
        .map(|path| macro_source(&files, path, "pdf"))
        .transpose()?;
    let macros = RenderMacros::new("pdf", override_source)?.for_theme(&manifest.id);
    Ok(PdfThemePackage {
        shell_path,
        files,
        macros,
    })
}

pub(crate) fn capture_theme(
    root: &Path,
    id: &str,
    format: PublicationFormat,
) -> Result<ThemeSnapshot, String> {
    capture_theme_for(root, id, format.into())
}

pub(crate) fn capture_article_theme(root: &Path, id: &str) -> Result<ThemeSnapshot, String> {
    capture_theme_for(root, id, ThemeTarget::Html)
}

fn capture_theme_for(root: &Path, id: &str, target: ThemeTarget) -> Result<ThemeSnapshot, String> {
    let (origin, files) = load_theme(root, id)?;
    let manifest_source = files
        .get("manifest.json")
        .ok_or("Theme has no manifest.json.")?;
    let manifest: ThemeManifest = serde_json::from_slice(manifest_source)
        .map_err(|error| format!("Invalid theme manifest: {error}"))?;
    validate_manifest(&manifest, &files, id, target)?;
    Ok(ThemeSnapshot {
        id: manifest.id,
        version: manifest.version,
        name: manifest.name,
        origin,
        sha256: hash_files(&files),
        settings: manifest.settings,
        options: manifest
            .controls
            .iter()
            .map(|control| (control.id.clone(), control.default.clone()))
            .collect(),
        controls: manifest.controls,
    })
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn render_html_shell(
    root: &Path,
    snapshot: &ThemeSnapshot,
    kind: HtmlShellKind,
    context: HtmlShellContext<'_>,
    trust_project_theme: bool,
) -> Result<String, String> {
    let target = match kind {
        HtmlShellKind::Article => ThemeTarget::Html,
        HtmlShellKind::Site => ThemeTarget::Site,
    };
    let (manifest, files) = checked_theme(root, snapshot, target, trust_project_theme)?;
    let default_path = match kind {
        HtmlShellKind::Article => manifest.templates.html.as_deref(),
        HtmlShellKind::Site => manifest.templates.site.as_deref(),
    }
    .ok_or("Theme does not support the requested HTML shell.")?;
    let path = if matches!(kind, HtmlShellKind::Site) {
        manifest
            .templates
            .site_roles
            .get(context.role)
            .map_or(default_path, String::as_str)
    } else {
        default_path
    };
    let mut environment = html_environment(&files)?;
    environment.set_fuel(Some(50_000));
    let template = environment
        .get_template(path)
        .map_err(|error| format!("Cannot load theme template {path}: {error}"))?;
    template
        .render(context! {
            title => context.title,
            language => context.language,
            body => TemplateValue::from_safe_string(context.body.to_owned()),
            navigation => TemplateValue::from_safe_string(context.navigation.to_owned()),
            contents => TemplateValue::from_safe_string(context.contents.to_owned()),
            theme_css => TemplateValue::from_safe_string(css_stylesheet(&snapshot.settings)?),
            theme_options => &snapshot.options,
            page => &context.page,
            role => context.role,
        })
        .map_err(|error| format!("Theme template {path} failed: {error}"))
}

pub(crate) fn load_html_macros(
    root: &Path,
    snapshot: &ThemeSnapshot,
    kind: HtmlShellKind,
    trust_project_theme: bool,
) -> Result<RenderMacros, String> {
    let target = match kind {
        HtmlShellKind::Article => ThemeTarget::Html,
        HtmlShellKind::Site => ThemeTarget::Site,
    };
    let (manifest, files) = checked_theme(root, snapshot, target, trust_project_theme)?;
    let override_source = manifest
        .macros
        .html
        .as_deref()
        .map(|path| macro_source(&files, path, "html"))
        .transpose()?;
    Ok(RenderMacros::new("html", override_source)?.for_theme(&manifest.id))
}

fn checked_theme(
    root: &Path,
    snapshot: &ThemeSnapshot,
    target: ThemeTarget,
    trust_project_theme: bool,
) -> Result<(ThemeManifest, ThemeFiles), String> {
    if matches!(snapshot.origin, ThemeOrigin::Project) && !trust_project_theme {
        return Err(format!(
            "Project theme {} requires explicit trust for package hash {}.",
            snapshot.id, snapshot.sha256
        ));
    }
    let current = capture_theme_for(root, &snapshot.id, target)?;
    if current.sha256 != snapshot.sha256
        || !matches!(
            (current.origin, snapshot.origin),
            (ThemeOrigin::Builtin, ThemeOrigin::Builtin)
                | (ThemeOrigin::Project, ThemeOrigin::Project)
        )
    {
        return Err(format!(
            "Publication theme {} changed after planning.",
            snapshot.id
        ));
    }
    let (_, files) = load_theme(root, &snapshot.id)?;
    if hash_files(&files) != snapshot.sha256 {
        return Err(format!(
            "Publication theme {} changed while loading templates.",
            snapshot.id
        ));
    }
    let manifest: ThemeManifest = serde_json::from_slice(
        files
            .get("manifest.json")
            .ok_or("Theme has no manifest.json.")?,
    )
    .map_err(|error| format!("Invalid theme manifest: {error}"))?;
    Ok((manifest, files))
}

#[derive(Clone, Copy)]
enum ThemeTarget {
    Pdf,
    Html,
    Site,
}

impl From<PublicationFormat> for ThemeTarget {
    fn from(format: PublicationFormat) -> Self {
        match format {
            PublicationFormat::Pdf => Self::Pdf,
            PublicationFormat::Html => Self::Html,
            PublicationFormat::Site => Self::Site,
        }
    }
}

pub(crate) fn assert_theme_fresh(
    root: &Path,
    prior: &ThemeSnapshot,
    format: PublicationFormat,
) -> Result<(), String> {
    let current = capture_theme(root, &prior.id, format)?;
    if current.sha256 != prior.sha256
        || !matches!(
            (current.origin, prior.origin),
            (ThemeOrigin::Builtin, ThemeOrigin::Builtin)
                | (ThemeOrigin::Project, ThemeOrigin::Project)
        )
    {
        return Err(format!(
            "Publication theme {} changed after planning.",
            prior.id
        ));
    }
    Ok(())
}

fn load_theme(root: &Path, id: &str) -> Result<(ThemeOrigin, ThemeFiles), String> {
    if !is_theme_id(id) {
        return Err("Theme ID must be lowercase and hyphenated.".to_owned());
    }
    let path = root.join("themes").join(id);
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
            load_project_files(&path, &path, 0, &mut files)?;
            Ok((ThemeOrigin::Project, files))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => builtin_theme(id)
            .map(|files| (ThemeOrigin::Builtin, files))
            .ok_or_else(|| format!("Publication theme {id} does not exist.")),
        Err(error) => Err(format!("Cannot inspect theme {id}: {error}")),
    }
}

fn load_project_files(
    base: &Path,
    directory: &Path,
    depth: usize,
    files: &mut ThemeFiles,
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
            load_project_files(base, &path, depth + 1, files)?;
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
        let bytes =
            fs::read(&path).map_err(|error| format!("Cannot read theme file {key}: {error}"))?;
        let total = files.values().map(Vec::len).sum::<usize>() + bytes.len();
        if total > MAX_THEME_BYTES {
            return Err("Theme files exceed the 16 MiB package limit.".to_owned());
        }
        files.insert(key, bytes);
    }
    Ok(())
}

fn builtin_theme(id: &str) -> Option<ThemeFiles> {
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
            (
                "pdf/document.typ",
                include_bytes!("../../../../vendor/themes/classic/pdf/document.typ"),
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
            (
                "pdf/document.typ",
                include_bytes!("../../../../vendor/themes/editorial/pdf/document.typ"),
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

fn validate_manifest(
    manifest: &ThemeManifest,
    files: &ThemeFiles,
    requested_id: &str,
    target: ThemeTarget,
) -> Result<(), String> {
    if manifest.schema_version != 1 || manifest.id != requested_id || !is_theme_id(&manifest.id) {
        return Err("Theme manifest version or ID does not match the requested theme.".to_owned());
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 80 || !is_version(&manifest.version)
    {
        return Err("Theme needs a name and a three-part numeric version.".to_owned());
    }
    validate_settings(&manifest.settings)?;
    validate_controls(&manifest.controls)?;
    validate_html_syntax(files)?;
    let selected = match target {
        ThemeTarget::Pdf => manifest.templates.pdf.as_deref(),
        ThemeTarget::Html => manifest.templates.html.as_deref(),
        ThemeTarget::Site => manifest.templates.site.as_deref(),
    }
    .ok_or_else(|| format!("Theme {} does not support this output format.", manifest.id))?;
    for (role, path) in [
        ("pdf", manifest.templates.pdf.as_deref()),
        ("html", manifest.templates.html.as_deref()),
        ("site", manifest.templates.site.as_deref()),
    ] {
        if let Some(path) = path {
            let source = template_source(files, path, role)?;
            validate_shell_slots(role, source)?;
        }
    }
    for (role, path) in &manifest.templates.site_roles {
        if !matches!(
            role.as_str(),
            "home" | "page" | "article" | "listing" | "archive"
        ) {
            return Err(format!("Unknown site template role: {role}."));
        }
        let source = template_source(files, path, "site")?;
        validate_shell_slots(
            if matches!(role.as_str(), "listing" | "archive") {
                "site_listing"
            } else {
                "site"
            },
            source,
        )?;
    }
    template_source(
        files,
        selected,
        match target {
            ThemeTarget::Pdf => "pdf",
            ThemeTarget::Html => "html",
            ThemeTarget::Site => "site",
        },
    )?;
    for (role, path) in [
        ("html", manifest.macros.html.as_deref()),
        ("pdf", manifest.macros.pdf.as_deref()),
    ] {
        if let Some(path) = path {
            RenderMacros::new(role, Some(macro_source(files, path, role)?))?;
        }
    }
    Ok(())
}

fn validate_controls(controls: &[ThemeControl]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for control in controls {
        if !is_theme_id(&control.id)
            || !ids.insert(&control.id)
            || control.label.trim().is_empty()
            || control.label.len() > 80
        {
            return Err("Theme controls need distinct IDs and readable labels.".to_owned());
        }
        match control.kind {
            ThemeControlKind::Toggle
                if control.default.is_boolean() && control.choices.is_empty() => {}
            ThemeControlKind::Choice
                if control
                    .default
                    .as_str()
                    .is_some_and(|value| control.choices.iter().any(|choice| choice == value))
                    && !control.choices.is_empty()
                    && control.choices.len() <= 20 => {}
            _ => {
                return Err(format!(
                    "Theme control {} has an invalid default or choices.",
                    control.id
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn apply_theme_options(
    snapshot: &mut ThemeSnapshot,
    overrides: &BTreeMap<String, Value>,
) -> Result<(), String> {
    for (id, value) in overrides {
        let control = snapshot
            .controls
            .iter()
            .find(|control| &control.id == id)
            .ok_or_else(|| format!("Theme has no setting named {id}."))?;
        let valid = match control.kind {
            ThemeControlKind::Toggle => value.is_boolean(),
            ThemeControlKind::Choice => value
                .as_str()
                .is_some_and(|text| control.choices.iter().any(|choice| choice == text)),
        };
        if !valid {
            return Err(format!("Theme setting {id} has an unsupported value."));
        }
        snapshot.options.insert(
            id.clone(),
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

pub(crate) fn validate_theme_override(value: &Value) -> Result<(), String> {
    let _: ThemeSettingsOverride = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid appearance changes: {error}"))?;
    let root = value
        .as_object()
        .ok_or("Publication appearance changes must be an object.")?;
    for (section, fields) in root {
        let allowed: &[&str] = match section.as_str() {
            "colors" => &["ink", "muted", "link", "accent", "surface", "paper"],
            "typography" => &["bodyFamily", "headingFamily", "bodyScale", "leading"],
            "layout" => &["measureCh", "spacing"],
            _ => return Err(format!("Unknown appearance section: {section}.")),
        };
        let values = fields
            .as_object()
            .ok_or_else(|| format!("Appearance section {section} must be an object."))?;
        for (key, value) in values {
            if !allowed.contains(&key.as_str())
                || value.is_null()
                || value.is_array()
                || value.is_object()
            {
                return Err(format!(
                    "Unknown or invalid appearance setting: {section}.{key}."
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn apply_theme_settings(
    snapshot: &mut ThemeSnapshot,
    overrides: &Value,
) -> Result<(), String> {
    validate_theme_override(overrides)?;
    let mut value = serde_json::to_value(&snapshot.settings).map_err(|error| error.to_string())?;
    for (section, fields) in overrides.as_object().ok_or("Invalid appearance changes.")? {
        for (key, override_value) in fields.as_object().ok_or("Invalid appearance section.")? {
            value[section][key] = override_value.clone();
        }
    }
    let settings: ThemeSettings = serde_json::from_value(value)
        .map_err(|error| format!("Invalid appearance setting: {error}"))?;
    validate_settings(&settings)?;
    snapshot.settings = settings;
    Ok(())
}

fn macro_source<'a>(files: &'a ThemeFiles, path: &str, role: &str) -> Result<&'a str, String> {
    let suffix = if role == "html" { ".html" } else { ".j2" };
    if !safe_theme_path(path) || !path.ends_with(suffix) {
        return Err(format!("Unsafe or unsupported {role} macro path: {path}."));
    }
    let source = files
        .get(path)
        .ok_or_else(|| format!("Theme macro file {path} is missing."))?;
    std::str::from_utf8(source).map_err(|_| format!("Theme macro file {path} must be UTF-8."))
}

fn validate_html_syntax(files: &ThemeFiles) -> Result<(), String> {
    html_environment(files).map(|_| ())
}
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
fn html_environment(files: &ThemeFiles) -> Result<Environment<'static>, String> {
    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|_| AutoEscape::Html);
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.set_fuel(Some(50_000));
    for (path, bytes) in files {
        if !path.ends_with(".html") {
            continue;
        }
        let source = std::str::from_utf8(bytes)
            .map_err(|_| format!("Theme template {path} must be UTF-8."))?;
        environment
            .add_template_owned(path.to_owned(), source.to_owned())
            .map_err(|error| format!("Invalid theme template {path}: {error}"))?;
    }
    Ok(environment)
}

fn template_source<'a>(files: &'a ThemeFiles, path: &str, role: &str) -> Result<&'a str, String> {
    if !safe_theme_path(path) || !path.ends_with(if role == "pdf" { ".typ" } else { ".html" }) {
        return Err(format!(
            "Unsafe or unsupported theme template path: {path}."
        ));
    }
    let source = files
        .get(path)
        .ok_or_else(|| format!("Theme template {path} is missing."))?;
    std::str::from_utf8(source).map_err(|_| format!("Theme template {path} must be UTF-8."))
}

fn validate_shell_slots(role: &str, source: &str) -> Result<(), String> {
    let slots: &[&str] = match role {
        "pdf" => &[
            "#include \"content.typ\"",
            "#import \"theme-values.typ\": *",
        ],
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
                "Theme {role} template needs exactly one {slot} slot."
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_settings(settings: &ThemeSettings) -> Result<(), String> {
    crate::engine::publication::fonts::family(&settings.typography.body_family)?;
    crate::engine::publication::fonts::family(&settings.typography.heading_family)?;
    let colors = &settings.colors;
    for (role, value) in [
        ("ink", &colors.ink),
        ("muted", &colors.muted),
        ("link", &colors.link),
        ("accent", &colors.accent),
        ("surface", &colors.surface),
        ("paper", &colors.paper),
    ] {
        if parse_color(value).is_none() {
            return Err(format!("Theme color {role} needs a six-digit hex value."));
        }
    }
    let paper = parse_color(&colors.paper).ok_or("Invalid paper color.")?;
    for (role, value) in [
        ("ink", &colors.ink),
        ("muted", &colors.muted),
        ("link", &colors.link),
    ] {
        if contrast(parse_color(value).ok_or("Invalid theme color.")?, paper) < 4.5 {
            return Err(format!(
                "Theme {role} color needs at least 4.5:1 contrast against paper."
            ));
        }
    }
    let typography = &settings.typography;
    if !(0.8..=1.4).contains(&typography.body_scale)
        || !(1.25..=2.2).contains(&typography.leading)
        || !(45..=100).contains(&settings.layout.measure_ch)
        || !(0.75..=1.5).contains(&settings.layout.spacing)
    {
        return Err("Theme typography or layout settings are outside supported ranges.".to_owned());
    }
    Ok(())
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    if value.len() != 7 || !value.starts_with('#') {
        return None;
    }
    Some([
        u8::from_str_radix(&value[1..3], 16).ok()?,
        u8::from_str_radix(&value[3..5], 16).ok()?,
        u8::from_str_radix(&value[5..7], 16).ok()?,
    ])
}

fn contrast(left: [u8; 3], right: [u8; 3]) -> f64 {
    fn luminance(color: [u8; 3]) -> f64 {
        let channels = color.map(|value| {
            let normalized = f64::from(value) / 255.0;
            if normalized <= 0.04045 {
                normalized / 12.92
            } else {
                ((normalized + 0.055) / 1.055).powf(2.4)
            }
        });
        0.0722f64.mul_add(
            channels[2],
            0.7152f64.mul_add(channels[1], 0.2126 * channels[0]),
        )
    }
    let a = luminance(left);
    let b = luminance(right);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn safe_theme_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|component| {
            valid_filename(component)
                && !matches!(component, "." | "..")
                && !component.starts_with('.')
        })
        && !path.contains('\\')
}

fn is_theme_id(value: &str) -> bool {
    let mut segments = value.split('-');
    segments.next().is_some_and(valid_segment) && segments.all(valid_segment)
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

fn is_version(value: &str) -> bool {
    let mut parts = value.split('.');
    let valid = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    });
    valid && parts.next().is_none()
}

fn hash_files(files: &ThemeFiles) -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    super::digest_hex(&digest.finalize())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    use super::{
        HtmlShellContext, HtmlShellKind, ThemeOrigin, assert_theme_fresh, builtin_theme,
        capture_article_theme, capture_theme, load_html_macros, load_pdf_theme, render_html_shell,
    };
    use crate::engine::publication::html::{HtmlResources, render_html_document};
    use crate::engine::publication::plan::PublicationFormat;

    #[test]
    fn theme_controls_and_listing_role_allow_structural_overrides() {
        let root = std::env::temp_dir().join(format!("bluink-role-{}", uuid::Uuid::now_v7()));
        let folder = root.join("themes/custom");
        std::fs::create_dir_all(&folder).unwrap();
        for (path, data) in builtin_theme("classic").unwrap() {
            let target = folder.join(&path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            if path == "manifest.json" {
                let mut manifest: serde_json::Value = serde_json::from_slice(&data).unwrap();
                manifest["id"] = serde_json::json!("custom");
                std::fs::write(target, serde_json::to_vec(&manifest).unwrap()).unwrap();
            } else {
                std::fs::write(target, data).unwrap();
            }
        }
        std::fs::write(folder.join("site/listing.html"),
            "{{ theme_css }}{{ navigation }}{{ contents }}<main>{% for entry in page.entries %}<a href=\"{{ entry.href }}\">Custom: {{ entry.title }}</a>{% endfor %}</main>").unwrap();
        let mut theme = capture_theme(&root, "custom", PublicationFormat::Site).unwrap();
        super::apply_theme_options(
            &mut theme,
            &std::collections::BTreeMap::from([("sidebar".to_owned(), serde_json::json!(false))]),
        )
        .unwrap();
        let html = render_html_shell(
            &root,
            &theme,
            HtmlShellKind::Site,
            HtmlShellContext {
                title: "Listing",
                language: "en",
                body: "",
                navigation: "",
                contents: "",
                role: "listing",
                page: serde_json::json!({"entries":[{"href":"../safe.html","title":"A < B"}]}),
            },
            true,
        )
        .unwrap();
        assert!(html.contains("Custom: A &lt; B"));
        assert!(html.contains("safe.html"));
        assert!(
            super::apply_theme_options(
                &mut theme,
                &std::collections::BTreeMap::from([(
                    "unknown".to_owned(),
                    serde_json::json!(true)
                )])
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_appearance_change_preserves_theme_defaults() {
        let root = Path::new("/unused-bluink-theme-root");
        let mut theme = capture_theme(root, "classic", PublicationFormat::Site).unwrap();
        let original_paper = theme.settings.colors.paper.clone();
        super::apply_theme_settings(
            &mut theme,
            &serde_json::json!({"typography":{"bodyScale":1.2}}),
        )
        .unwrap();
        assert_eq!(theme.settings.typography.body_scale, 1.2);
        assert_eq!(theme.settings.colors.paper, original_paper);
        assert!(
            super::apply_theme_settings(
                &mut theme,
                &serde_json::json!({"colors":{"ink":"#ffffff"}})
            )
            .unwrap_err()
            .contains("contrast")
        );
    }

    #[test]
    fn builtins_have_distinct_valid_shared_settings() {
        let root = Path::new("/unused-bluink-theme-root");
        let classic = capture_theme(root, "classic", PublicationFormat::Pdf).unwrap();
        let editorial = capture_theme(root, "editorial", PublicationFormat::Site).unwrap();
        assert!(matches!(classic.origin, ThemeOrigin::Builtin));
        assert!(matches!(editorial.origin, ThemeOrigin::Builtin));
        assert_ne!(classic.sha256, editorial.sha256);
        assert_theme_fresh(root, &classic, PublicationFormat::Pdf).unwrap();
        let article = capture_article_theme(root, "classic").unwrap();
        let html = render_html_shell(
            root,
            &article,
            HtmlShellKind::Article,
            HtmlShellContext {
                title: "<img src=x onerror=alert(1)>",
                language: "en",
                body: "<p>Rendered &amp; safe</p>",
                navigation: "",
                contents: "",
                role: "article",
                page: serde_json::json!({}),
            },
            false,
        )
        .unwrap();
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<img src=x"));
        assert!(html.contains("<p>Rendered &amp; safe</p>"));
        assert!(html.contains("--publication-ink: #222222;"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn project_package_changes_and_invalid_slots_fail_before_rendering() {
        let root = std::env::temp_dir().join(format!("bluink-theme-{}", uuid::Uuid::now_v7()));
        let path = root.join("themes/custom");
        std::fs::create_dir_all(path.join("html")).unwrap();
        std::fs::create_dir_all(path.join("site")).unwrap();
        std::fs::create_dir_all(path.join("pdf")).unwrap();
        for (name, data) in builtin_theme("classic").unwrap() {
            let output = path.join(&name);
            let bytes = if name == "manifest.json" {
                String::from_utf8(data)
                    .unwrap()
                    .replace("\"classic\"", "\"custom\"")
                    .into_bytes()
            } else {
                data
            };
            std::fs::write(output, bytes).unwrap();
        }
        let wrapper = path.join("html/macros.html");
        std::fs::write(
            &wrapper,
            "{% macro paragraph(ctx, children) %}<p{{ ctx.attributes }} class=\"prose\">{{ children }}</p>{% endmacro %}",
        )
        .unwrap();
        let manifest_path = path.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["macros"]["html"] = serde_json::json!("html/macros.html");
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let first = capture_theme(&root, "custom", PublicationFormat::Pdf).unwrap();
        assert!(matches!(first.origin, ThemeOrigin::Project));
        assert!(
            load_pdf_theme(&root, &first, false)
                .err()
                .unwrap()
                .contains("requires explicit trust")
        );
        let pdf = load_pdf_theme(&root, &first, true).unwrap();
        assert_eq!(pdf.shell_path, "pdf/document.typ");
        assert!(pdf.files.contains_key("pdf/document.typ"));
        assert!(capture_theme(&root, "custom", PublicationFormat::Site).is_ok());
        let shell_context = HtmlShellContext {
            title: "Custom",
            language: "en",
            body: "<p>Body</p>",
            navigation: "<a href=\"#chapter\">Chapter</a>",
            contents: "<a href=\"#section\">Section</a>",
            role: "page",
            page: serde_json::json!({}),
        };
        assert!(
            render_html_shell(
                &root,
                &first,
                HtmlShellKind::Site,
                shell_context.clone(),
                false
            )
            .unwrap_err()
            .contains("requires explicit trust")
        );
        let html = render_html_shell(
            &root,
            &first,
            HtmlShellKind::Site,
            shell_context.clone(),
            true,
        )
        .unwrap();
        assert!(html.contains("<a href=\"#chapter\">Chapter</a>"));
        let macros = load_html_macros(&root, &first, HtmlShellKind::Site, true).unwrap();
        let empty = HashMap::new();
        let snippets = HashMap::new();
        let assets = HashMap::from([("cover".to_owned(), "assets/cover.png".to_owned())]);
        let exclusions = HashSet::new();
        let rendered = render_html_document(
            &serde_json::json!({"type":"doc","content":[
                {"type":"list","content":[{"type":"listItem","content":[
                    {"type":"paragraph","attrs":{"id":"01a0e2a0-0000-7000-8000-000000000001"},"content":[{"type":"text","text":"Safe <nested> & text"}]}
                ]}]},
                {"type":"image","attrs":{"src":"cover","alt":"A <cover>"}}
            ]}),
            &HtmlResources {
                links: &empty,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: Some(&macros),
            },
            0,
        ).unwrap();
        assert!(rendered.contains("<li><p id=\"block-01a0e2a0-0000-7000-8000-000000000001\" class=\"prose\">Safe &lt;nested&gt; &amp; text</p>"));
        assert!(rendered.contains("<img src=\"assets/cover.png\" alt=\"A &lt;cover&gt;\">"));
        assert!(!rendered.contains("<nested>"));
        std::fs::write(
            &wrapper,
            "{% macro paragraph(ctx, children) %}<p>{{ children }}{{ missing_value }}</p>{% endmacro %}",
        )
        .unwrap();
        let invalid_snapshot = capture_theme(&root, "custom", PublicationFormat::Site).unwrap();
        let invalid_macros =
            load_html_macros(&root, &invalid_snapshot, HtmlShellKind::Site, true).unwrap();
        let invalid = render_html_document(
            &serde_json::json!({"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Content"}]}
            ]}),
            &HtmlResources {
                links: &empty,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: Some(&invalid_macros),
            },
            0,
        )
        .unwrap_err();
        assert!(invalid.contains("Theme custom html macro paragraph failed"));
        assert!(invalid.contains("HTML node paragraph"));
        std::fs::write(
            &wrapper,
            "{% macro paragraph(ctx, children) %}<p{{ ctx.attributes }} class=\"prose\">{{ children }}</p>{% endmacro %}",
        )
        .unwrap();
        std::fs::write(&wrapper, "{% macro paragraph(ctx, children) %}{% if %}").unwrap();
        assert!(
            capture_theme(&root, "custom", PublicationFormat::Site)
                .unwrap_err()
                .contains("Invalid theme template")
        );
        std::fs::write(
            &wrapper,
            "{% macro paragraph(ctx, children) %}<p{{ ctx.attributes }} class=\"prose\">{{ children }}</p>{% endmacro %}",
        )
        .unwrap();
        let article = path.join("html/article.html");
        let original = std::fs::read_to_string(&article).unwrap();
        std::fs::write(&article, format!("{original}\n<!-- edited -->\n")).unwrap();
        assert!(
            assert_theme_fresh(&root, &first, PublicationFormat::Pdf)
                .unwrap_err()
                .contains("changed")
        );
        std::fs::write(&article, original.replace("{{ body }}", "")).unwrap();
        assert!(
            capture_theme(&root, "custom", PublicationFormat::Pdf)
                .unwrap_err()
                .contains("slot")
        );
        std::fs::write(
            &article,
            original.replace("{{ title }}", "{{ missing_value }}"),
        )
        .unwrap();
        let current = capture_article_theme(&root, "custom").unwrap();
        let error = render_html_shell(&root, &current, HtmlShellKind::Article, shell_context, true)
            .unwrap_err();
        assert!(error.contains("undefined"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_contrast_and_unsafe_template_paths() {
        let root =
            std::env::temp_dir().join(format!("bluink-theme-invalid-{}", uuid::Uuid::now_v7()));
        let path = root.join("themes/custom");
        for directory in ["html", "site", "pdf"] {
            std::fs::create_dir_all(path.join(directory)).unwrap();
        }
        let mut files = builtin_theme("classic").unwrap();
        let manifest = String::from_utf8(files.remove("manifest.json").unwrap())
            .unwrap()
            .replace("\"classic\"", "\"custom\"");
        for (name, data) in files {
            std::fs::write(path.join(name), data).unwrap();
        }
        std::fs::write(
            path.join("manifest.json"),
            manifest.replace("#222222", "#eeeeee"),
        )
        .unwrap();
        assert!(
            capture_theme(&root, "custom", PublicationFormat::Pdf)
                .unwrap_err()
                .contains("contrast")
        );
        std::fs::write(
            path.join("manifest.json"),
            manifest.replace("pdf/document.typ", "../outside.typ"),
        )
        .unwrap();
        assert!(
            capture_theme(&root, "custom", PublicationFormat::Pdf)
                .unwrap_err()
                .contains("Unsafe")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ThemeOptionValue {
    Boolean(bool),
    Text(String),
}
impl ThemeOptionValue {
    const fn is_boolean(&self) -> bool {
        matches!(self, Self::Boolean(_))
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            Self::Boolean(_) => None,
        }
    }
}
