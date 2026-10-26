// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{DocxDocument, print, story::Story, word_root};
use crate::engine::{
    publication::{render::Reference, theme_format::Role},
    xml::Xml,
};

pub(super) const PART: &str = "word/footnotes.xml";

impl DocxDocument<'_> {
    pub(super) fn reference(&mut self, reference: &Reference) -> Result<(), String> {
        if self.saved_story.is_some() {
            return Err("DOCX does not allow nested footnotes".into());
        }
        if reference.first {
            if !self.note_ids.insert(reference.number) {
                return Err("DOCX reference has a duplicate definition".into());
            }
            let key = format!("reference-{}", reference.number);
            let (id, name) = self
                .bookmarks
                .define(&key)
                .ok_or("DOCX reference bookmark is already defined")?;
            self.story.xml.empty(
                "w:bookmarkStart",
                &[("w:id", id.to_string()), ("w:name", name)],
            )?;
            self.story.xml.start("w:r", &[])?;
            self.story.xml.start("w:rPr", &[])?;
            self.story.xml.value("w:rStyle", "FootnoteReference")?;
            self.story.xml.end()?;
            self.story.xml.empty(
                "w:footnoteReference",
                &[("w:id", reference.number.to_string())],
            )?;
            self.story.xml.end()?;
            self.story
                .xml
                .empty("w:bookmarkEnd", &[("w:id", id.to_string())])?;
            if self.notes.is_none() {
                self.notes = Some(footnotes()?);
            }
            let mut xml = Xml::fragment();
            xml.start("w:footnote", &[("w:id", reference.number.to_string())])?;
            let mut story = Story::new(xml, PART, self.story.width(), Role::Footnote);
            story.note_marker = true;
            self.saved_story = Some(std::mem::replace(&mut self.story, story));
        } else {
            if !self.note_ids.contains(&reference.number) {
                return Err("DOCX repeated reference has no definition".into());
            }
            let name = self
                .bookmarks
                .link(&format!("reference-{}", reference.number));
            print::field(
                &mut self.story.xml,
                &format!(" NOTEREF {name} \\h \\f "),
                &reference.number.to_string(),
                Some("FootnoteReference"),
            )?;
        }
        Ok(())
    }

    pub(super) fn end_reference(&mut self, reference: &Reference) -> Result<(), String> {
        if reference.first {
            if !self.story.balanced() {
                return Err("DOCX footnote ended with unbalanced content".into());
            }
            let parent = self
                .saved_story
                .take()
                .ok_or("DOCX footnote story is missing")?;
            let mut note = std::mem::replace(&mut self.story, parent);
            note.xml.end()?;
            self.notes
                .as_mut()
                .ok_or("DOCX footnotes part is missing")?
                .append_fragment(note.xml)?;
        }
        Ok(())
    }
}

fn footnotes() -> Result<Xml, String> {
    let mut xml = Xml::new();
    word_root(&mut xml, "w:footnotes")?;
    for (id, kind, mark) in [
        ("-1", "separator", "w:separator"),
        ("0", "continuationSeparator", "w:continuationSeparator"),
    ] {
        xml.start(
            "w:footnote",
            &[("w:type", kind.into()), ("w:id", id.into())],
        )?;
        xml.start("w:p", &[])?;
        xml.start("w:r", &[])?;
        xml.empty(mark, &[])?;
        xml.end()?;
        xml.end()?;
        xml.end()?;
    }
    Ok(xml)
}
