// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Small shared XML writer. Names are internal constants; values are always escaped.

use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};

pub(crate) struct Xml {
    writer: Writer<Vec<u8>>,
    stack: Vec<String>,
}

impl Xml {
    pub(crate) const fn fragment() -> Self {
        Self {
            writer: Writer::new(Vec::new()),
            stack: Vec::new(),
        }
    }

    pub(crate) const fn position(&self) -> usize {
        self.writer.get_ref().len()
    }

    /// Only balanced fragments built by this writer can be inserted. Source text
    /// cannot bypass escaping through a raw-string API.
    pub(crate) fn insert_fragment(
        &mut self,
        position: usize,
        fragment: Self,
    ) -> Result<(), String> {
        let bytes = fragment.finish()?;
        if position > self.writer.get_ref().len() {
            return Err("XML insertion position is out of range".into());
        }
        self.writer.get_mut().splice(position..position, bytes);
        Ok(())
    }

    pub(crate) fn append_fragment(&mut self, fragment: Self) -> Result<(), String> {
        self.insert_fragment(self.position(), fragment)
    }
    #[expect(
        clippy::expect_used,
        reason = "A fixed valid XML declaration written to an owned Vec has no fallible I/O."
    )]
    pub(crate) fn new() -> Self {
        let mut writer = Writer::new(Vec::new());
        writer
            .write_event(Event::Decl(BytesDecl::new(
                "1.0",
                Some("UTF-8"),
                Some("yes"),
            )))
            .expect("writing XML to a Vec cannot fail");
        Self {
            writer,
            stack: Vec::new(),
        }
    }

    fn tag<'a>(name: &'a str, attributes: &[(&str, String)]) -> Result<BytesStart<'a>, String> {
        let mut tag = BytesStart::new(name);
        for (key, value) in attributes {
            valid_characters(value)?;
            tag.push_attribute((*key, value.as_str()));
        }
        Ok(tag)
    }

    pub(crate) fn start(
        &mut self,
        name: &str,
        attributes: &[(&str, String)],
    ) -> Result<(), String> {
        self.writer
            .write_event(Event::Start(Self::tag(name, attributes)?))
            .map_err(|error| error.to_string())?;
        self.stack.push(name.to_owned());
        Ok(())
    }

    pub(crate) fn empty(
        &mut self,
        name: &str,
        attributes: &[(&str, String)],
    ) -> Result<(), String> {
        self.writer
            .write_event(Event::Empty(Self::tag(name, attributes)?))
            .map_err(|error| error.to_string())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "The writer accepts temporary numbers and owned strings through one value API."
    )]
    pub(crate) fn value(&mut self, name: &str, value: impl ToString) -> Result<(), String> {
        self.empty(name, &[("w:val", value.to_string())])
    }

    pub(crate) fn text(&mut self, text: &str) -> Result<(), String> {
        valid_characters(text)?;
        self.writer
            .write_event(Event::Text(BytesText::new(text)))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn end(&mut self) -> Result<(), String> {
        let name = self.stack.pop().ok_or("XML end has no matching start")?;
        self.writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn element(&mut self, name: &str, text: &str) -> Result<(), String> {
        self.start(name, &[])?;
        self.text(text)?;
        self.end()
    }

    pub(crate) fn finish(self) -> Result<Vec<u8>, String> {
        if !self.stack.is_empty() {
            return Err(format!(
                "XML has unclosed elements: {}",
                self.stack.join(", ")
            ));
        }
        Ok(self.writer.into_inner())
    }
}

pub(crate) fn valid_characters(value: &str) -> Result<(), String> {
    if value.chars().any(|character| !matches!(character,
        '\u{9}' | '\u{a}' | '\u{d}' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        return Err("Text contains a character prohibited by XML 1.0".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_fragments_preserve_escaping_order_and_parent_balance() {
        let mut xml = Xml::new();
        xml.start("root", &[]).unwrap();
        let insertion = xml.position();
        xml.element("later", "café").unwrap();
        let mut fragment = Xml::fragment();
        fragment.element("earlier", "<first> & safe").unwrap();
        xml.insert_fragment(insertion, fragment).unwrap();
        let mut unfinished = Xml::fragment();
        unfinished.start("unfinished", &[]).unwrap();
        let before = xml.position();
        assert!(xml.append_fragment(unfinished).is_err());
        assert_eq!(xml.position(), before);
        assert!(xml.insert_fragment(usize::MAX, Xml::fragment()).is_err());
        let mut tail = Xml::fragment();
        tail.element("last", "done").unwrap();
        xml.append_fragment(tail).unwrap();
        xml.end().unwrap();
        assert!(String::from_utf8(xml.finish().unwrap()).unwrap().ends_with(
            "<root><earlier>&lt;first&gt; &amp; safe</earlier><later>café</later><last>done</last></root>"
        ));
    }

    #[test]
    fn escapes_values_without_double_escaping_and_rejects_invalid_xml() {
        let mut xml = Xml::new();
        xml.start(
            "root",
            &[("title", "A & B < C > D \"quoted\" 'single'".into())],
        )
        .unwrap();
        xml.text("text & <tag> café 😀").unwrap();
        xml.end().unwrap();
        let result = String::from_utf8(xml.finish().unwrap()).unwrap();
        assert!(result.contains("&amp; B &lt; C &gt; D &quot;quoted&quot; &apos;single&apos;"));
        assert!(result.contains("text &amp; &lt;tag&gt; café 😀"));
        assert!(!result.contains("&amp;amp;"));
        assert!(Xml::new().text("bad\u{0}text").is_err());
        assert!(
            Xml::new()
                .empty("item", &[("title", "bad\u{b}".into())])
                .is_err()
        );
        assert!(Xml::new().end().is_err());
        let mut unfinished = Xml::new();
        unfinished.start("root", &[]).unwrap();
        assert!(unfinished.finish().is_err());
    }
}
