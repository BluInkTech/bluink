// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! EPUB's manifest and spine are independent of Word's OPC relationships.
use crate::engine::xml::Xml;
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub(super) const XHTML: &str = "application/xhtml+xml";
pub(super) const NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
pub(super) const EPUB: &str = "http://www.idpf.org/2007/ops";

struct Part {
    kind: String,
    bytes: Vec<u8>,
}
#[derive(Default)]
pub(super) struct Package {
    parts: BTreeMap<String, Part>,
}

impl Package {
    pub(super) fn add(&mut self, path: &str, kind: &str, bytes: Vec<u8>) -> Result<(), String> {
        if path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) {
            return Err(format!("Unsafe EPUB resource path: {path}"));
        }
        if let Some(existing) = self.parts.get(path) {
            return if existing.kind == kind && existing.bytes == bytes {
                Ok(())
            } else {
                Err(format!("Conflicting EPUB resource: {path}"))
            };
        }
        self.parts.insert(
            path.into(),
            Part {
                kind: kind.into(),
                bytes,
            },
        );
        Ok(())
    }

    pub(super) fn finish(
        self,
        identifier: &str,
        title: &str,
        language: &str,
        chapters: &[super::Chapter],
    ) -> Result<Vec<u8>, String> {
        if chapters.is_empty() {
            return Err("EPUB must contain at least one chapter".into());
        }
        let opf = self.opf(identifier, title, language, chapters)?;
        let mut container = Xml::new();
        container.start(
            "container",
            &[
                ("version", "1.0".into()),
                (
                    "xmlns",
                    "urn:oasis:names:tc:opendocument:xmlns:container".into(),
                ),
            ],
        )?;
        container.start("rootfiles", &[])?;
        container.empty(
            "rootfile",
            &[
                ("full-path", "EPUB/book.opf".into()),
                ("media-type", "application/oebps-package+xml".into()),
            ],
        )?;
        container.end()?;
        container.end()?;
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        // OCF requires this first, stored, with no BOM, whitespace or extra fields.
        zip.start_file(
            "mimetype",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .map_err(|error| error.to_string())?;
        zip.write_all(b"application/epub+zip")
            .map_err(|error| error.to_string())?;
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (path, bytes) in [
            ("META-INF/container.xml".to_owned(), container.finish()?),
            ("EPUB/book.opf".into(), opf),
        ]
        .into_iter()
        .chain(
            self.parts
                .into_iter()
                .map(|(path, part)| (format!("EPUB/{path}"), part.bytes)),
        ) {
            zip.start_file(path, options)
                .map_err(|error| error.to_string())?;
            zip.write_all(&bytes).map_err(|error| error.to_string())?;
        }
        Ok(zip
            .finish()
            .map_err(|error| error.to_string())?
            .into_inner())
    }

    fn opf(
        &self,
        identifier: &str,
        title: &str,
        language: &str,
        chapters: &[super::Chapter],
    ) -> Result<Vec<u8>, String> {
        let mut xml = Xml::new();
        xml.start(
            "package",
            &[
                ("xmlns", "http://www.idpf.org/2007/opf".into()),
                ("version", "3.0".into()),
                ("unique-identifier", "publication-id".into()),
                ("xml:lang", language.into()),
            ],
        )?;
        xml.start(
            "metadata",
            &[("xmlns:dc", "http://purl.org/dc/elements/1.1/".into())],
        )?;
        xml.start("dc:identifier", &[("id", "publication-id".into())])?;
        xml.text(identifier)?;
        xml.end()?;
        xml.element("dc:title", title)?;
        xml.element("dc:language", language)?;
        xml.start("meta", &[("property", "dcterms:modified".into())])?;
        // Reproducible export proofs use the same conventional clock override
        // as Typst. Ordinary exports record their actual generation time.
        let modified = match std::env::var("SOURCE_DATE_EPOCH") {
            Ok(value) => chrono::DateTime::from_timestamp(
                value.parse().map_err(|_| "Invalid SOURCE_DATE_EPOCH")?,
                0,
            )
            .ok_or("SOURCE_DATE_EPOCH is out of range")?,
            Err(std::env::VarError::NotPresent) => chrono::Utc::now(),
            Err(_) => return Err("Invalid SOURCE_DATE_EPOCH".into()),
        };
        xml.text(&modified.format("%Y-%m-%dT%H:%M:%SZ").to_string())?;
        xml.end()?;
        xml.end()?;
        xml.start("manifest", &[])?;
        let mut ids = BTreeMap::new();
        for (index, (path, part)) in self.parts.iter().enumerate() {
            let id = format!("resource-{index}");
            let mut attrs = vec![
                ("id", id.clone()),
                ("href", path.clone()),
                ("media-type", part.kind.clone()),
            ];
            if path == "nav.xhtml" {
                attrs.push(("properties", "nav".into()));
            }
            xml.empty("item", &attrs)?;
            ids.insert(path.as_str(), id);
        }
        if !ids.contains_key("nav.xhtml") {
            return Err("EPUB navigation is missing".into());
        }
        xml.end()?;
        xml.start("spine", &[])?;
        for chapter in chapters {
            let id = ids
                .get(chapter.path.as_str())
                .ok_or("EPUB spine references an unregistered chapter")?;
            xml.empty("itemref", &[("idref", id.clone())])?;
        }
        xml.end()?;
        xml.end()?;
        xml.finish()
    }
}

pub(super) fn navigation(
    title: &str,
    language: &str,
    chapters: &[super::Chapter],
) -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start(
        "html",
        &[
            ("xmlns", NAMESPACE.into()),
            ("xmlns:epub", EPUB.into()),
            ("xml:lang", language.into()),
            ("lang", language.into()),
        ],
    )?;
    xml.start("head", &[])?;
    xml.element("title", title)?;
    xml.end()?;
    xml.start("body", &[])?;
    xml.start("nav", &[("epub:type", "toc".into()), ("id", "toc".into())])?;
    xml.element("h1", title)?;
    navigation_list(&mut xml, chapters, &mut 0, 0)?;
    xml.end()?;
    xml.end()?;
    xml.end()?;
    xml.finish()
}

fn navigation_list(
    xml: &mut Xml,
    chapters: &[super::Chapter],
    cursor: &mut usize,
    minimum: usize,
) -> Result<(), String> {
    xml.start("ol", &[])?;
    while let Some(chapter) = chapters.get(*cursor) {
        if chapter.depth < minimum {
            break;
        }
        let depth = chapter.depth;
        xml.start("li", &[])?;
        xml.start("a", &[("href", chapter.path.clone())])?;
        xml.text(&chapter.title)?;
        xml.end()?;
        *cursor += 1;
        if chapters.get(*cursor).is_some_and(|next| next.depth > depth) {
            navigation_list(xml, chapters, cursor, depth + 1)?;
        }
        xml.end()?;
    }
    xml.end()
}
