// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Native `WordprocessingML`. No Word-generation library or HTML import is used.

mod bookmarks;
#[cfg(test)]
mod content_tests;
mod fonts;
mod frames;
mod media;
mod notes;
mod numbering;
mod package;
mod print;
mod story;
mod styles;
mod tables;
#[cfg(test)]
pub(crate) mod tests;

use crate::engine::{
    publication::{
        render::{Destination, ExportError, Formatter, Kind, Mark, NodeInfo, RenderEvent},
        theme_format::{ResolvedTheme, Role},
    },
    xml::Xml,
};
use bookmarks::Bookmarks;
use numbering::Numbering;
use package::{MAIN, Package};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use story::Story;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn word_root(xml: &mut Xml, name: &str) -> Result<(), String> {
    let mut attributes = vec![
        ("xmlns:w", W.into()),
        ("xmlns:r", R.into()),
        (
            "xmlns:mc",
            "http://schemas.openxmlformats.org/markup-compatibility/2006".into(),
        ),
        ("mc:Ignorable", "asvg".into()),
    ];
    attributes.extend(media::namespaces());
    xml.start(name, &attributes)
}

pub(crate) struct DocxBundle {
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct DocxDocument<'a> {
    theme: &'a ResolvedTheme,
    package: Package,
    numbering: Numbering,
    fonts: fonts::Fonts,
    bookmarks: Bookmarks,
    story: Story,
    saved_story: Option<Story>,
    notes: Option<Xml>,
    note_ids: BTreeSet<usize>,
    figures: Vec<(String, bool)>,
    assets: BTreeMap<String, media::Asset>,
    svg_fonts: super::media::SvgFonts,
    media_parts: BTreeSet<String>,
    drawing_id: usize,
    title: String,
    language: String,
    content_language: String,
    toc_position: Option<usize>,
    contents: Vec<print::Entry>,
    first_chapter_anchor: Option<String>,
}

impl<'a> DocxDocument<'a> {
    pub(crate) fn new(
        theme: &'a ResolvedTheme,
        title: &str,
        language: &str,
        font_folder: &Path,
    ) -> Result<Self, ExportError> {
        Self::create(theme, title, language, font_folder).map_err(ExportError::new)
    }

    fn create(
        theme: &'a ResolvedTheme,
        title: &str,
        language: &str,
        font_folder: &Path,
    ) -> Result<Self, String> {
        let language = canonical_language(language)?;
        let language = language.as_str();
        for role in Role::ALL {
            if !theme.styles.contains_key(&role) {
                return Err(format!("DOCX theme has no {} style", role.name()));
            }
        }
        let mut fonts = fonts::Fonts::load(font_folder, theme)?;
        fonts.use_style(&theme.styles[&Role::Body], false, false);
        if [&theme.page.header, &theme.page.footer]
            .iter()
            .any(|line| !line.left.is_empty() || !line.center.is_empty() || !line.right.is_empty())
        {
            fonts.use_style(&theme.styles[&Role::Footnote], false, false);
        }
        let mut package = Package::default();
        package.relate("", "officeDocument", MAIN)?;
        for (path, kind, bytes) in [
            (
                "word/styles.xml",
                "styles",
                styles::compile(theme, language, &fonts)?,
            ),
            ("word/settings.xml", "settings", settings()?),
        ] {
            package.add(path, styles::content_type(kind), bytes)?;
            package.relate(MAIN, kind, path)?;
        }
        package.add(
            "docProps/core.xml",
            "application/vnd.openxmlformats-package.core-properties+xml",
            core_properties(title, language)?,
        )?;
        package.relate("", "core-properties", "docProps/core.xml")?;
        package.add(
            "docProps/app.xml",
            "application/vnd.openxmlformats-officedocument.extended-properties+xml",
            app_properties()?,
        )?;
        package.relate("", "extended-properties", "docProps/app.xml")?;
        let mut xml = Xml::new();
        word_root(&mut xml, "w:document")?;
        xml.empty(
            "w:background",
            &[(
                "w:color",
                theme.colors["paper"].trim_start_matches('#').into(),
            )],
        )?;
        xml.start("w:body", &[])?;
        let width = theme.page.width_pt - theme.page.margin_left_pt - theme.page.margin_right_pt;
        Ok(Self {
            theme,
            package,
            numbering: Numbering::default(),
            fonts,
            bookmarks: Bookmarks::default(),
            story: Story::new(xml, MAIN, width, Role::Body),
            saved_story: None,
            notes: None,
            note_ids: BTreeSet::new(),
            figures: Vec::new(),
            assets: BTreeMap::new(),
            svg_fonts: super::media::SvgFonts::default(),
            media_parts: BTreeSet::new(),
            drawing_id: 0,
            title: title.into(),
            language: language.into(),
            content_language: language.into(),
            toc_position: None,
            contents: Vec::new(),
            first_chapter_anchor: None,
        })
    }

    pub(super) fn pending_anchor(&mut self, id: Option<&str>) {
        if let Some(id) = id {
            self.story.pending.push(format!("block-{id}"));
        }
    }

    pub(super) fn begin_paragraph(
        &mut self,
        role: Role,
        id: Option<&str>,
        level: Option<u16>,
    ) -> Result<(), String> {
        if self.story.paragraph {
            return Err("DOCX paragraph cannot contain another paragraph".into());
        }
        self.pending_anchor(id);
        self.fonts
            .use_style(&self.theme.styles[&role], false, false);
        self.story.paragraph_role = role;
        self.story.xml.start("w:p", &[])?;
        self.story.xml.start("w:pPr", &[])?;
        self.story.xml.value("w:pStyle", styles::id(role))?;
        if self.keep_paragraph_with_next(role) {
            self.story.xml.value("w:keepNext", 1)?;
        }
        if self.story.list_item {
            let number = *self
                .story
                .lists
                .last()
                .ok_or("DOCX list item has no numbering")?;
            self.story.xml.start("w:numPr", &[])?;
            self.story.xml.value("w:ilvl", 0)?;
            self.story.xml.value("w:numId", number)?;
            self.story.xml.end()?;
        }
        if self.story.drawing || frames::container_role(role) {
            let body = &self.theme.styles[&Role::Body];
            let style = &self.theme.styles[&role];
            let mut spacing = vec![
                (
                    "w:line",
                    if self.story.drawing {
                        "240".into()
                    } else {
                        styles::twips(style.font_size_pt * style.line_height)?
                    },
                ),
                (
                    "w:lineRule",
                    if self.story.drawing { "auto" } else { "exact" }.into(),
                ),
            ];
            if frames::container_role(role) {
                spacing.push(("w:before", styles::twips(body.space_before_pt)?));
                spacing.push(("w:after", styles::twips(body.space_after_pt)?));
            }
            self.story.xml.empty("w:spacing", &spacing)?;
        }
        if frames::container_role(role) {
            self.story
                .xml
                .empty("w:ind", &[("w:left", "0".into()), ("w:right", "0".into())])?;
        }
        if self.story.drawing {
            self.story.xml.value("w:jc", "center")?;
        }
        if let Some(level) = level {
            self.story
                .xml
                .value("w:outlineLvl", level.saturating_sub(1).min(8))?;
        }
        self.story.xml.end()?;
        self.story.paragraph = true;
        if let Some(content) = self.story.frame_content.last_mut() {
            *content = true;
        }
        self.story.last_table = false;
        if let Some(table) = self.story.table.as_mut() {
            table.cell_content = true;
        }
        for key in std::mem::take(&mut self.story.pending) {
            if let Some((number, name)) = self.bookmarks.define(&key) {
                self.story.xml.empty(
                    "w:bookmarkStart",
                    &[("w:id", number.to_string()), ("w:name", name)],
                )?;
                self.story.paragraph_bookmarks.push(number);
            }
        }
        if self.story.note_marker {
            self.story.xml.start("w:r", &[])?;
            self.story.xml.start("w:rPr", &[])?;
            self.story.xml.value("w:rStyle", "FootnoteReference")?;
            self.story.xml.end()?;
            self.story.xml.empty("w:footnoteRef", &[])?;
            self.story.xml.end()?;
            self.text(" ")?;
            self.story.note_marker = false;
        }
        Ok(())
    }

    pub(super) fn end_paragraph(&mut self) -> Result<(), String> {
        if !self.story.paragraph {
            return Err("DOCX paragraph end has no matching start".into());
        }
        for number in std::mem::take(&mut self.story.paragraph_bookmarks)
            .into_iter()
            .rev()
        {
            self.story
                .xml
                .empty("w:bookmarkEnd", &[("w:id", number.to_string())])?;
        }
        self.story.xml.end()?;
        self.story.paragraph = false;
        Ok(())
    }

    fn begin_run(&mut self) -> Result<(), String> {
        if !self.story.paragraph {
            return Err("DOCX inline content must be inside a paragraph".into());
        }
        let role = if self.story.marks.contains(&Mark::Code) {
            Role::InlineCode
        } else if self.story.marks.iter().any(
            |mark| matches!(mark,Mark::Link(destination) if *destination!=Destination::PlainText),
        ) {
            Role::Link
        } else {
            self.story.paragraph_role
        };
        self.fonts.use_style(
            &self.theme.styles[&role],
            self.story.marks.contains(&Mark::Strong),
            self.story.marks.contains(&Mark::Emphasis),
        );
        self.story.xml.start("w:r", &[])?;
        {
            self.story.xml.start("w:rPr", &[])?;
            if self.story.marks.contains(&Mark::Code) {self.story.xml.value("w:rStyle",styles::id(Role::InlineCode))?;}
            else if self.story.marks.iter().any(|mark|matches!(mark,Mark::Link(destination) if *destination!=Destination::PlainText)) {self.story.xml.value("w:rStyle",styles::id(Role::Link))?;}
            if self.story.marks.contains(&Mark::Strong) {
                self.story.xml.value("w:b", 1)?;
                self.story.xml.value("w:bCs", 1)?;
            }
            if self.story.marks.contains(&Mark::Emphasis) {
                self.story.xml.value("w:i", 1)?;
                self.story.xml.value("w:iCs", 1)?;
            }
            if self.story.marks.contains(&Mark::Strike) {
                self.story.xml.value("w:strike", 1)?;
            }
            if self.story.marks.contains(&Mark::Underline) {
                self.story.xml.value("w:u", "single")?;
            }
            if self.story.marks.contains(&Mark::Superscript) {
                self.story.xml.value("w:vertAlign", "superscript")?;
            } else if self.story.marks.contains(&Mark::Subscript) {
                self.story.xml.value("w:vertAlign", "subscript")?;
            }
            self.story.xml.empty(
                "w:lang",
                &[
                    ("w:val", self.content_language.clone()),
                    ("w:eastAsia", self.content_language.clone()),
                    ("w:bidi", self.content_language.clone()),
                ],
            )?;
            self.story.xml.end()?;
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        self.begin_run()?;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut offset = 0;
        for (index, character) in text
            .char_indices()
            .filter(|(_, character)| matches!(character, '\n' | '\t'))
        {
            self.text_segment(&text[offset..index])?;
            self.story
                .xml
                .empty(if character == '\t' { "w:tab" } else { "w:br" }, &[])?;
            offset = index + character.len_utf8();
        }
        self.text_segment(&text[offset..])?;
        self.story.xml.end()
    }

    fn text_segment(&mut self, text: &str) -> Result<(), String> {
        if !text.is_empty() {
            self.story
                .xml
                .start("w:t", &[("xml:space", "preserve".into())])?;
            self.story.xml.text(text)?;
            self.story.xml.end()?;
        }
        Ok(())
    }

    fn enter_mark(&mut self, mark: &Mark) -> Result<(), String> {
        if let Mark::Link(destination) = mark {
            let mut attributes = vec![("w:history", "1".into())];
            match destination {
                Destination::Url(url) => {
                    attributes.push(("r:id", self.package.hyperlink(&self.story.owner, url)?));
                }
                Destination::Anchor(anchor) => {
                    attributes.push(("w:anchor", self.bookmarks.link(anchor)));
                }
                Destination::PlainText => {
                    self.story.marks.push(mark.clone());
                    return Ok(());
                }
            }
            self.story.xml.start("w:hyperlink", &attributes)?;
        }
        self.story.marks.push(mark.clone());
        Ok(())
    }

    fn exit_mark(&mut self, mark: &Mark) -> Result<(), String> {
        if self.story.marks.pop().as_ref() != Some(mark) {
            return Err("DOCX inline marks are unbalanced".into());
        }
        if matches!(mark,Mark::Link(destination) if *destination!=Destination::PlainText) {
            self.story.xml.end()?;
        }
        Ok(())
    }

    fn enter(&mut self, node: &NodeInfo) -> Result<(), String> {
        match &node.kind {
            Kind::Document => Ok(()),
            Kind::Paragraph => self.begin_paragraph(
                if self.story.list_item {
                    Role::List
                } else {
                    self.story.role()
                },
                node.id.as_deref(),
                None,
            ),
            Kind::Heading { level } => {
                let role = print::heading_role(*level);
                self.begin_paragraph(role, node.id.as_deref(), Some(*level))
            }
            Kind::Text(text) => self.text(text),
            Kind::HardBreak => {
                self.begin_run()?;
                self.story.xml.empty("w:br", &[])?;
                self.story.xml.end()
            }
            Kind::CodeBlock { text, .. } => {
                self.start_frame(Role::Code, node.id.as_deref())?;
                self.begin_paragraph(Role::Code, None, None)?;
                self.text(text)
            }
            Kind::Rule => self.begin_paragraph(Role::Rule, node.id.as_deref(), None),
            Kind::Quote => self.start_frame(Role::Quote, node.id.as_deref()),
            Kind::Embed { .. } => self.start_frame(Role::Embed, node.id.as_deref()),
            Kind::Box(kind) => {
                let role = match kind {
                    super::render::BoxKind::Info => Role::BoxInfo,
                    super::render::BoxKind::Abstract => Role::BoxAbstract,
                    super::render::BoxKind::Note => Role::BoxNote,
                    super::render::BoxKind::Tip => Role::BoxTip,
                    super::render::BoxKind::Success => Role::BoxSuccess,
                    super::render::BoxKind::Important => Role::BoxImportant,
                    super::render::BoxKind::Warning => Role::BoxWarning,
                    super::render::BoxKind::Error => Role::BoxError,
                    super::render::BoxKind::Question => Role::BoxQuestion,
                    super::render::BoxKind::Example => Role::BoxExample,
                };
                self.start_frame(role, node.id.as_deref())
            }
            Kind::BoxHeading => self.begin_paragraph(Role::BoxHeading, node.id.as_deref(), None),
            Kind::List { ordered, start } => {
                self.pending_anchor(node.id.as_deref());
                let id = self.numbering.list(*ordered, *start, 0)?;
                self.story.lists.push(id);
                Ok(())
            }
            Kind::ListItem => {
                self.pending_anchor(node.id.as_deref());
                self.story.list_item = true;
                Ok(())
            }
            Kind::Table {
                widths_percent,
                rows,
            } => self.start_table(widths_percent, *rows, node.id.as_deref()),
            Kind::TableRow {
                index,
                repeat_header,
                joins_next_row,
            } => self.start_row(*index, *repeat_header, *joins_next_row, node.id.as_deref()),
            Kind::TableCell(cell) => self.start_cell(*cell, node.id.as_deref()),
            Kind::Figure { caption } => {
                self.pending_anchor(node.id.as_deref());
                let before = self.theme.styles[&Role::Caption].caption_position
                    == Some(super::theme_format::CaptionPosition::Before);
                self.figures.push((caption.clone(), before));
                if before {
                    self.caption(caption)?;
                }
                Ok(())
            }
            Kind::Image(image) => self.image(image, node.id.as_deref()),
            Kind::Reference(reference) => self.reference(reference),
        }
    }

    fn caption(&mut self, caption: &str) -> Result<(), String> {
        if !caption.trim().is_empty() {
            self.begin_paragraph(Role::Caption, None, None)?;
            self.text(caption)?;
            self.end_paragraph()?;
        }
        Ok(())
    }

    fn caption_follows(&self) -> bool {
        self.figures
            .last()
            .is_some_and(|(caption, before)| !before && !caption.trim().is_empty())
    }

    fn keep_paragraph_with_next(&self, role: Role) -> bool {
        if role == Role::Caption {
            return self.figures.last().is_some_and(|(_, before)| *before);
        }
        if self.story.drawing || role == Role::Code {
            return self.caption_follows();
        }
        if matches!(role, Role::TableHeader | Role::TableCell) {
            let table_style = &self.theme.styles[&Role::Table];
            return table_style.keep_together
                || self.story.table.as_ref().is_some_and(|table| {
                    table.joins_next_row()
                        || (table.last_row()
                            && (self.caption_follows() || table_style.keep_with_next))
                });
        }
        false
    }

    fn exit(&mut self, node: &NodeInfo) -> Result<(), String> {
        match &node.kind {
            Kind::Document if !self.story.pending.is_empty() => {
                self.begin_paragraph(Role::Body, None, None)?;
                self.end_paragraph()
            }
            Kind::Paragraph | Kind::BoxHeading | Kind::Rule | Kind::Heading { .. } => {
                self.end_paragraph()
            }
            Kind::CodeBlock { .. } => {
                self.end_paragraph()?;
                self.end_frame()
            }
            Kind::Quote | Kind::Embed { .. } | Kind::Box(_) => self.end_frame(),
            Kind::List { .. } => {
                if !self.story.pending.is_empty() {
                    self.begin_paragraph(Role::List, None, None)?;
                    self.end_paragraph()?;
                }
                self.story.lists.pop();
                Ok(())
            }
            Kind::ListItem => {
                self.story.list_item = false;
                Ok(())
            }
            Kind::TableCell(_) => self.close_cell(),
            Kind::TableRow { .. } => self.end_row(),
            Kind::Table { .. } => self.end_table(),
            Kind::Figure { .. } => {
                let (caption, before) =
                    self.figures.pop().ok_or("DOCX figure capture is missing")?;
                if !before {
                    self.caption(&caption)?;
                }
                if !self.story.pending.is_empty() {
                    self.begin_paragraph(self.story.role(), None, None)?;
                    self.end_paragraph()?;
                }
                Ok(())
            }
            Kind::Reference(reference) => self.end_reference(reference),
            _ => Ok(()),
        }
    }
}

impl Formatter for DocxDocument<'_> {
    type Output = DocxBundle;
    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
        match event {
            RenderEvent::Enter(node) => self.enter(node),
            RenderEvent::Exit(node) => self.exit(node),
            RenderEvent::EnterMark(mark) => self.enter_mark(mark),
            RenderEvent::ExitMark(mark) => self.exit_mark(mark),
        }
        .map_err(ExportError::new)
    }
    fn finish(mut self) -> Result<DocxBundle, ExportError> {
        let finish = || -> Result<DocxBundle, String> {
            if !self.story.balanced() || self.saved_story.is_some() || !self.figures.is_empty() {
                return Err("DOCX document ended with unbalanced content".into());
            }
            if let Some(position) = self.toc_position
                && !self.contents.is_empty()
            {
                let fragment = print::contents(&self.contents, self.theme)?;
                self.story.xml.insert_fragment(position, fragment)?;
            }
            self.bookmarks.validate()?;
            if let Some(mut notes) = self.notes {
                notes.end()?;
                self.package.add(
                    notes::PART,
                    styles::content_type("footnotes"),
                    notes.finish()?,
                )?;
                self.package.relate(MAIN, "footnotes", notes::PART)?;
            }
            let print_parts = print::install(
                &mut self.package,
                self.theme,
                &self.title,
                self.first_chapter_anchor.as_deref(),
            )?;
            print_parts.section(&mut self.story.xml, self.theme)?;
            self.story.xml.end()?;
            self.story.xml.end()?;
            self.package.add(
                MAIN,
                styles::content_type("document.main"),
                self.story.xml.finish()?,
            )?;
            self.package.add(
                "word/numbering.xml",
                styles::content_type("numbering"),
                self.numbering
                    .compile(self.theme, &self.fonts, &self.language)?,
            )?;
            self.package
                .relate(MAIN, "numbering", "word/numbering.xml")?;
            self.fonts.install(&mut self.package)?;
            Ok(DocxBundle {
                bytes: self.package.finish()?,
            })
        };
        finish().map_err(ExportError::new)
    }
}

pub(crate) fn publish_docx(bundle: &DocxBundle, destination: &Path) -> Result<(), String> {
    super::output::publish_file(&bundle.bytes, destination, "Word", "docx")
}

fn canonical_language(value: &str) -> Result<String, String> {
    let (language, region) =
        super::source::parse_language(value).ok_or("DOCX language must be a valid language tag")?;
    Ok(region.map_or_else(|| language.clone(), |region| format!("{language}-{region}")))
}

fn settings() -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start("w:settings", &[("xmlns:w", W.into())])?;
    xml.value("w:displayBackgroundShape", 1)?;
    xml.value("w:embedTrueTypeFonts", 1)?;
    xml.value("w:embedSystemFonts", 0)?;
    xml.value("w:saveSubsetFonts", 0)?;
    xml.value("w:defaultTabStop", 720)?;
    xml.value("w:updateFields", 1)?;
    xml.start("w:compat", &[])?;
    xml.empty(
        "w:compatSetting",
        &[
            ("w:name", "compatibilityMode".into()),
            ("w:uri", "http://schemas.microsoft.com/office/word".into()),
            ("w:val", "15".into()),
        ],
    )?;
    xml.end()?;
    xml.end()?;
    xml.finish()
}

fn core_properties(title: &str, language: &str) -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start(
        "cp:coreProperties",
        &[
            (
                "xmlns:cp",
                "http://schemas.openxmlformats.org/package/2006/metadata/core-properties".into(),
            ),
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/".into()),
        ],
    )?;
    xml.element("dc:title", title)?;
    xml.element("dc:creator", "Bluink")?;
    xml.element("dc:language", language)?;
    xml.end()?;
    xml.finish()
}

fn app_properties() -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start(
        "Properties",
        &[(
            "xmlns",
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties".into(),
        )],
    )?;
    xml.element("Application", "Bluink")?;
    xml.end()?;
    xml.finish()
}
