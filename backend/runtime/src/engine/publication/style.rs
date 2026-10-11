// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::engine::publication::theme::{ThemeSettings, validate_settings};

/// Compile the same validated semantic values for the HTML and site targets.
/// The renderer owns the CSS slot; these values never contain authored text.
pub(crate) fn css_variables(settings: &ThemeSettings) -> Result<String, String> {
    validate_settings(settings)?;
    let colors = &settings.colors;
    let typography = &settings.typography;
    let layout = &settings.layout;
    Ok(format!(
        ":root {{\n  --publication-ink: {};\n  --publication-muted: {};\n  --publication-link: {};\n  --publication-accent: {};\n  --publication-surface: {};\n  --publication-paper: {};\n  --publication-body-font: {};\n  --publication-heading-font: {};\n  --publication-body-scale: {};\n  --publication-leading: {};\n  --publication-measure: {}ch;\n  --publication-spacing: {};\n}}\n",
        colors.ink,
        colors.muted,
        colors.link,
        colors.accent,
        colors.surface,
        colors.paper,
        css_font(&typography.body_family)?,
        css_font(&typography.heading_family)?,
        decimal(typography.body_scale),
        decimal(typography.leading),
        layout.measure_ch,
        decimal(layout.spacing),
    ))
}

pub(crate) fn css_stylesheet(settings: &ThemeSettings) -> Result<String, String> {
    let mut css = css_variables(settings)?;
    css.push_str(
        "* { box-sizing: border-box; }\n\
html { scroll-behavior: smooth; }\n\
body { margin: 0 auto; padding: clamp(1.5rem, 4vw, 3rem) 1.25rem 5rem; max-width: calc(var(--publication-measure) + 2.5rem); color: var(--publication-ink); background: var(--publication-paper); font-family: var(--publication-body-font); font-size: calc(1rem * var(--publication-body-scale)); line-height: var(--publication-leading); }\n\
main { max-width: var(--publication-measure); margin-inline: auto; }\n\
h1, h2, h3, h4, h5, h6 { font-family: var(--publication-heading-font); color: var(--publication-ink); line-height: 1.25; margin: calc(1.7em * var(--publication-spacing)) 0 .55em; text-wrap: balance; }\n\
article > header h1, main > h1 { margin-top: 0; }\n\
p, blockquote, pre, table, figure, ul, ol, aside { margin: 0 0 calc(1.1em * var(--publication-spacing)); }\n\
li > p { margin-bottom: 0; }\n\
a { color: var(--publication-link); text-underline-offset: .15em; }\n\
a:focus-visible { outline: 3px solid var(--publication-accent); outline-offset: 3px; }\n\
img { display: block; max-width: 100%; height: auto; }\n\
blockquote { border-inline-start: .2rem solid var(--publication-accent); padding-inline-start: 1rem; }\n\
pre { overflow-x: auto; padding: 1rem; background: var(--publication-surface); border-radius: .35rem; }\n\
code { font-family: 'IBM Plex Mono', monospace; }\n\
table { width: 100%; table-layout: fixed; border-collapse: collapse; }\n\
figure { margin-inline: 0; }\n\
caption, figcaption { text-align: start; color: var(--publication-muted); font-size: .9em; margin-block: .4em; overflow-wrap: anywhere; }\n\
td, th { overflow-wrap: anywhere; }\n\
th, td { border: 1px solid var(--publication-muted); padding: .4rem .6rem; text-align: start; vertical-align: top; }\n\
th, .callout { background: var(--publication-surface); }\n\
.callout { border-inline-start: .2rem solid var(--publication-accent); padding: .75rem 1rem; }\n\
.embedded-passage { border-inline-start: .15rem solid var(--publication-accent); padding-inline-start: 1rem; }\n\
.footnotes { border-block-start: 1px solid var(--publication-muted); margin-block-start: 3rem; font-size: .9em; }\n\
.publication-masthead { display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; padding-bottom: .75rem; margin-bottom: 2rem; border-bottom: .2rem solid var(--publication-accent); font-family: var(--publication-heading-font); }\n\
.publication-masthead > span { color: var(--publication-muted); font-size: .75em; letter-spacing: .1em; text-transform: uppercase; }\n\
.publication--site { max-width: calc(var(--publication-measure) + 24rem); display: grid; grid-template-columns: minmax(12rem, 18rem) minmax(0, 1fr); gap: clamp(1.5rem, 4vw, 3.5rem); align-items: start; }\n\
.publication--site.publication--no-sidebar { grid-template-columns: minmax(0, 1fr); max-width: calc(var(--publication-measure) + 2.5rem); }\n\
.publication--site > nav { position: sticky; top: 1rem; max-height: calc(100vh - 2rem); overflow: auto; padding: 1rem; background: var(--publication-surface); border-radius: .5rem; }\n\
.publication--site > main { min-width: 0; width: 100%; }\n\
.publication--editorial.publication--site { display: block; }\n\
.publication--editorial .publication-columns { display: grid; grid-template-columns: minmax(12rem, 18rem) minmax(0, 1fr); gap: clamp(1.5rem, 4vw, 3.5rem); align-items: start; }\n\
.publication--editorial.publication--no-sidebar .publication-columns { grid-template-columns: minmax(0, 1fr); }\n\
.publication--editorial .publication-columns > nav { position: sticky; top: 1rem; max-height: calc(100vh - 2rem); overflow: auto; padding: 1rem; background: var(--publication-surface); border-radius: .5rem; }\n\
.publication--editorial .publication-columns > main { min-width: 0; width: 100%; }\n\
.publication-contents, .publication-contents ol { list-style: none; margin: 0; padding: 0; }\n\
.publication-contents ol { padding-inline-start: 1rem; margin-top: .3rem; border-inline-start: 1px solid var(--publication-muted); }\n\
.publication-contents li { margin: .3rem 0; }\n\
.publication-contents a { display: block; padding: .2rem .35rem; line-height: 1.35; text-decoration: none; border-radius: .25rem; }\n\
.publication-contents a:hover, .publication-contents a[aria-current=page] { color: var(--publication-ink); background: var(--publication-paper); }\n\
.publication-contents a[aria-current=page] { font-weight: 700; }\n\
.publication-search { margin-bottom: 1rem; padding-bottom: 1rem; border-bottom: 1px solid var(--publication-muted); }\n\
.publication-search label { display: block; margin-bottom: .4rem; font-size: .82em; font-weight: 700; }\n\
.publication-search input { width: 100%; min-height: 2.4rem; padding: .45rem .65rem; border: 1px solid var(--publication-muted); border-radius: .4rem; background: var(--publication-paper); color: var(--publication-ink); font: inherit; }\n\
.publication-search input:focus-visible { outline: 3px solid var(--publication-accent); outline-offset: 2px; }\n\
.publication-search [role=status] p { margin: .5rem 0 .25rem; font-size: .8em; color: var(--publication-muted); }\n\
.publication-search [role=status] ol { list-style: none; margin: 0; padding: 0; max-height: 16rem; overflow: auto; }\n\
.publication-search [role=status] a { display: block; padding: .25rem 0; font-size: .86em; line-height: 1.35; }\n\
.publication-page-contents { margin: 0 0 2rem; padding: .8rem 1rem; background: var(--publication-surface); border-radius: .5rem; }\n\
.publication-page-contents h2 { margin: 0 0 .35rem; font-family: var(--publication-body-font); font-size: .82em; font-weight: 700; }\n\
.publication-page-contents ol { display: flex; flex-wrap: wrap; gap: .15rem .75rem; list-style: none; margin: 0; padding: 0; }\n\
.publication-page-contents li { margin: 0; }\n\
.publication-page-contents a { font-size: .9em; text-decoration: none; }\n\
.publication-page-contents a:hover { text-decoration: underline; }\n\
.publication-listing h1 { margin-bottom: 1.25rem; }\n\
.publication-card-list { list-style: none; padding: 0; margin: 0 0 2rem; }\n\
.publication-card-list > li { margin: 0; padding: 1rem 0; border-bottom: 1px solid var(--publication-surface); }\n\
.publication-card-list > li:first-child { border-top: 1px solid var(--publication-surface); }\n\
.publication-card-list h2 { margin: 0 0 .25rem; font-size: 1.18em; line-height: 1.35; }\n\
.publication-card-list p { margin: .3rem 0 0; color: var(--publication-muted); }\n\
.publication-card-list time { display: block; color: var(--publication-muted); font-size: .85em; font-variant-numeric: tabular-nums; }\n\
.publication-card-tags { display: flex; flex-wrap: wrap; gap: .25rem .75rem; list-style: none; padding: 0; margin: .5rem 0 0; font-size: .85em; }\n\
.publication-card-tags li { margin: 0; }\n\
.publication-pagination { display: flex; flex-wrap: wrap; gap: .4rem; margin: 1.5rem 0 2rem; }\n\
.publication-pagination a { display: grid; place-items: center; min-width: 2.25rem; min-height: 2.25rem; padding: .2rem .5rem; border-radius: .3rem; text-decoration: none; }\n\
.publication-pagination a:hover, .publication-pagination a[aria-current=page] { background: var(--publication-surface); color: var(--publication-ink); }\n\
.publication-archive-links { padding: 1rem; background: var(--publication-surface); border-radius: .5rem; }\n\
.publication-archive-links h2 { margin: 0 0 .5rem; font-size: 1.05em; }\n\
.publication-archive-links ul { margin: 0; }\n\
.skip-link { position: absolute; top: -10rem; left: 1rem; padding: .5rem 1rem; background: var(--publication-paper); }\n\
.skip-link:focus { top: 1rem; }\n\
@media (max-width: 48rem) { .publication--site, .publication--editorial .publication-columns { display: block; } .publication--site > nav, .publication--editorial .publication-columns > nav { position: static; max-height: 18rem; margin-bottom: 2rem; } }\n\
@media print { body { max-width: none; padding: 0; } a { color: inherit; } }\n",
    );
    Ok(css)
}

/// Emit only literals from validated settings. Content and theme code are
/// included by separate renderer steps and cannot inject Typst syntax here.
pub(crate) fn typst_values(settings: &ThemeSettings) -> Result<String, String> {
    validate_settings(settings)?;
    let colors = &settings.colors;
    let typography = &settings.typography;
    let layout = &settings.layout;
    let body_size = 11.0 * typography.body_scale;
    let leading = typography.leading - 1.0;
    Ok(format!(
        "#let ink = rgb(\"{}\")\n#let muted = rgb(\"{}\")\n#let link-color = rgb(\"{}\")\n#let accent = rgb(\"{}\")\n#let surface = rgb(\"{}\")\n#let paper = rgb(\"{}\")\n#let body-font = \"{}\"\n#let heading-font = \"{}\"\n#let body-size = {}pt\n#let leading = {}em\n#let measure = {}\n#let spacing = {}\n",
        colors.ink,
        colors.muted,
        colors.link,
        colors.accent,
        colors.surface,
        colors.paper,
        crate::engine::publication::fonts::family(&typography.body_family)?,
        crate::engine::publication::fonts::family(&typography.heading_family)?,
        decimal(body_size),
        decimal(leading),
        layout.measure_ch,
        decimal(layout.spacing),
    ))
}

fn css_font(id: &str) -> Result<String, String> {
    serde_json::to_string(crate::engine::publication::fonts::family(id)?)
        .map_err(|error| error.to_string())
}

fn decimal(value: f64) -> String {
    format!("{value:.3}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{css_stylesheet, css_variables, typst_values};
    use crate::engine::publication::plan::PublicationFormat;
    use crate::engine::publication::theme::capture_theme;

    #[test]
    fn shared_semantic_roles_compile_for_both_targets() {
        let root = Path::new("/unused-bluink-theme-root");
        let theme = capture_theme(root, "editorial", PublicationFormat::Pdf).unwrap();
        let css = css_variables(&theme.settings).unwrap();
        let typst = typst_values(&theme.settings).unwrap();
        assert!(css.contains("--publication-accent: #a2453a;"));
        assert!(css.contains("--publication-heading-font: \"Inter\""));
        assert!(css.contains("--publication-measure: 68ch;"));
        assert!(
            css_stylesheet(&theme.settings)
                .unwrap()
                .contains("color: var(--publication-link)")
        );
        assert!(typst.contains("#let accent = rgb(\"#a2453a\")"));
        assert!(typst.contains("#let heading-font = \"Inter\""));
        assert!(typst.contains("#let measure = 68"));
        let mut invalid = theme.settings;
        invalid.colors.ink = "#222222; } body { display: none".to_owned();
        assert!(css_variables(&invalid).is_err());
        assert!(typst_values(&invalid).is_err());
    }

    #[test]
    fn builtin_pdf_shell_compiles_with_shared_values_when_typst_is_packaged() {
        let binary = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "linux-x64/typst"
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "windows-x64/typst.exe"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-arm64/typst"
        } else {
            return;
        };
        let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");
        let typst_binary = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../typst/assets")
            .join(binary);
        if !typst_binary.is_file() {
            return;
        }
        let temp = std::env::temp_dir().join(format!("bluink-theme-pdf-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&temp).unwrap();
        for id in ["classic", "editorial"] {
            let theme = capture_theme(
                Path::new("/unused-bluink-theme-root"),
                id,
                PublicationFormat::Pdf,
            )
            .unwrap();
            std::fs::write(
                temp.join("theme-values.typ"),
                typst_values(&theme.settings).unwrap(),
            )
            .unwrap();
            std::fs::write(
                temp.join("content.typ"),
                "= Sample chapter\n\nA short paragraph.\n",
            )
            .unwrap();
            std::fs::copy(
                vendor.join(format!("themes/{id}/pdf/document.typ")),
                temp.join("document.typ"),
            )
            .unwrap();
            let output = std::process::Command::new(&typst_binary)
                .arg("compile")
                .arg(temp.join("document.typ"))
                .arg(temp.join("document.pdf"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Typst rejected {id}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                std::fs::read(temp.join("document.pdf"))
                    .unwrap()
                    .starts_with(b"%PDF-")
            );
        }
        std::fs::remove_dir_all(temp).unwrap();
    }
}
