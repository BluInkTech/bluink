// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Own the OPC parts and relationship graph; ZIP is only a byte container.

use crate::engine::xml::Xml;
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const RELATIONS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
pub(super) const MAIN: &str = "word/document.xml";
pub(super) const WORD_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

struct Part {
    content_type: String,
    bytes: Vec<u8>,
}
#[derive(Clone)]
enum Target {
    Part(String),
    External(String),
}
struct Relationship {
    id: String,
    kind: String,
    target: Target,
}

#[derive(Default)]
pub(super) struct Package {
    parts: BTreeMap<String, Part>,
    relationships: BTreeMap<String, Vec<Relationship>>,
}

impl Package {
    pub(super) fn add(
        &mut self,
        path: &str,
        content_type: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if !safe_path(path)
            || std::path::Path::new(path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rels"))
            || path == "[Content_Types].xml"
            || path.contains("/_rels/")
        {
            return Err(format!("Invalid DOCX part path: {path}"));
        }
        if self.parts.contains_key(path) {
            return Err(format!("Duplicate DOCX part: {path}"));
        }
        self.parts.insert(
            path.into(),
            Part {
                content_type: content_type.into(),
                bytes,
            },
        );
        Ok(())
    }

    pub(super) fn relate(&mut self, owner: &str, kind: &str, part: &str) -> Result<String, String> {
        if !safe_path(part) {
            return Err(format!("Invalid relationship part: {part}"));
        }
        self.relationship(owner, kind, Target::Part(part.into()))
    }

    pub(super) fn hyperlink(&mut self, owner: &str, url: &str) -> Result<String, String> {
        if !super::super::render::safe_output_url(url, false) || url::Url::parse(url).is_err() {
            return Err(
                "DOCX external hyperlinks require an absolute HTTP, HTTPS or mailto URL".into(),
            );
        }
        self.relationship(owner, "hyperlink", Target::External(url.into()))
    }

    fn relationship(&mut self, owner: &str, kind: &str, target: Target) -> Result<String, String> {
        if (!owner.is_empty() && !safe_path(owner))
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || kind.is_empty()
        {
            return Err("Invalid DOCX relationship owner or kind".into());
        }
        let relationships = self.relationships.entry(owner.into()).or_default();
        let id = format!("rId{}", relationships.len() + 1);
        let kind = if kind == "core-properties" {
            format!("{RELATIONS}/metadata/core-properties")
        } else {
            format!("{OFFICE}/{kind}")
        };
        relationships.push(Relationship {
            id: id.clone(),
            kind,
            target,
        });
        Ok(id)
    }

    pub(super) fn finish(self) -> Result<Vec<u8>, String> {
        let roots = self
            .relationships
            .get("")
            .ok_or("DOCX package has no root relationships")?;
        if roots
            .iter()
            .filter(|relation| {
                relation.kind == format!("{OFFICE}/officeDocument")
                    && matches!(&relation.target, Target::Part(part) if part == MAIN)
            })
            .count()
            != 1
        {
            return Err("DOCX needs exactly one root officeDocument relationship".into());
        }
        let mut files = BTreeMap::new();
        files.insert("[Content_Types].xml".to_owned(), self.content_types()?);
        for (owner, relationships) in &self.relationships {
            if !owner.is_empty() && !self.parts.contains_key(owner) {
                return Err(format!("Relationship owner is missing: {owner}"));
            }
            for relation in relationships {
                if let Target::Part(target) = &relation.target
                    && !self.parts.contains_key(target)
                {
                    return Err(format!("Relationship target is missing: {target}"));
                }
            }
            files.insert(
                relationship_path(owner),
                relationship_xml(owner, relationships)?,
            );
        }
        for (path, part) in self.parts {
            files.insert(path, part.bytes);
        }
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (path, bytes) in files {
            zip.start_file(path, options)
                .map_err(|error| error.to_string())?;
            zip.write_all(&bytes).map_err(|error| error.to_string())?;
        }
        zip.finish()
            .map(Cursor::into_inner)
            .map_err(|error| error.to_string())
    }

    fn content_types(&self) -> Result<Vec<u8>, String> {
        let mut xml = Xml::new();
        xml.start("Types", &[("xmlns", CONTENT_TYPES.into())])?;
        for (extension, kind) in [
            (
                "rels",
                "application/vnd.openxmlformats-package.relationships+xml",
            ),
            ("xml", "application/xml"),
        ] {
            xml.empty(
                "Default",
                &[
                    ("Extension", extension.into()),
                    ("ContentType", kind.into()),
                ],
            )?;
        }
        for (path, part) in &self.parts {
            xml.empty(
                "Override",
                &[
                    ("PartName", format!("/{path}")),
                    ("ContentType", part.content_type.clone()),
                ],
            )?;
        }
        xml.end()?;
        xml.finish()
    }
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn relationship_path(owner: &str) -> String {
    if owner.is_empty() {
        "_rels/.rels".into()
    } else if let Some((folder, file)) = owner.rsplit_once('/') {
        format!("{folder}/_rels/{file}.rels")
    } else {
        format!("_rels/{owner}.rels")
    }
}

fn relative_target(owner: &str, target: &str) -> String {
    let folder = owner.rsplit_once('/').map_or("", |(folder, _)| folder);
    let source = folder
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let target = target.split('/').collect::<Vec<_>>();
    let common = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    std::iter::repeat_n("..", source.len() - common)
        .chain(target[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

fn relationship_xml(owner: &str, relationships: &[Relationship]) -> Result<Vec<u8>, String> {
    let mut xml = Xml::new();
    xml.start("Relationships", &[("xmlns", RELATIONS.into())])?;
    for relation in relationships {
        let mut attributes = vec![("Id", relation.id.clone()), ("Type", relation.kind.clone())];
        match &relation.target {
            Target::Part(target) => attributes.push(("Target", relative_target(owner, target))),
            Target::External(url) => {
                attributes.push(("Target", url.clone()));
                attributes.push(("TargetMode", "External".into()));
            }
        }
        xml.empty("Relationship", &attributes)?;
    }
    xml.end()?;
    xml.finish()
}
