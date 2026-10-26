// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{
    DocxDocument,
    package::{MAIN, Package},
    styles, word_root,
};
use crate::engine::{
    publication::{
        render::ExportError,
        theme_format::{ChapterStart, ResolvedTheme, Role, RunningLine, RunningPart},
    },
    xml::Xml,
};

pub(super) struct Entry {
    pub(super) level: u16,
    pub(super) title: String,
    pub(super) anchor: String,
}
pub(super) struct Parts {
    header: String,
    footer: String,
    first_header: String,
    first_footer: String,
}

pub(super) const fn heading_role(level: u16) -> Role {
    match level {
        1 => Role::Heading1,
        2 => Role::Heading2,
        3 => Role::Heading3,
        4 => Role::Heading4,
        5 => Role::Heading5,
        _ => Role::Heading6,
    }
}

impl DocxDocument<'_> {
    pub(crate) fn set_content_language(&mut self, language: &str) -> Result<(), ExportError> {
        if !self.story.balanced() {
            return Err(ExportError::new(
                "DOCX page language must be valid and set between complete pages",
            ));
        }
        self.content_language = super::canonical_language(language).map_err(ExportError::new)?;
        Ok(())
    }

    pub(crate) fn page_anchor(&mut self, anchor: &str) -> Result<(), ExportError> {
        if !self.story.balanced() {
            return Err(ExportError::new(
                "DOCX page anchors must start between complete pages",
            ));
        }
        self.story.pending.push(anchor.into());
        Ok(())
    }

    pub(crate) fn contents_page(&mut self) -> Result<(), ExportError> {
        if !self.story.balanced() || self.story.owner != MAIN || self.toc_position.is_some() {
            return Err(ExportError::new(
                "DOCX contents must be inserted once between complete pages",
            ));
        }
        self.toc_position = Some(self.story.xml.position());
        self.fonts
            .use_style(&self.theme.styles[&Role::Heading1], false, false);
        self.fonts
            .use_style(&self.theme.styles[&Role::Body], false, false);
        Ok(())
    }

    pub(crate) fn chapter(
        &mut self,
        title: &str,
        level: u16,
        anchor: &str,
        first: bool,
    ) -> Result<(), ExportError> {
        self.chapter_inner(title, level, anchor, first)
            .map_err(ExportError::new)
    }

    #[expect(
        clippy::expect_used,
        reason = "The outline value is capped at eight before conversion to u8."
    )]
    fn chapter_inner(
        &mut self,
        title: &str,
        level: u16,
        anchor: &str,
        first: bool,
    ) -> Result<(), String> {
        if !self.story.balanced() || self.story.owner != MAIN {
            return Err("DOCX chapter must start between complete pages".into());
        }
        let (id, name) = self
            .bookmarks
            .define(anchor)
            .ok_or("DOCX chapter anchor is already defined")?;
        let (contents_id, contents_name) = self
            .bookmarks
            .define_contents(anchor)
            .ok_or("DOCX contents anchor is already defined")?;
        let role = heading_role(level);
        self.fonts
            .use_style(&self.theme.styles[&role], false, false);
        let xml = &mut self.story.xml;
        xml.start("w:p", &[])?;
        xml.start("w:pPr", &[])?;
        xml.value("w:pStyle", "BluinkChapter")?;
        xml.value("w:keepNext", 1)?;
        let role = heading_role(level);
        styles::paragraph_layout(
            xml,
            role,
            &self.theme.styles[&role],
            Some(u8::try_from(level.saturating_sub(1).min(8)).expect("outline is capped at eight")),
            !first && self.theme.page.chapter_start == ChapterStart::NextPage,
        )?;
        xml.end()?;
        xml.empty(
            "w:bookmarkStart",
            &[("w:id", id.to_string()), ("w:name", name.clone())],
        )?;
        xml.empty(
            "w:bookmarkStart",
            &[
                ("w:id", contents_id.to_string()),
                ("w:name", contents_name.clone()),
            ],
        )?;
        xml.start("w:r", &[])?;
        styles::run_properties(
            xml,
            &self.theme.styles[&heading_role(level)],
            self.fonts.family(&self.theme.styles[&role].font_family)?,
            &self.content_language,
            false,
            false,
        )?;
        xml.start("w:t", &[("xml:space", "preserve".into())])?;
        xml.text(title)?;
        xml.end()?;
        xml.end()?;
        xml.empty("w:bookmarkEnd", &[("w:id", contents_id.to_string())])?;
        xml.empty("w:bookmarkEnd", &[("w:id", id.to_string())])?;
        if !first {
            // Explicit outline entries keep Word updates aligned with PDF and
            // HTML instead of collecting every body heading or the cover title.
            contents_entry(xml, title, level, &name)?;
        }
        xml.end()?;
        if !first {
            self.first_chapter_anchor.get_or_insert(name);
            self.contents.push(Entry {
                level,
                title: title.into(),
                anchor: contents_name,
            });
        }
        self.story.last_table = false;
        Ok(())
    }
}

fn field_marker(xml: &mut Xml, kind: &str) -> Result<(), String> {
    xml.start("w:r", &[])?;
    xml.empty("w:fldChar", &[("w:fldCharType", kind.into())])?;
    xml.end()
}

fn instruction(xml: &mut Xml, text: &str) -> Result<(), String> {
    xml.start("w:r", &[])?;
    xml.start("w:instrText", &[("xml:space", "preserve".into())])?;
    xml.text(text)?;
    xml.end()?;
    xml.end()
}

fn contents_entry(xml: &mut Xml, title: &str, level: u16, anchor: &str) -> Result<(), String> {
    // A nested REF keeps arbitrary chapter text out of the instruction grammar
    // and follows edits to the authored title bookmark. TC has no visible result.
    // Explicit vanish formatting makes Word skip TC entries during TOC updates.
    field_marker(xml, "begin")?;
    instruction(xml, " TC \"")?;
    field_marker(xml, "begin")?;
    instruction(xml, &format!(" REF {anchor} "))?;
    field_marker(xml, "separate")?;
    text_run(xml, title)?;
    field_marker(xml, "end")?;
    instruction(xml, &format!("\" \\f B \\l {} ", level.clamp(1, 9)))?;
    field_marker(xml, "end")
}

pub(super) fn field(
    xml: &mut Xml,
    instruction: &str,
    result: &str,
    style: Option<&str>,
) -> Result<(), String> {
    xml.start(
        "w:fldSimple",
        &[("w:instr", instruction.into()), ("w:dirty", "1".into())],
    )?;
    xml.start("w:r", &[])?;
    if let Some(style) = style {
        xml.start("w:rPr", &[])?;
        xml.value("w:rStyle", style)?;
        xml.end()?;
    }
    xml.start("w:t", &[("xml:space", "preserve".into())])?;
    xml.text(result)?;
    xml.end()?;
    xml.end()?;
    xml.end()
}

pub(super) fn install(
    package: &mut Package,
    theme: &ResolvedTheme,
    title: &str,
    first_chapter: Option<&str>,
) -> Result<Parts, String> {
    let mut ids = Vec::new();
    for (kind, line) in [
        ("header", &theme.page.header),
        ("footer", &theme.page.footer),
    ] {
        let path = format!("word/{kind}.xml");
        package.add(
            &path,
            styles::content_type(kind),
            running(theme, line, title, kind, false, first_chapter)?,
        )?;
        ids.push(package.relate(MAIN, kind, &path)?);
        let first = format!("word/{kind}-first.xml");
        package.add(
            &first,
            styles::content_type(kind),
            running(
                theme,
                line,
                title,
                kind,
                !line.show_on_first_page,
                first_chapter,
            )?,
        )?;
        ids.push(package.relate(MAIN, kind, &first)?);
    }
    Ok(Parts {
        header: ids[0].clone(),
        first_header: ids[1].clone(),
        footer: ids[2].clone(),
        first_footer: ids[3].clone(),
    })
}

impl Parts {
    pub(super) fn section(&self, xml: &mut Xml, theme: &ResolvedTheme) -> Result<(), String> {
        xml.start("w:sectPr", &[])?;
        for (kind, normal, first) in [
            ("header", &self.header, &self.first_header),
            ("footer", &self.footer, &self.first_footer),
        ] {
            for (scope, id) in [("default", normal), ("first", first)] {
                xml.empty(
                    &format!("w:{kind}Reference"),
                    &[("w:type", scope.into()), ("r:id", id.clone())],
                )?;
            }
        }
        styles::page_properties(xml, theme)?;
        xml.value("w:titlePg", 1)?;
        xml.end()
    }
}

fn running(
    theme: &ResolvedTheme,
    line: &RunningLine,
    title: &str,
    kind: &str,
    empty: bool,
    first_chapter: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    word_root(&mut xml, if kind == "header" { "w:hdr" } else { "w:ftr" })?;
    xml.start("w:p", &[])?;
    if !empty {
        let width = theme.page.width_pt - theme.page.margin_left_pt - theme.page.margin_right_pt;
        xml.start("w:pPr", &[])?;
        xml.value("w:pStyle", "BluinkRunning")?;
        xml.start("w:tabs", &[])?;
        for (alignment, position) in [("center", width / 2.0), ("right", width)] {
            xml.empty(
                "w:tab",
                &[
                    ("w:val", alignment.into()),
                    ("w:pos", styles::twips(position)?),
                ],
            )?;
        }
        xml.end()?;
        xml.end()?;
        for (index, parts) in [&line.left, &line.center, &line.right].iter().enumerate() {
            if index > 0 {
                xml.start("w:r", &[])?;
                xml.empty("w:tab", &[])?;
                xml.end()?;
            }
            for part in *parts {
                match part {
                    RunningPart::Text(text) => text_run(&mut xml, text)?,
                    RunningPart::DocumentTitle => text_run(&mut xml, title)?,
                    RunningPart::ChapterTitle => {
                        if let Some(anchor) = first_chapter {
                            chapter_title(&mut xml, anchor)?;
                        }
                    }
                    RunningPart::PageNumber => field(&mut xml, " PAGE ", "1", None)?,
                    RunningPart::PageCount => field(&mut xml, " NUMPAGES ", "1", None)?,
                }
            }
        }
    }
    xml.end()?;
    xml.end()?;
    xml.finish()
}

fn complex_field(xml: &mut Xml, code: &str, result: &str) -> Result<(), String> {
    field_marker(xml, "begin")?;
    instruction(xml, code)?;
    field_marker(xml, "separate")?;
    text_run(xml, result)?;
    field_marker(xml, "end")
}

fn chapter_title(xml: &mut Xml, first_chapter: &str) -> Result<(), String> {
    // STYLEREF also finds the cover title or a future chapter. Compare physical
    // pages first so cover/contents pages retain an empty chapter-title slot.
    field_marker(xml, "begin")?;
    instruction(xml, " IF ")?;
    complex_field(xml, " PAGE ", "1")?;
    instruction(xml, " < ")?;
    complex_field(xml, &format!(" PAGEREF {first_chapter} "), "1")?;
    instruction(xml, " \"\" \"")?;
    complex_field(xml, " STYLEREF \"Bluink chapter\" ", "")?;
    instruction(xml, "\" ")?;
    field_marker(xml, "separate")?;
    text_run(xml, "")?;
    field_marker(xml, "end")
}

fn text_run(xml: &mut Xml, text: &str) -> Result<(), String> {
    xml.start("w:r", &[])?;
    xml.start("w:t", &[("xml:space", "preserve".into())])?;
    xml.text(text)?;
    xml.end()?;
    xml.end()
}

pub(super) fn contents(entries: &[Entry], theme: &ResolvedTheme) -> Result<Xml, String> {
    let mut xml = Xml::fragment();
    xml.start("w:sdt", &[])?;
    xml.start("w:sdtPr", &[])?;
    xml.start("w:docPartObj", &[])?;
    xml.value("w:docPartGallery", "Table of Contents")?;
    xml.empty("w:docPartUnique", &[])?;
    xml.end()?;
    xml.end()?;
    xml.start("w:sdtContent", &[])?;
    xml.start("w:p", &[])?;
    xml.start("w:pPr", &[])?;
    xml.value("w:pStyle", "TOCHeading")?;
    xml.value("w:pageBreakBefore", 1)?;
    xml.end()?;
    text_run(&mut xml, "Contents")?;
    xml.end()?;
    let width = theme.page.width_pt - theme.page.margin_left_pt - theme.page.margin_right_pt;
    for (index, entry) in entries.iter().enumerate() {
        xml.start("w:p", &[])?;
        xml.start("w:pPr", &[])?;
        xml.value("w:pStyle", format!("TOC{}", entry.level.clamp(1, 9)))?;
        xml.start("w:tabs", &[])?;
        xml.empty(
            "w:tab",
            &[
                ("w:val", "right".into()),
                ("w:leader", "dot".into()),
                ("w:pos", styles::twips(width)?),
            ],
        )?;
        xml.end()?;
        xml.end()?;
        if index == 0 {
            field_marker(&mut xml, "begin")?;
            instruction(&mut xml, " TOC \\f B \\h \\z ")?;
            field_marker(&mut xml, "separate")?;
        }
        xml.start("w:hyperlink", &[("w:anchor", entry.anchor.clone())])?;
        text_run(&mut xml, &entry.title)?;
        xml.end()?;
        xml.start("w:r", &[])?;
        xml.empty("w:tab", &[])?;
        xml.end()?;
        field(
            &mut xml,
            &format!(" PAGEREF {} \\h ", entry.anchor),
            "1",
            None,
        )?;
        if index + 1 == entries.len() {
            field_marker(&mut xml, "end")?;
        }
        xml.end()?;
    }
    xml.end()?;
    xml.end()?;
    Ok(xml)
}
