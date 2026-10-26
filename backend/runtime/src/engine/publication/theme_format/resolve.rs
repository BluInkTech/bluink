// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    Alignment, CaptionPosition, ChapterStart, Definition, Length, Orientation, Overrides, Patch,
    Role, RunningLine, Stroke, Style, Token, Unit, Value, Weight,
};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "ResolvedPublicationThemeStyle"))]
pub(crate) struct ResolvedStyle {
    pub(crate) font_family: String,
    pub(crate) font_size_pt: f64,
    pub(crate) font_weight: Weight,
    pub(crate) italic: bool,
    pub(crate) line_height: f64,
    pub(crate) color: String,
    pub(crate) background: Option<String>,
    pub(crate) space_before_pt: f64,
    pub(crate) space_after_pt: f64,
    pub(crate) indent_pt: f64,
    pub(crate) padding_pt: f64,
    pub(crate) border_width_pt: f64,
    pub(crate) border_color: String,
    pub(crate) border_style: Stroke,
    pub(crate) alignment: Alignment,
    pub(crate) keep_with_next: bool,
    pub(crate) keep_together: bool,
    pub(crate) caption_position: Option<CaptionPosition>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "ResolvedPublicationThemePage"))]
pub(crate) struct ResolvedPage {
    pub(crate) width_pt: f64,
    pub(crate) height_pt: f64,
    pub(crate) margin_top_pt: f64,
    pub(crate) margin_right_pt: f64,
    pub(crate) margin_bottom_pt: f64,
    pub(crate) margin_left_pt: f64,
    pub(crate) header_distance_pt: f64,
    pub(crate) footer_distance_pt: f64,
    pub(crate) chapter_start: ChapterStart,
    pub(crate) header: RunningLine,
    pub(crate) footer: RunningLine,
    pub(crate) measure_characters: u16,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "ResolvedPublicationTheme"))]
pub(crate) struct ResolvedTheme {
    pub(crate) root_font_size_pt: f64,
    pub(crate) colors: BTreeMap<String, String>,
    pub(crate) styles: BTreeMap<Role, ResolvedStyle>,
    pub(crate) page: ResolvedPage,
    pub(crate) fonts: BTreeSet<String>,
}

#[derive(Clone, Debug)]
enum Literal {
    Color(String),
    Length(Length),
    Font(String),
    Number(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Color,
    Length,
    Font,
    Number,
}

impl Token {
    const fn kind(&self) -> Kind {
        match self {
            Self::Color(_) => Kind::Color,
            Self::Length(_) => Kind::Length,
            Self::Font(_) => Kind::Font,
            Self::Number(_) => Kind::Number,
        }
    }
}

struct Resolver<'a> {
    tokens: &'a BTreeMap<String, Token>,
    cache: BTreeMap<String, Literal>,
    visiting: BTreeSet<String>,
}

impl Resolver<'_> {
    fn token(&mut self, name: &str, expected: Kind) -> Result<Literal, String> {
        let token = self
            .tokens
            .get(name)
            .ok_or_else(|| format!("tokens.{name}: reference does not exist"))?;
        if token.kind() != expected {
            return Err(format!(
                "tokens.{name}: expected {expected:?}, found {:?}",
                token.kind()
            ));
        }
        if let Some(value) = self.cache.get(name) {
            return Ok(value.clone());
        }
        if self.visiting.len() >= 64 || !self.visiting.insert(name.to_owned()) {
            return Err(format!(
                "tokens.{name}: cyclic or excessively deep reference"
            ));
        }
        let result = match token {
            Token::Color(value) => self.color(value).map(Literal::Color),
            Token::Length(value) => self.length(value).map(Literal::Length),
            Token::Font(value) => self.font(value).map(Literal::Font),
            Token::Number(value) => self.number(value).map(Literal::Number),
        };
        self.visiting.remove(name);
        let result = result?;
        self.cache.insert(name.to_owned(), result.clone());
        Ok(result)
    }

    fn color(&mut self, value: &Value<String>) -> Result<String, String> {
        let color = match value {
            Value::Literal(value) => value.clone(),
            Value::Reference(reference) => match self.token(&reference.name, Kind::Color)? {
                Literal::Color(value) => value,
                _ => unreachable!("Token kinds are checked before resolution"),
            },
        };
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Expected a six-digit sRGB colour".to_owned());
        }
        Ok(color.to_ascii_lowercase())
    }

    fn font(&mut self, value: &Value<String>) -> Result<String, String> {
        let font = match value {
            Value::Literal(value) => value.clone(),
            Value::Reference(reference) => match self.token(&reference.name, Kind::Font)? {
                Literal::Font(value) => value,
                _ => unreachable!("Token kinds are checked before resolution"),
            },
        };
        crate::engine::publication::fonts::family(&font)?;
        Ok(font)
    }

    fn length(&mut self, value: &Value<Length>) -> Result<Length, String> {
        match value {
            Value::Literal(value) => Ok(*value),
            Value::Reference(reference) => match self.token(&reference.name, Kind::Length)? {
                Literal::Length(value) => Ok(value),
                _ => unreachable!("Token kinds are checked before resolution"),
            },
        }
    }

    fn number(&mut self, value: &Value<f64>) -> Result<f64, String> {
        let number = match value {
            Value::Literal(value) => *value,
            Value::Reference(reference) => match self.token(&reference.name, Kind::Number)? {
                Literal::Number(value) => value,
                _ => unreachable!("Token kinds are checked before resolution"),
            },
        };
        if number.is_finite() {
            Ok(number)
        } else {
            Err("Expected a finite number".to_owned())
        }
    }

    fn distance(&mut self, value: &Value<Length>, root: f64) -> Result<f64, String> {
        let points = self.length(value)?.points(root);
        if points.is_finite() && points.abs() <= 1_000_000.0 {
            Ok(points)
        } else {
            Err("Resolved length exceeds supported bounds".to_owned())
        }
    }
}

fn merge(target: &mut serde_json::Value, patch: serde_json::Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            // References are atomic values, not objects whose properties inherit.
            if value.get("ref").is_some() {
                target.insert(key.clone(), value.clone());
            } else if let Some(previous) = target.get_mut(key) {
                merge(previous, value.clone());
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = patch;
    }
}

fn apply(definition: &mut Definition, patch: &Patch) -> Result<(), String> {
    definition.tokens.extend(patch.tokens.clone());
    if let Some(root) = &patch.root {
        definition.root.clone_from(root);
    }
    if let Some(patch) = &patch.colors {
        let mut colors =
            serde_json::to_value(&definition.colors).map_err(|error| error.to_string())?;
        merge(
            &mut colors,
            serde_json::to_value(patch).map_err(|error| error.to_string())?,
        );
        definition.colors =
            serde_json::from_value(colors).map_err(|error| format!("colors: {error}"))?;
    }
    for (role, style) in &patch.styles {
        let mut previous =
            serde_json::to_value(definition.styles.get(role).cloned().unwrap_or_default())
                .map_err(|error| error.to_string())?;
        merge(
            &mut previous,
            serde_json::to_value(style).map_err(|error| error.to_string())?,
        );
        definition.styles.insert(
            *role,
            serde_json::from_value(previous)
                .map_err(|error| format!("styles.{}: {error}", role.name()))?,
        );
    }
    if let Some(patch) = &patch.page {
        let mut page = serde_json::to_value(&definition.page).map_err(|error| error.to_string())?;
        merge(
            &mut page,
            serde_json::to_value(patch).map_err(|error| error.to_string())?,
        );
        definition.page = serde_json::from_value(page).map_err(|error| format!("page: {error}"))?;
    }
    Ok(())
}

fn checked_target(target: &str) -> Result<(), String> {
    if crate::engine::publication::plan::PublicationFormat::ALL
        .iter()
        .any(|format| format.name() == target)
    {
        Ok(())
    } else {
        Err(format!("Unknown theme target: {target}"))
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn context<T>(path: &str, result: Result<T, String>) -> Result<T, String> {
    result.map_err(|message| format!("{path}: {message}"))
}

fn nonnegative(value: f64, path: &str) -> Result<f64, String> {
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(format!("{path}: length cannot be negative"))
    }
}

#[expect(
    clippy::expect_used,
    reason = "Theme resolution validates six-digit hexadecimal colours before contrast calculation."
)]
fn luminance(hex: &str) -> f64 {
    let channels = [1, 3, 5].map(|index| {
        f64::from(u8::from_str_radix(&hex[index..index + 2], 16).expect("Validated colour")) / 255.0
    });
    let channels = channels.map(|channel| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    channels[2].mul_add(0.0722, channels[1].mul_add(0.7152, channels[0] * 0.2126))
}

fn contrast(first: &str, second: &str) -> f64 {
    let a = luminance(first);
    let b = luminance(second);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn resolve_style(
    resolver: &mut Resolver<'_>,
    style: &Style,
    mut result: ResolvedStyle,
    root: f64,
) -> Result<ResolvedStyle, String> {
    if let Some(value) = &style.font_family {
        result.font_family = resolver.font(value)?;
    }
    if let Some(value) = &style.font_size {
        result.font_size_pt = resolver.distance(value, root)?;
    }
    if let Some(value) = style.font_weight {
        result.font_weight = value;
    }
    if let Some(value) = style.italic {
        result.italic = value;
    }
    if let Some(value) = &style.line_height {
        result.line_height = resolver.number(value)?;
    }
    if let Some(value) = &style.color {
        result.color = resolver.color(value)?;
    }
    if let Some(value) = &style.background {
        result.background = Some(resolver.color(value)?);
    }
    for (value, destination, name) in [
        (
            &style.space_before,
            &mut result.space_before_pt,
            "spaceBefore",
        ),
        (&style.space_after, &mut result.space_after_pt, "spaceAfter"),
        (&style.padding, &mut result.padding_pt, "padding"),
        (
            &style.border_width,
            &mut result.border_width_pt,
            "borderWidth",
        ),
    ] {
        if let Some(value) = value {
            *destination = nonnegative(resolver.distance(value, root)?, name)?;
        }
    }
    if let Some(value) = &style.indent {
        result.indent_pt = resolver.distance(value, root)?;
    }
    if let Some(value) = &style.border_color {
        result.border_color = resolver.color(value)?;
    }
    if let Some(value) = style.border_style {
        result.border_style = value;
    }
    if let Some(value) = style.alignment {
        result.alignment = value;
    }
    if let Some(value) = style.keep_with_next {
        result.keep_with_next = value;
    }
    if let Some(value) = style.keep_together {
        result.keep_together = value;
    }
    if let Some(value) = style.caption_position {
        result.caption_position = Some(value);
    }
    if !(1.0..=1000.0).contains(&result.font_size_pt) {
        return Err("fontSize: expected 1–1000pt".to_owned());
    }
    if !(0.5..=4.0).contains(&result.line_height) {
        return Err("lineHeight: expected a multiplier between 0.5 and 4".to_owned());
    }
    Ok(result)
}

fn validate_metadata(
    definition: &Definition,
    target: &str,
    overrides: &Overrides,
) -> Result<(), String> {
    checked_target(target)?;
    for name in definition.targets.keys().chain(overrides.targets.keys()) {
        checked_target(name)?;
    }
    if definition.schema_version != 1 {
        return Err("schemaVersion: expected 1".to_owned());
    }
    if !crate::engine::publication::theme::is_theme_id(&definition.id) {
        return Err("id: expected a lowercase hyphenated theme ID".to_owned());
    }
    if definition.name.trim().is_empty()
        || definition.name.chars().count() > 80
        || definition.name.chars().any(char::is_control)
    {
        return Err("name: expected 1–80 characters".to_owned());
    }
    let version = definition.version.split('.').collect::<Vec<_>>();
    if version.len() != 3
        || version
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("version: expected three numeric components".to_owned());
    }
    validate_web(&definition.web)
}

fn validate_web(web: &super::Web) -> Result<(), String> {
    let mut paths = web
        .html
        .iter()
        .chain(web.site.iter())
        .chain(web.site_roles.values());
    if paths.any(|path| {
        path.len() > 1024
            || path.is_empty()
            || path.contains(['\\', ':'])
            || path.chars().any(char::is_control)
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    }) {
        return Err("web: shell paths must be relative package paths without traversal".to_owned());
    }
    if web.site_roles.len() > 32 || web.site_roles.keys().any(|role| !identifier(role)) {
        return Err("web.siteRoles: expected at most 32 named roles".to_owned());
    }
    if web.controls.len() > 32 {
        return Err("web.controls: expected at most 32 controls".to_owned());
    }
    let mut ids = BTreeSet::new();
    for control in &web.controls {
        let (id, label) = match control {
            super::WebControl::Toggle { id, label, .. } => (id, label),
            super::WebControl::Choice {
                id,
                label,
                default,
                choices,
            } => {
                let unique = choices.iter().collect::<BTreeSet<_>>();
                if choices.is_empty()
                    || choices.len() > 32
                    || unique.len() != choices.len()
                    || !choices.contains(default)
                    || choices.iter().any(|choice| {
                        choice.trim().is_empty()
                            || choice.len() > 80
                            || choice.chars().any(char::is_control)
                    })
                {
                    return Err(format!(
                        "web.controls.{id}: choices must be unique, bounded, and contain the default"
                    ));
                }
                (id, label)
            }
        };
        if !identifier(id)
            || !ids.insert(id)
            || label.trim().is_empty()
            || label.len() > 80
            || label.chars().any(char::is_control)
        {
            return Err(format!(
                "web.controls.{id}: expected a unique ID and a short label"
            ));
        }
    }
    Ok(())
}

fn effective_definition(
    definition: &Definition,
    target: &str,
    overrides: &Overrides,
) -> Result<Definition, String> {
    validate_metadata(definition, target, overrides)?;
    let mut definition = definition.clone();
    if target == "site"
        && let Some(patch) = definition.targets.get("html").cloned()
    {
        apply(&mut definition, &patch)?;
    }
    if let Some(patch) = definition.targets.get(target).cloned() {
        apply(&mut definition, &patch)?;
    }
    apply(&mut definition, &overrides.shared)?;
    if target == "site"
        && let Some(patch) = overrides.targets.get("html")
    {
        apply(&mut definition, patch)?;
    }
    if let Some(patch) = overrides.targets.get(target) {
        apply(&mut definition, patch)?;
    }
    Ok(definition)
}

pub(crate) fn resolve(
    definition: &Definition,
    target: &str,
    overrides: &Overrides,
) -> Result<ResolvedTheme, String> {
    let definition = effective_definition(definition, target, overrides)?;
    if definition.tokens.len() > 256 {
        return Err("tokens: at most 256 tokens are supported".to_owned());
    }
    let mut resolver = Resolver {
        tokens: &definition.tokens,
        cache: BTreeMap::new(),
        visiting: BTreeSet::new(),
    };
    for (name, token) in &definition.tokens {
        if !identifier(name) {
            return Err(format!("tokens.{name}: invalid token name"));
        }
        context(
            &format!("tokens.{name}"),
            resolver.token(name, token.kind()),
        )?;
    }
    let root = context("root.fontSize", resolver.length(&definition.root.font_size))?;
    if root.unit == Unit::Rem {
        return Err("root.fontSize: the root cannot depend on rem".to_owned());
    }
    let root = root.points(0.0);
    if !(1.0..=1000.0).contains(&root) {
        return Err("root.fontSize: expected 1–1000pt".to_owned());
    }
    let colors = resolve_colors(&mut resolver, &definition.colors)?;
    let styles = resolve_styles(&mut resolver, &definition, root, &colors)?;
    let page = resolve_page(&mut resolver, &definition.page, root)?;
    let fonts = styles
        .values()
        .map(|style| style.font_family.clone())
        .collect();
    Ok(ResolvedTheme {
        root_font_size_pt: root,
        colors,
        styles,
        fonts,
        page,
    })
}

fn resolve_colors(
    resolver: &mut Resolver<'_>,
    source: &super::Colors,
) -> Result<BTreeMap<String, String>, String> {
    let colors = [
        ("ink", &source.ink),
        ("muted", &source.muted),
        ("link", &source.link),
        ("accent", &source.accent),
        ("surface", &source.surface),
        ("paper", &source.paper),
    ]
    .into_iter()
    .map(|(name, value)| {
        context(&format!("colors.{name}"), resolver.color(value))
            .map(|color| (name.to_owned(), color))
    })
    .collect::<Result<BTreeMap<_, _>, _>>()?;
    for name in ["ink", "muted", "link"] {
        if contrast(&colors[name], &colors["paper"]) < 4.5 {
            return Err(format!(
                "colors.{name}: contrast against paper must be at least 4.5:1"
            ));
        }
    }
    Ok(colors)
}

fn resolve_styles(
    resolver: &mut Resolver<'_>,
    definition: &Definition,
    root: f64,
    colors: &BTreeMap<String, String>,
) -> Result<BTreeMap<Role, ResolvedStyle>, String> {
    let body = definition
        .styles
        .get(&Role::Body)
        .ok_or("styles.body: a body style is required")?;
    let font = body
        .font_family
        .as_ref()
        .ok_or("styles.body.fontFamily: a bundled font is required")?;
    let base = ResolvedStyle {
        font_family: context("styles.body.fontFamily", resolver.font(font))?,
        font_size_pt: root,
        font_weight: Weight::Regular,
        italic: false,
        line_height: 1.5,
        color: colors["ink"].clone(),
        background: None,
        space_before_pt: 0.0,
        space_after_pt: 0.0,
        indent_pt: 0.0,
        padding_pt: 0.0,
        border_width_pt: 0.0,
        border_color: colors["muted"].clone(),
        border_style: Stroke::Solid,
        alignment: Alignment::Start,
        keep_with_next: false,
        keep_together: false,
        caption_position: None,
    };
    let mut styles = BTreeMap::<Role, ResolvedStyle>::new();
    for role in Role::ALL {
        let parent = role
            .parent()
            .map_or_else(|| base.clone(), |parent| styles[&parent].clone());
        let style = definition.styles.get(&role).cloned().unwrap_or_default();
        styles.insert(
            role,
            context(
                &format!("styles.{}", role.name()),
                resolve_style(resolver, &style, parent, root),
            )?,
        );
    }
    Ok(styles)
}

fn resolve_page(
    resolver: &mut Resolver<'_>,
    page: &super::Page,
    root: f64,
) -> Result<ResolvedPage, String> {
    let width = context("page.width", resolver.length(&page.width))?;
    let height = context("page.height", resolver.length(&page.height))?;
    if width.unit == Unit::Rem || height.unit == Unit::Rem {
        return Err("page: paper dimensions must be absolute".to_owned());
    }
    let mut dimensions = [width.points(root), height.points(root)];
    dimensions.sort_by(f64::total_cmp);
    let [width_pt, height_pt] = match page.orientation {
        Orientation::Portrait => dimensions,
        Orientation::Landscape => [dimensions[1], dimensions[0]],
    };
    if width_pt <= 0.0 || height_pt <= 0.0 || height_pt.max(width_pt) > 1584.0 {
        return Err("page: dimensions must be positive and at most 22 inches".to_owned());
    }
    let margin_top_pt = nonnegative(
        context("page.marginTop", resolver.distance(&page.margin_top, root))?,
        "page.marginTop",
    )?;
    let margin_right_pt = nonnegative(
        context(
            "page.marginRight",
            resolver.distance(&page.margin_right, root),
        )?,
        "page.marginRight",
    )?;
    let margin_bottom_pt = nonnegative(
        context(
            "page.marginBottom",
            resolver.distance(&page.margin_bottom, root),
        )?,
        "page.marginBottom",
    )?;
    let margin_left_pt = nonnegative(
        context(
            "page.marginLeft",
            resolver.distance(&page.margin_left, root),
        )?,
        "page.marginLeft",
    )?;
    if margin_left_pt + margin_right_pt >= width_pt || margin_top_pt + margin_bottom_pt >= height_pt
    {
        return Err("page: margins leave no usable content area".to_owned());
    }
    let header_distance_pt = nonnegative(
        context(
            "page.headerDistance",
            resolver.distance(&page.header_distance, root),
        )?,
        "page.headerDistance",
    )?;
    let footer_distance_pt = nonnegative(
        context(
            "page.footerDistance",
            resolver.distance(&page.footer_distance, root),
        )?,
        "page.footerDistance",
    )?;
    if header_distance_pt > margin_top_pt || footer_distance_pt > margin_bottom_pt {
        return Err("page: header/footer distances must fit inside their margins".to_owned());
    }
    if !(20..=160).contains(&page.measure_characters) {
        return Err("page.measureCharacters: expected 20–160".to_owned());
    }
    for (name, line) in [("header", &page.header), ("footer", &page.footer)] {
        for slot in [&line.left, &line.center, &line.right] {
            if slot.len() > 16 || slot.iter().any(|part| matches!(part, super::RunningPart::Text(text) if text.len() > 1024 || text.chars().any(char::is_control))) {
                return Err(format!("page.{name}: running text is too long or contains control characters"));
            }
        }
    }
    Ok(ResolvedPage {
        width_pt,
        height_pt,
        margin_top_pt,
        margin_right_pt,
        margin_bottom_pt,
        margin_left_pt,
        header_distance_pt,
        footer_distance_pt,
        chapter_start: page.chapter_start,
        header: page.header.clone(),
        footer: page.footer.clone(),
        measure_characters: page.measure_characters,
    })
}
