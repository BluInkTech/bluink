// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::{fonts::family, theme_format::ResolvedTheme};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
pub(crate) fn resources<'a>(
    font_ids: impl IntoIterator<Item = &'a str>,
    web: &Path,
) -> Result<(String, BTreeMap<String, Vec<u8>>), String> {
    static URLS: OnceLock<Result<regex::Regex, String>> = OnceLock::new();
    let mut css = String::new();
    let mut assets = BTreeMap::new();
    let mut ids = font_ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    let urls = URLS
        .get_or_init(|| {
            regex::Regex::new(r"url\('\./([^']+)'\)").map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    for id in ids {
        family(id)?;
        let stylesheet = std::fs::read_to_string(web.join(format!("{id}.css")))
            .map_err(|error| format!("Cannot read packaged font {id}: {error}"))?;
        for capture in urls.captures_iter(&stylesheet) {
            let relative = &capture[1];
            let path = Path::new(relative);
            if !path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
                || !relative.ends_with(".woff2")
            {
                return Err("Unsafe packaged font URL.".to_owned());
            }
            let bytes = std::fs::read(web.join(path))
                .map_err(|error| format!("Cannot read packaged font asset {relative}: {error}"))?;
            if !bytes.starts_with(b"wOF2") {
                return Err(format!("Invalid packaged web font: {relative}"));
            }
            assets.insert(relative.to_owned(), bytes);
        }
        css.push_str(&stylesheet);
    }
    Ok((css, assets))
}

fn add_style(html: &str, css: &str) -> Result<String, String> {
    let offset = html
        .as_bytes()
        .windows(7)
        .position(|window| window.eq_ignore_ascii_case(b"</head>"))
        .ok_or("HTML theme must provide a head element for offline fonts.")?;
    let mut output = String::with_capacity(html.len() + css.len() + 16);
    output.push_str(&html[..offset]);
    output.push_str("<style data-bluink-fonts>");
    output.push_str(css);
    output.push_str("</style>");
    output.push_str(&html[offset..]);
    Ok(output)
}

/// A downloaded HTML document carries its selected fonts without network access.
pub(crate) fn embed_html(html: &str, theme: &ResolvedTheme, web: &Path) -> Result<String, String> {
    embed_html_fonts(html, theme.fonts.iter().map(String::as_str), web)
}

/// Embed every font selected by resolved semantic roles, including custom role fonts.
pub(crate) fn embed_html_fonts<'a>(
    html: &str,
    font_ids: impl IntoIterator<Item = &'a str>,
    web: &Path,
) -> Result<String, String> {
    let (mut css, assets) = resources(font_ids, web)?;
    for (relative, bytes) in assets {
        css = css.replace(
            &format!("./{relative}"),
            &format!("data:font/woff2;base64,{}", STANDARD.encode(bytes)),
        );
    }
    add_style(html, &css)
}

/// A website stores each selected font once, with URLs relative to each page.
pub(crate) struct SiteFonts {
    css: String,
}

impl SiteFonts {
    /// The fonts' styles, plus the font and licence files keyed by site path.
    pub(crate) fn prepare(
        theme: &ResolvedTheme,
        web: &Path,
    ) -> Result<(Self, BTreeMap<String, Vec<u8>>), String> {
        let (css, assets) = resources(theme.fonts.iter().map(String::as_str), web)?;
        let mut files = assets
            .into_iter()
            .map(|(relative, bytes)| (format!("fonts/{relative}"), bytes))
            .collect::<BTreeMap<_, _>>();
        // Font licence notices travel with generated websites too.
        for id in &theme.fonts {
            let license = std::fs::read(web.join("licenses").join(format!("{id}.txt")))
                .map_err(|error| format!("Cannot read font licence {id}: {error}"))?;
            files.insert(format!("fonts/licenses/{id}.txt"), license);
        }
        Ok((Self { css }, files))
    }

    /// Add the font styles to the page at `route`, with URLs relative to its folder.
    pub(crate) fn style(&self, route: &str, html: &str) -> Result<String, String> {
        let depth = Path::new(route)
            .parent()
            .map_or(0, |parent| parent.components().count());
        let css = self
            .css
            .replace("url('./", &format!("url('{}fonts/", "../".repeat(depth)));
        add_style(html, &css)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::publication::fonts::Font;
    use crate::domain::publication::plan::PublicationFormat;
    use crate::domain::publication::theme_format::{Role, css, typst};
    use crate::export_jobs::theme::capture_theme;

    #[test]
    fn shared_catalogue_drives_both_formats_and_rejects_unknown_ids() {
        let mut theme =
            capture_theme(Path::new("/unused"), "classic", PublicationFormat::Html).unwrap();
        let fonts: Vec<Font> =
            serde_json::from_str(include_str!("../../../fonts/catalogue.json")).unwrap();
        for font in fonts {
            theme
                .resolved
                .styles
                .get_mut(&Role::Body)
                .unwrap()
                .font_family = font.id.clone();
            theme
                .resolved
                .styles
                .get_mut(&Role::Heading1)
                .unwrap()
                .font_family = font.id;
            assert!(
                css::stylesheet(&theme.resolved, "Test")
                    .unwrap()
                    .contains(&font.family)
            );
            assert!(
                typst::values(&theme.resolved)
                    .unwrap()
                    .contains(&font.family)
            );
        }
        theme
            .resolved
            .styles
            .get_mut(&Role::Body)
            .unwrap()
            .font_family = "../unknown".to_owned();
        assert!(css::stylesheet(&theme.resolved, "Test").is_err());
        assert!(typst::values(&theme.resolved).is_err());
    }

    #[test]
    fn html_is_self_contained_and_site_fonts_are_shared_with_relative_urls() {
        let theme =
            capture_theme(Path::new("/unused"), "editorial", PublicationFormat::Html).unwrap();
        let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web");
        let html = "<!doctype html><html><head></head><body>Text</body></html>";
        let document = embed_html(html, &theme.resolved, &web).unwrap();
        assert!(document.contains("data:font/woff2;base64,d09GMg"));
        assert!(document.contains("SIL OPEN FONT LICENSE"));
        assert!(!document.contains("url('./"));
        assert!(embed_html(html, &theme.resolved, Path::new("/missing-font-package")).is_err());
        let (fonts, files) = SiteFonts::prepare(&theme.resolved, &web).unwrap();
        assert!(
            fonts
                .style("index.html", html)
                .unwrap()
                .contains("url('fonts/")
        );
        assert!(
            fonts
                .style("chapters/page.html", html)
                .unwrap()
                .contains("url('../fonts/")
        );
        assert!(files.contains_key("fonts/licenses/inter.txt"));
        assert!(!files.keys().any(|key| key.contains("literata")));
    }
}
