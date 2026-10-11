// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Resolve the shared @bluink/fonts catalogue and package offline HTML assets.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

use crate::engine::publication::theme::ThemeSettings;

#[derive(Deserialize)]
struct Font {
    id: String,
    family: String,
}

pub(crate) fn family(id: &str) -> Result<&'static str, String> {
    static FONTS: OnceLock<Vec<Font>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../../../fonts/catalogue.json"))
                .expect("valid bundled catalogue")
        })
        .iter()
        .find(|font| font.id == id)
        .map(|font| font.family.as_str())
        .ok_or_else(|| format!("Unknown bundled font: {id}"))
}
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
fn resources(
    settings: &ThemeSettings,
    web: &Path,
) -> Result<(String, BTreeMap<String, Vec<u8>>), String> {
    let mut css = String::new();
    let mut assets = BTreeMap::new();
    let mut ids = vec![
        settings.typography.body_family.as_str(),
        settings.typography.heading_family.as_str(),
        "ibm-plex-mono",
    ];
    ids.sort_unstable();
    ids.dedup();
    let urls = regex::Regex::new(r"url\('\./([^']+)'\)").expect("valid font URL expression");
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
        .to_ascii_lowercase()
        .find("</head>")
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
pub(crate) fn embed_html(
    html: &str,
    settings: &ThemeSettings,
    web: &Path,
) -> Result<String, String> {
    let (mut css, assets) = resources(settings, web)?;
    for (relative, bytes) in assets {
        css = css.replace(
            &format!("./{relative}"),
            &format!("data:font/woff2;base64,{}", STANDARD.encode(bytes)),
        );
    }
    add_style(html, &css)
}

/// A website stores each selected font once, with URLs relative to each page.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
pub(crate) fn add_site(
    files: &mut BTreeMap<String, Vec<u8>>,
    settings: &ThemeSettings,
    web: &Path,
) -> Result<(), String> {
    let (css, assets) = resources(settings, web)?;
    for (filename, bytes) in files.iter_mut().filter(|(name, _)| name.ends_with(".html")) {
        let depth = Path::new(filename)
            .parent()
            .map_or(0, |parent| parent.components().count());
        let css = css.replace("url('./", &format!("url('{}fonts/", "../".repeat(depth)));
        let html =
            std::str::from_utf8(bytes).map_err(|error| format!("Invalid HTML page: {error}"))?;
        *bytes = add_style(html, &css)?.into_bytes();
    }
    for (relative, bytes) in assets {
        files.insert(format!("fonts/{relative}"), bytes);
    }
    // Font licence notices travel with generated websites too.
    for id in [
        &settings.typography.body_family,
        &settings.typography.heading_family,
        &"ibm-plex-mono".to_owned(),
    ] {
        let license = std::fs::read(web.join("licenses").join(format!("{id}.txt")))
            .map_err(|error| format!("Cannot read font licence {id}: {error}"))?;
        files.insert(format!("fonts/licenses/{id}.txt"), license);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::publication::plan::PublicationFormat;
    use crate::engine::publication::style::{css_variables, typst_values};
    use crate::engine::publication::theme::capture_theme;

    #[test]
    fn shared_catalogue_drives_both_formats_and_rejects_unknown_ids() {
        let mut theme =
            capture_theme(Path::new("/unused"), "classic", PublicationFormat::Html).unwrap();
        let fonts: Vec<Font> =
            serde_json::from_str(include_str!("../../../../../fonts/catalogue.json")).unwrap();
        for font in fonts {
            theme.settings.typography.body_family = font.id.clone();
            theme.settings.typography.heading_family = font.id;
            assert!(
                css_variables(&theme.settings)
                    .unwrap()
                    .contains(&font.family)
            );
            assert!(
                typst_values(&theme.settings)
                    .unwrap()
                    .contains(&font.family)
            );
        }
        theme.settings.typography.body_family = "../unknown".to_owned();
        assert!(css_variables(&theme.settings).is_err());
        assert!(typst_values(&theme.settings).is_err());
    }

    #[test]
    fn html_is_self_contained_and_site_fonts_are_shared_with_relative_urls() {
        let theme =
            capture_theme(Path::new("/unused"), "editorial", PublicationFormat::Html).unwrap();
        let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web");
        let html = "<!doctype html><html><head></head><body>Text</body></html>";
        let document = embed_html(html, &theme.settings, &web).unwrap();
        assert!(document.contains("data:font/woff2;base64,d09GMg"));
        assert!(document.contains("SIL OPEN FONT LICENSE"));
        assert!(!document.contains("url('./"));
        assert!(embed_html(html, &theme.settings, Path::new("/missing-font-package")).is_err());
        let mut site = BTreeMap::from([
            ("index.html".to_owned(), html.as_bytes().to_vec()),
            ("chapters/page.html".to_owned(), html.as_bytes().to_vec()),
        ]);
        add_site(&mut site, &theme.settings, &web).unwrap();
        assert!(String::from_utf8_lossy(&site["index.html"]).contains("url('fonts/"));
        assert!(String::from_utf8_lossy(&site["chapters/page.html"]).contains("url('../fonts/"));
        assert!(site.contains_key("fonts/licenses/inter.txt"));
        assert!(!site.keys().any(|key| key.contains("literata")));
    }
}
