// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::{
    render::{Citation, Destination, Footnote, Mark},
    theme_format::Role,
};
use crate::domain::xml::Xml;
use crate::export_jobs::docx::{DocxDocument, story::Story, word_root};

pub(super) const PART: &str = "word/footnotes.xml";

impl DocxDocument<'_> {
    /// A native footnote: Word places it at the foot of the page and numbers
    /// it within its section. Its content follows until `end_footnote`.
    pub(super) fn footnote(&mut self, footnote: Footnote) -> Result<(), String> {
        if self.saved_story.is_some() {
            return Err("DOCX does not allow nested footnotes".into());
        }
        // A raised comma keeps adjacent markers from reading as one number.
        if self.story.reference_end == Some(self.story.xml.position()) {
            let xml = &mut self.story.xml;
            xml.start("w:r", &[])?;
            xml.start("w:rPr", &[])?;
            xml.value("w:rStyle", "FootnoteReference")?;
            xml.end()?;
            xml.start("w:t", &[])?;
            xml.text(",")?;
            xml.end()?;
            xml.end()?;
        }
        // Word needs IDs unique in the whole document; the number shown is the
        // section's own.
        let note = self.note_offset + footnote.number;
        if !self.note_ids.insert(note) {
            return Err("DOCX footnote has a duplicate number".into());
        }
        self.story.xml.start("w:r", &[])?;
        self.story.xml.start("w:rPr", &[])?;
        self.story.xml.value("w:rStyle", "FootnoteReference")?;
        self.story.xml.end()?;
        self.story
            .xml
            .empty("w:footnoteReference", &[("w:id", note.to_string())])?;
        self.story.xml.end()?;
        self.story.reference_end = Some(self.story.xml.position());
        if self.notes.is_none() {
            self.notes = Some(footnotes()?);
        }
        let mut xml = Xml::fragment();
        xml.start("w:footnote", &[("w:id", note.to_string())])?;
        let mut story = Story::new(xml, PART, self.story.width(), Role::Footnote);
        story.note_marker = true;
        self.saved_story = Some(std::mem::replace(&mut self.story, story));
        Ok(())
    }

    pub(super) fn end_footnote(&mut self) -> Result<(), String> {
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
            .append_fragment(note.xml)
    }

    /// Citations side by side as one bracket, each number linking to its
    /// source's entry in the reference list.
    pub(super) fn citations(&mut self, citations: &[Citation]) -> Result<(), String> {
        self.text("[")?;
        for (index, citation) in citations.iter().enumerate() {
            if index > 0 {
                self.text(", ")?;
            }
            let link = Mark::Link(Destination::Anchor(citation.target.clone()));
            self.enter_mark(&link)?;
            self.text(&citation.number.to_string())?;
            self.exit_mark(&link)?;
        }
        self.text("]")
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
