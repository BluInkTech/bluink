// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{W, package::WORD_TYPE};
use crate::engine::{
    publication::theme_format::{Alignment, ResolvedStyle, ResolvedTheme, Role, Stroke, Weight},
    xml::Xml,
};

pub(super) const fn id(role: Role) -> &'static str {
    match role {
        Role::Body => "Normal",
        Role::Heading1 => "Heading1",
        Role::Heading2 => "Heading2",
        Role::Heading3 => "Heading3",
        Role::Heading4 => "Heading4",
        Role::Heading5 => "Heading5",
        Role::Heading6 => "Heading6",
        Role::Link => "Hyperlink",
        Role::Quote => "BluinkQuote",
        Role::Code => "BluinkCode",
        Role::InlineCode => "BluinkInlineCode",
        Role::List => "BluinkList",
        Role::Table => "BluinkTable",
        Role::TableHeader => "BluinkTableHeader",
        Role::TableCell => "BluinkTableCell",
        Role::Caption => "BluinkCaption",
        Role::Footnote => "FootnoteText",
        Role::Embed => "BluinkEmbed",
        Role::Rule => "BluinkRule",
        Role::Box => "BluinkBox",
        Role::BoxHeading => "BluinkBoxHeading",
        Role::BoxInfo => "BluinkBoxInfo",
        Role::BoxAbstract => "BluinkBoxAbstract",
        Role::BoxNote => "BluinkBoxNote",
        Role::BoxTip => "BluinkBoxTip",
        Role::BoxSuccess => "BluinkBoxSuccess",
        Role::BoxImportant => "BluinkBoxImportant",
        Role::BoxWarning => "BluinkBoxWarning",
        Role::BoxError => "BluinkBoxError",
        Role::BoxQuestion => "BluinkBoxQuestion",
        Role::BoxExample => "BluinkBoxExample",
    }
}

pub(super) fn integer(value: f64, scale: f64, field: &str) -> Result<String, String> {
    let scaled = (value * scale).round();
    if !scaled.is_finite() || scaled < 0.0 || scaled > f64::from(i32::MAX) {
        return Err(format!(
            "DOCX {field} is outside the supported numeric range"
        ));
    }
    Ok(format!("{scaled:.0}"))
}

pub(super) fn twips(value: f64) -> Result<String, String> {
    integer(value, 20.0, "length")
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete named style registry together for OOXML review."
)]
pub(super) fn compile(
    theme: &ResolvedTheme,
    language: &str,
    fonts: &super::fonts::Fonts,
) -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start("w:styles", &[("xmlns:w", W.into())])?;
    xml.start("w:docDefaults", &[])?;
    xml.start("w:rPrDefault", &[])?;
    run_properties(
        &mut xml,
        &theme.styles[&Role::Body],
        fonts.family(&theme.styles[&Role::Body].font_family)?,
        language,
        false,
        false,
    )?;
    xml.end()?;
    xml.start("w:pPrDefault", &[])?;
    paragraph_properties(&mut xml, Role::Body, &theme.styles[&Role::Body], None)?;
    xml.end()?;
    xml.end()?;
    xml.start(
        "w:style",
        &[
            ("w:type", "character".into()),
            ("w:default", "1".into()),
            ("w:styleId", "DefaultParagraphFont".into()),
        ],
    )?;
    xml.value("w:name", "Default Paragraph Font")?;
    xml.empty("w:semiHidden", &[])?;
    xml.empty("w:unhideWhenUsed", &[])?;
    xml.end()?;
    for role in Role::ALL {
        let character = matches!(role, Role::Link | Role::InlineCode);
        let mut attributes = vec![
            (
                "w:type",
                if role == Role::Table {
                    "table"
                } else if character {
                    "character"
                } else {
                    "paragraph"
                }
                .into(),
            ),
            ("w:styleId", id(role).into()),
        ];
        if role == Role::Body {
            attributes.push(("w:default", "1".into()));
        }
        if !matches!(
            role,
            Role::Body
                | Role::Link
                | Role::Footnote
                | Role::Heading1
                | Role::Heading2
                | Role::Heading3
                | Role::Heading4
                | Role::Heading5
                | Role::Heading6
        ) {
            attributes.push(("w:customStyle", "1".into()));
        }
        xml.start("w:style", &attributes)?;
        xml.value("w:name", display_name(role))?;
        if role != Role::Body && role != Role::Table {
            xml.value(
                "w:basedOn",
                if character {
                    "DefaultParagraphFont"
                } else {
                    "Normal"
                },
            )?;
        }
        if !character && role != Role::Table {
            xml.value("w:next", "Normal")?;
        }
        xml.empty("w:qFormat", &[])?;
        let style = &theme.styles[&role];
        if !character {
            paragraph_properties(&mut xml, role, style, heading_level(role))?;
        }
        run_properties(
            &mut xml,
            style,
            fonts.family(&style.font_family)?,
            language,
            character,
            role == Role::Link,
        )?;
        if role == Role::Table {
            table_properties(&mut xml, style)?;
        }
        xml.end()?;
    }
    xml.start(
        "w:style",
        &[
            ("w:type", "paragraph".into()),
            ("w:styleId", "Title".into()),
        ],
    )?;
    xml.value("w:name", "Title")?;
    xml.value("w:basedOn", "Heading1")?;
    xml.value("w:next", "Normal")?;
    xml.empty("w:qFormat", &[])?;
    xml.start("w:pPr", &[])?;
    xml.value("w:outlineLvl", 9)?;
    xml.end()?;
    xml.end()?;
    for (name, property, value) in [
        ("Strong", "w:b", "1"),
        ("Emphasis", "w:i", "1"),
        ("FootnoteReference", "w:vertAlign", "superscript"),
    ] {
        xml.start(
            "w:style",
            &[("w:type", "character".into()), ("w:styleId", name.into())],
        )?;
        xml.value("w:name", name)?;
        xml.value("w:basedOn", "DefaultParagraphFont")?;
        xml.start("w:rPr", &[])?;
        xml.value(property, value)?;
        xml.end()?;
        xml.end()?;
    }
    for (name, based_on, outline) in [
        ("BluinkChapter", "Heading1", 0),
        ("BluinkRunning", "FootnoteText", 9),
        ("TOCHeading", "Heading1", 9),
    ] {
        xml.start(
            "w:style",
            &[("w:type", "paragraph".into()), ("w:styleId", name.into())],
        )?;
        xml.value(
            "w:name",
            if name == "BluinkChapter" {
                "Bluink chapter"
            } else {
                name
            },
        )?;
        xml.value("w:basedOn", based_on)?;
        xml.value("w:next", "Normal")?;
        xml.start("w:pPr", &[])?;
        if name == "BluinkRunning" {
            xml.empty(
                "w:spacing",
                &[("w:before", "0".into()), ("w:after", "0".into())],
            )?;
        }
        xml.value("w:outlineLvl", outline)?;
        xml.end()?;
        if name == "BluinkRunning" {
            xml.start("w:rPr", &[])?;
            xml.value("w:color", theme.colors["muted"].trim_start_matches('#'))?;
            xml.end()?;
        }
        xml.end()?;
    }
    for level in 1..=9 {
        xml.start(
            "w:style",
            &[
                ("w:type", "paragraph".into()),
                ("w:styleId", format!("TOC{level}")),
            ],
        )?;
        xml.value("w:name", format!("toc {level}"))?;
        xml.value("w:basedOn", "Normal")?;
        xml.value("w:next", "Normal")?;
        xml.start("w:pPr", &[])?;
        xml.value("w:keepNext", 0)?;
        xml.empty(
            "w:spacing",
            &[("w:before", "0".into()), ("w:after", "60".into())],
        )?;
        xml.empty("w:ind", &[("w:left", ((level - 1) * 240).to_string())])?;
        xml.value("w:outlineLvl", 9)?;
        xml.end()?;
        xml.end()?;
    }
    xml.end()?;
    xml.finish()
}

fn display_name(role: Role) -> String {
    if let Some(level) = heading_level(role) {
        return format!("heading {}", level + 1);
    }
    match role {
        Role::Body => "Normal".into(),
        Role::Link => "Hyperlink".into(),
        Role::Footnote => "footnote text".into(),
        _ => format!("Bluink {}", role.name()),
    }
}

const fn heading_level(role: Role) -> Option<u8> {
    match role {
        Role::Heading1 => Some(0),
        Role::Heading2 => Some(1),
        Role::Heading3 => Some(2),
        Role::Heading4 => Some(3),
        Role::Heading5 => Some(4),
        Role::Heading6 => Some(5),
        _ => None,
    }
}

pub(super) fn paragraph_properties(
    xml: &mut Xml,
    role: Role,
    style: &ResolvedStyle,
    outline: Option<u8>,
) -> Result<(), String> {
    xml.start("w:pPr", &[])?;
    xml.value("w:keepNext", u8::from(style.keep_with_next))?;
    paragraph_layout(xml, role, style, outline, false)?;
    xml.end()
}

pub(super) fn paragraph_layout(
    xml: &mut Xml,
    role: Role,
    style: &ResolvedStyle,
    outline: Option<u8>,
    page_break: bool,
) -> Result<(), String> {
    xml.value("w:keepLines", u8::from(style.keep_together))?;
    if page_break {
        xml.value("w:pageBreakBefore", 1)?;
    }
    xml.value("w:widowControl", 1)?;
    if style.border_width_pt > 0.0
        && !super::frames::container_role(role)
        && !matches!(role, Role::Table | Role::TableHeader | Role::TableCell)
    {
        xml.start("w:pBdr", &[])?;
        for side in if role == Role::Rule {
            &["bottom"][..]
        } else {
            &["top", "left", "bottom", "right", "between"][..]
        } {
            border(xml, side, style, true)?;
        }
        xml.end()?;
    }
    if !super::frames::container_role(role)
        && !matches!(role, Role::Table | Role::TableHeader | Role::TableCell)
    {
        shading(xml, style)?;
    }
    xml.empty(
        "w:spacing",
        &[
            ("w:before", twips(style.space_before_pt)?),
            ("w:after", twips(style.space_after_pt)?),
            ("w:line", twips(style.line_height * style.font_size_pt)?),
            ("w:lineRule", "exact".into()),
        ],
    )?;
    xml.empty(
        "w:ind",
        &[("w:left", twips(style.indent_pt)?), ("w:right", "0".into())],
    )?;
    xml.value(
        "w:jc",
        match style.alignment {
            Alignment::Start => "start",
            Alignment::Center => "center",
            Alignment::End => "end",
            Alignment::Justify => "both",
        },
    )?;
    if let Some(level) = outline {
        xml.value("w:outlineLvl", level)?;
    }
    Ok(())
}

pub(super) fn run_properties(
    xml: &mut Xml,
    style: &ResolvedStyle,
    family: &str,
    language: &str,
    character: bool,
    link: bool,
) -> Result<(), String> {
    xml.start("w:rPr", &[])?;
    xml.empty(
        "w:rFonts",
        &[
            ("w:ascii", family.into()),
            ("w:hAnsi", family.into()),
            ("w:eastAsia", family.into()),
            ("w:cs", family.into()),
        ],
    )?;
    xml.value("w:b", u8::from(style.font_weight == Weight::Bold))?;
    xml.value("w:bCs", u8::from(style.font_weight == Weight::Bold))?;
    xml.value("w:i", u8::from(style.italic))?;
    xml.value("w:iCs", u8::from(style.italic))?;
    xml.value("w:color", style.color.trim_start_matches('#'))?;
    xml.value("w:sz", integer(style.font_size_pt, 2.0, "font size")?)?;
    xml.value("w:szCs", integer(style.font_size_pt, 2.0, "font size")?)?;
    if link {
        xml.value("w:u", "single")?;
    }
    if character {
        if style.border_width_pt > 0.0 {
            border(xml, "bdr", style, true)?;
        }
        shading(xml, style)?;
    }
    xml.empty("w:lang", &[("w:val", language.into())])?;
    xml.end()
}

pub(super) fn shading(xml: &mut Xml, style: &ResolvedStyle) -> Result<(), String> {
    if let Some(background) = &style.background {
        xml.empty(
            "w:shd",
            &[
                ("w:val", "clear".into()),
                ("w:color", "auto".into()),
                ("w:fill", background.trim_start_matches('#').into()),
            ],
        )?;
    }
    Ok(())
}

pub(super) fn border(
    xml: &mut Xml,
    side: &str,
    style: &ResolvedStyle,
    paragraph: bool,
) -> Result<(), String> {
    if style.border_width_pt > 12.0 {
        return Err("DOCX borders cannot exceed 12pt".into());
    }
    let mut attributes = vec![(
        "w:val",
        if style.border_width_pt == 0.0 {
            "nil"
        } else {
            match style.border_style {
                Stroke::Solid => "single",
                Stroke::Dashed => "dashed",
                Stroke::Dotted => "dotted",
            }
        }
        .into(),
    )];
    if style.border_width_pt > 0.0 {
        attributes.push((
            "w:sz",
            integer(style.border_width_pt.max(0.25), 8.0, "border width")?,
        ));
        attributes.push(("w:color", style.border_color.trim_start_matches('#').into()));
        if paragraph {
            if style.padding_pt > 31.0 {
                return Err("DOCX paragraph border padding cannot exceed 31pt".into());
            }
            attributes.push(("w:space", integer(style.padding_pt, 1.0, "border padding")?));
        }
    }
    xml.empty(&format!("w:{side}"), &attributes)
}

fn table_properties(xml: &mut Xml, style: &ResolvedStyle) -> Result<(), String> {
    xml.start("w:tblPr", &[])?;
    xml.empty(
        "w:tblInd",
        &[("w:w", twips(style.indent_pt)?), ("w:type", "dxa".into())],
    )?;
    xml.start("w:tblBorders", &[])?;
    for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        border(xml, side, style, false)?;
    }
    xml.end()?;
    shading(xml, style)?;
    xml.start("w:tblCellMar", &[])?;
    for side in ["top", "left", "bottom", "right"] {
        xml.empty(
            &format!("w:{side}"),
            &[("w:w", twips(style.padding_pt)?), ("w:type", "dxa".into())],
        )?;
    }
    xml.end()?;
    xml.end()
}

pub(super) fn page_properties(xml: &mut Xml, theme: &ResolvedTheme) -> Result<(), String> {
    let page = &theme.page;
    xml.empty(
        "w:pgSz",
        &[
            ("w:w", twips(page.width_pt)?),
            ("w:h", twips(page.height_pt)?),
            (
                "w:orient",
                if page.width_pt > page.height_pt {
                    "landscape"
                } else {
                    "portrait"
                }
                .into(),
            ),
        ],
    )?;
    xml.empty(
        "w:pgMar",
        &[
            ("w:top", twips(page.margin_top_pt)?),
            ("w:right", twips(page.margin_right_pt)?),
            ("w:bottom", twips(page.margin_bottom_pt)?),
            ("w:left", twips(page.margin_left_pt)?),
            ("w:header", twips(page.header_distance_pt)?),
            ("w:footer", twips(page.footer_distance_pt)?),
            ("w:gutter", "0".into()),
        ],
    )?;
    Ok(())
}

pub(super) fn content_type(part: &str) -> String {
    format!("{WORD_TYPE}.{part}+xml")
}
