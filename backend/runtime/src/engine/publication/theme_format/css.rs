// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! CSS serialization consumes resolved values; it never evaluates authored tokens.

use std::fmt::Write;

use super::{
    Alignment, CaptionPosition, ResolvedStyle, ResolvedTheme, Role, RunningLine, RunningPart,
    Stroke, Weight,
};

/// Reflowable books retain semantic theme roles without setting the reader's
/// root size, page geometry, running headers, or browser application layout.
#[expect(
    clippy::unwrap_used,
    reason = "Formatting primitive values into String is infallible."
)]
pub(crate) fn epub_stylesheet(theme: &ResolvedTheme) -> Result<String, String> {
    let root = theme.root_font_size_pt;
    let mut css =
        String::from("body { margin: 0; }\n.publication-content { overflow-wrap: break-word; }\n");
    css.push_str("body {\n");
    text(&mut css, &theme.styles[&Role::Body], root)?;
    css.push_str("}\n");
    for role in Role::ALL {
        let selectors = match role {
            Role::Body => ".publication-content p".into(),
            Role::List => ".publication-content ul, .publication-content ol".into(),
            Role::Caption => ".publication-content caption, .publication-content figcaption".into(),
            _ => selector(role),
        };
        writeln!(css, "{selectors} {{").unwrap();
        let style = &theme.styles[&role];
        if role != Role::Body {
            text(&mut css, style, root)?;
        }
        decorations(&mut css, style, root);
        if !matches!(role, Role::Link | Role::InlineCode) {
            writeln!(
                css,
                "margin: {}rem 0 {}rem {}rem; page-break-after: {}; page-break-inside: {};",
                decimal(style.space_before_pt / root),
                decimal(style.space_after_pt / root),
                decimal(style.indent_pt / root),
                if style.keep_with_next {
                    "avoid"
                } else {
                    "auto"
                },
                if style.keep_together { "avoid" } else { "auto" }
            )
            .unwrap();
        }
        if role == Role::Caption {
            writeln!(
                css,
                "caption-side: {};",
                if style.caption_position == Some(CaptionPosition::Before) {
                    "top"
                } else {
                    "bottom"
                }
            )
            .unwrap();
        }
        css.push_str("}\n");
    }
    css.push_str("img { max-width: 100%; height: auto; }\nfigure { margin-left: 0; margin-right: 0; }\npre { white-space: pre-wrap; overflow-wrap: break-word; }\npre code { font: inherit; }\ntable { width: 100%; table-layout: fixed; border-collapse: collapse; }\nth, td { vertical-align: top; overflow-wrap: break-word; }\nli > p:last-child, td > p:last-child, th > p:last-child { margin-bottom: 0; }\n");
    Ok(css)
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(crate) fn base_stylesheet(theme: &ResolvedTheme) -> Result<String, String> {
    let mut css = format!(
        ":root {{ font-size: {}pt;\n",
        decimal(theme.root_font_size_pt)
    );
    for (name, color) in &theme.colors {
        writeln!(css, "  --publication-{name}: {color};").unwrap();
    }
    writeln!(
        css,
        "  --publication-measure: {}ch;\n}}",
        theme.page.measure_characters
    )
    .unwrap();
    writeln!(
        css,
        ":root {{ --publication-body-font: {}; --publication-heading-font: {}; }}",
        css_string(crate::engine::publication::fonts::family(
            &theme.styles[&Role::Body].font_family
        )?),
        css_string(crate::engine::publication::fonts::family(
            &theme.styles[&Role::Heading1].font_family
        )?)
    )
    .unwrap();
    css.push_str("* { box-sizing: border-box; }\nbody { margin: 0 auto; max-width: calc(var(--publication-measure) + 2.5rem); padding: 2rem 1.25rem 4rem; background: var(--publication-paper); }\n");
    css.push_str("body {\n");
    text(
        &mut css,
        &theme.styles[&Role::Body],
        theme.root_font_size_pt,
    )?;
    css.push_str("}\n.publication-content p {\n");
    layout(
        &mut css,
        &theme.styles[&Role::Body],
        theme.root_font_size_pt,
    );
    css.push_str("}\n");
    for role in Role::ALL.into_iter().filter(|role| *role != Role::Body) {
        writeln!(css, "/* {} */\n{} {{", role.name(), selector(role)).unwrap();
        let style = &theme.styles[&role];
        text(&mut css, style, theme.root_font_size_pt)?;
        if matches!(role, Role::Link | Role::InlineCode) {
            decorations(&mut css, style, theme.root_font_size_pt);
        } else {
            layout(&mut css, style, theme.root_font_size_pt);
        }
        if role == Role::Caption {
            writeln!(
                css,
                "  caption-side: {};",
                if style.caption_position == Some(CaptionPosition::Before) {
                    "top"
                } else {
                    "bottom"
                }
            )
            .unwrap();
        }
        if role == Role::Rule {
            let stroke = match style.border_style {
                Stroke::Solid => "solid",
                Stroke::Dashed => "dashed",
                Stroke::Dotted => "dotted",
            };
            writeln!(
                css,
                "  border: 0;\n  border-block-start: {}rem {stroke} {};",
                decimal(style.border_width_pt / theme.root_font_size_pt),
                style.border_color
            )
            .unwrap();
        }
        css.push_str("}\n");
    }
    css.push_str(".publication-content { overflow-wrap: anywhere; }\n.publication-content :is(h1,h2,h3,h4,h5,h6) { text-wrap: balance; }\n.publication-content img { display: block; max-width: 100%; object-fit: contain; margin-inline: auto; }\n.publication-content pre { overflow-x: auto; white-space: pre; }\n.publication-content pre code { font: inherit; }\n.publication-content table { width: 100%; table-layout: fixed; border-collapse: collapse; }\n.publication-content :is(th,td) { vertical-align: top; }\n.publication-content :is(li,th,td) > p:last-child { margin-block-end: 0; }\n.publication-content figure { display: flex; flex-direction: column; margin-inline: 0; }\n.publication-content figcaption { overflow-wrap: anywhere; }\n.publication-content a:focus-visible { outline: 2px solid var(--publication-accent); outline-offset: 2px; }\n.publication-chapter:not(.publication-chapter--first) > header > :is(h1,h2,h3,h4,h5,h6), .publication-section > :is(h1,h2,h3,h4,h5,h6) { string-set: bluink-chapter content(text); }\n");
    let caption_order =
        if theme.styles[&Role::Caption].caption_position == Some(CaptionPosition::Before) {
            -1
        } else {
            1
        };
    writeln!(
        css,
        ".publication-content figcaption {{ order: {caption_order}; }}"
    )
    .unwrap();
    Ok(css)
}

#[cfg(test)]
pub(crate) fn stylesheet(theme: &ResolvedTheme, title: &str) -> Result<String, String> {
    let mut css = base_stylesheet(theme)?;
    append_page(&mut css, theme, title);
    Ok(css)
}

pub(crate) fn append_page(css: &mut String, theme: &ResolvedTheme, title: &str) {
    page(css, theme, title);
}

fn selector(role: Role) -> String {
    let semantic = match role {
        Role::Heading1 => "h1",
        Role::Heading2 => "h2",
        Role::Heading3 => "h3",
        Role::Heading4 => "h4",
        Role::Heading5 => "h5",
        Role::Heading6 => "h6",
        Role::Link => "a",
        Role::Quote => "blockquote",
        Role::Code => "pre",
        Role::InlineCode => ":not(pre) > code",
        Role::List => ":is(ul,ol)",
        Role::Table => "table",
        Role::TableHeader => "th",
        Role::TableCell => "td",
        Role::Caption => ":is(caption,figcaption)",
        Role::Footnote => "[role=doc-footnote]",
        Role::Embed => ".embedded-passage",
        Role::Rule => "hr",
        Role::Box => ".callout",
        Role::BoxHeading => ".callout-title",
        Role::BoxInfo => ".callout[data-kind=info]",
        Role::BoxAbstract => ".callout[data-kind=abstract]",
        Role::BoxNote => ".callout[data-kind=note]",
        Role::BoxTip => ".callout[data-kind=tip]",
        Role::BoxSuccess => ".callout[data-kind=success]",
        Role::BoxImportant => ".callout[data-kind=important]",
        Role::BoxWarning => ".callout[data-kind=warning]",
        Role::BoxError => ".callout[data-kind=error]",
        Role::BoxQuestion => ".callout[data-kind=question]",
        Role::BoxExample => ".callout[data-kind=example]",
        Role::Body => unreachable!(),
    };
    format!(
        ".publication-content {semantic}, .publication-content [data-bluink-role=\"{}\"]",
        role.name()
    )
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn text(css: &mut String, style: &ResolvedStyle, root: f64) -> Result<(), String> {
    let family = css_string(crate::engine::publication::fonts::family(
        &style.font_family,
    )?);
    let weight = if style.font_weight == Weight::Bold {
        700
    } else {
        400
    };
    let italic = if style.italic { "italic" } else { "normal" };
    let alignment = match style.alignment {
        Alignment::Start => "start",
        Alignment::Center => "center",
        Alignment::End => "end",
        Alignment::Justify => "justify",
    };
    writeln!(css, "  font-family: {family};\n  font-size: {}rem;\n  font-weight: {weight};\n  font-style: {italic};\n  line-height: {};\n  color: {};\n  text-align: {alignment};", decimal(style.font_size_pt / root), decimal(style.line_height), style.color).unwrap();
    Ok(())
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn layout(css: &mut String, style: &ResolvedStyle, root: f64) {
    writeln!(css, "  margin-block: {}rem {}rem;\n  margin-inline: {}rem 0;\n  break-after: {};\n  break-inside: {};", decimal(style.space_before_pt / root), decimal(style.space_after_pt / root), decimal(style.indent_pt / root), if style.keep_with_next { "avoid" } else { "auto" }, if style.keep_together { "avoid" } else { "auto" }).unwrap();
    decorations(css, style, root);
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn decorations(css: &mut String, style: &ResolvedStyle, root: f64) {
    let stroke = match style.border_style {
        Stroke::Solid => "solid",
        Stroke::Dashed => "dashed",
        Stroke::Dotted => "dotted",
    };
    writeln!(
        css,
        "  padding: {}rem;\n  border: {}rem {stroke} {};\n  background-color: {};",
        decimal(style.padding_pt / root),
        decimal(style.border_width_pt / root),
        style.border_color,
        style.background.as_deref().unwrap_or("transparent")
    )
    .unwrap();
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn page(css: &mut String, theme: &ResolvedTheme, title: &str) {
    let page = &theme.page;
    writeln!(
        css,
        "@page {{\n  size: {}pt {}pt;\n  margin: {}pt {}pt {}pt {}pt;",
        decimal(page.width_pt),
        decimal(page.height_pt),
        decimal(page.margin_top_pt),
        decimal(page.margin_right_pt),
        decimal(page.margin_bottom_pt),
        decimal(page.margin_left_pt)
    )
    .unwrap();
    running(css, "top", &page.header, title, page.header_distance_pt);
    running(css, "bottom", &page.footer, title, page.footer_distance_pt);
    css.push_str("}\n@page:first {\n");
    for (edge, line) in [("top", &page.header), ("bottom", &page.footer)] {
        if !line.show_on_first_page {
            for slot in ["left", "center", "right"] {
                writeln!(css, "  @{edge}-{slot} {{ content: none; }}").unwrap();
            }
        }
    }
    css.push_str("}\n@media print {\n  body { max-width: none; padding: 0; }\n  .publication-content { max-width: none; }\n  .publication-navigation { display: none; }\n  nav.publication-contents { break-before: page; }\n");
    if page.chapter_start == super::ChapterStart::NextPage {
        css.push_str("  .publication-chapter { break-before: page; }\n  .publication-chapter--first { break-before: auto; }\n");
    }
    css.push_str("}\n");
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn running(css: &mut String, edge: &str, line: &RunningLine, title: &str, distance: f64) {
    for (slot, parts) in [
        ("left", &line.left),
        ("center", &line.center),
        ("right", &line.right),
    ] {
        let content = if parts.is_empty() {
            "none".to_owned()
        } else {
            parts
                .iter()
                .map(|part| match part {
                    RunningPart::Text(text) => css_string(text),
                    RunningPart::DocumentTitle => css_string(title),
                    RunningPart::ChapterTitle => "string(bluink-chapter)".to_owned(),
                    RunningPart::PageNumber => "counter(page)".to_owned(),
                    RunningPart::PageCount => "counter(pages)".to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        writeln!(css, "  @{edge}-{slot} {{ content: {content}; text-align: {slot}; vertical-align: {}; padding-{edge}: {}pt; }}", if edge == "top" { "top" } else { "bottom" }, decimal(distance)).unwrap();
    }
}

/// Escape CSS strings and prevent an inline stylesheet from closing its HTML slot.
#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn css_string(source: &str) -> String {
    let mut value = String::from("\"");
    for character in source.chars() {
        match character {
            '\\' | '"' => {
                value.push('\\');
                value.push(character);
            }
            '<' => value.push_str("\\3c "),
            character if character.is_control() => {
                write!(value, "\\{:x} ", u32::from(character)).unwrap();
            }
            character => value.push(character),
        }
    }
    value.push('"');
    value
}

fn decimal(value: f64) -> String {
    format!("{value:.6}")
}
