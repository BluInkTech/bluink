// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Private, format-independent preparation and recursive traversal.
//! Serializers receive events without access to source JSON or child collections.

mod prepare;
mod resources;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

pub(crate) use prepare::{Resources, prepare_page};
pub(crate) use resources::safe_output_url;
pub(crate) use resources::{native_links, read_asset};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Url(String),
    Anchor(String),
    PlainText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Strong,
    Emphasis,
    Underline,
    Strike,
    Code,
    Superscript,
    Subscript,
    Link(Destination),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxKind {
    Info,
    Abstract,
    Note,
    Tip,
    Success,
    Important,
    Warning,
    Error,
    Question,
    Example,
}

impl BoxKind {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "info" => Ok(Self::Info),
            "abstract" => Ok(Self::Abstract),
            "note" => Ok(Self::Note),
            "tip" => Ok(Self::Tip),
            "success" => Ok(Self::Success),
            "important" => Ok(Self::Important),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            "question" => Ok(Self::Question),
            "example" => Ok(Self::Example),
            _ => Err(format!("Unsupported box kind: {value}")),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Abstract => "abstract",
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Success => "success",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Question => "question",
            Self::Example => "example",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    Footnote,
    Citation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reference {
    pub(crate) number: usize,
    pub(crate) occurrence: usize,
    pub(crate) kind: ReferenceKind,
    pub(crate) snippet_id: Option<String>,
    pub(crate) first: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Image {
    pub(crate) source: String,
    pub(crate) asset: String,
    pub(crate) alt: String,
    /// Authored image dimensions are CSS pixels, normalized once to points.
    pub(crate) width_pt: Option<f64>,
    pub(crate) height_pt: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) colspan: usize,
    pub(crate) rowspan: usize,
    pub(crate) header: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    Document,
    Paragraph,
    Heading {
        level: u16,
    },
    Quote,
    Box(BoxKind),
    BoxHeading,
    List {
        ordered: bool,
        start: u64,
    },
    ListItem,
    Table {
        widths_percent: Vec<f64>,
        rows: usize,
    },
    TableRow {
        index: usize,
        repeat_header: bool,
        joins_next_row: bool,
    },
    TableCell(Cell),
    Figure {
        caption: String,
    },
    Image(Image),
    CodeBlock {
        text: String,
        language: String,
    },
    Text(String),
    HardBreak,
    Rule,
    Embed {
        snippet_id: String,
    },
    Reference(Reference),
}

/// Only this descriptor is visible to a formatter; children remain private.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NodeInfo {
    pub(crate) id: Option<String>,
    pub(crate) kind: Kind,
}

#[derive(Debug)]
struct Node {
    info: NodeInfo,
    marks: Vec<Mark>,
    children: Vec<Self>,
}

#[derive(Debug)]
pub(crate) struct PreparedPage {
    id: String,
    root: Node,
}

/// Owned by the document orchestrator. A site creates a fresh scope per page.
#[derive(Debug, Default)]
pub(crate) struct ReferenceScope {
    next: usize,
    citations: HashMap<String, usize>,
    occurrences: HashMap<usize, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExportError {
    pub(crate) page_id: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) message: String,
}

impl ExportError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            page_id: None,
            block_id: None,
            message: message.into(),
        }
    }

    fn at(mut self, page_id: &str, block_id: Option<&str>) -> Self {
        if self.page_id.is_none() {
            self.page_id = Some(page_id.to_owned());
        }
        if self.block_id.is_none() {
            self.block_id = block_id.map(str::to_owned);
        }
        self
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(page) = &self.page_id {
            write!(formatter, "Page {page}")?;
        }
        if let Some(block) = &self.block_id {
            write!(formatter, ", block {block}")?;
        }
        if self.page_id.is_some() || self.block_id.is_some() {
            formatter.write_str(": ")?;
        }
        formatter.write_str(&self.message)
    }
}

impl Error for ExportError {}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RenderEvent<'a> {
    Enter(&'a NodeInfo),
    Exit(&'a NodeInfo),
    EnterMark(&'a Mark),
    ExitMark(&'a Mark),
}

pub(crate) trait Formatter {
    type Output;
    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError>;
    fn finish(self) -> Result<Self::Output, ExportError>;
}

/// Walk one prepared page; a formatter may retain output state across pages.
pub(crate) fn walk(page: &PreparedPage, formatter: &mut impl Formatter) -> Result<(), ExportError> {
    visit(&page.root, &page.id, None, formatter)
}

pub(crate) fn render<F: Formatter>(
    page: &PreparedPage,
    mut formatter: F,
) -> Result<F::Output, ExportError> {
    walk(page, &mut formatter)?;
    formatter.finish().map_err(|error| error.at(&page.id, None))
}

fn visit(
    node: &Node,
    page: &str,
    block: Option<&str>,
    formatter: &mut impl Formatter,
) -> Result<(), ExportError> {
    let block = node.info.id.as_deref().or(block);
    for mark in &node.marks {
        formatter
            .emit(RenderEvent::EnterMark(mark))
            .map_err(|error| error.at(page, block))?;
    }
    formatter
        .emit(RenderEvent::Enter(&node.info))
        .map_err(|error| error.at(page, block))?;
    for child in &node.children {
        visit(child, page, block, formatter)?;
    }
    formatter
        .emit(RenderEvent::Exit(&node.info))
        .map_err(|error| error.at(page, block))?;
    for mark in node.marks.iter().rev() {
        formatter
            .emit(RenderEvent::ExitMark(mark))
            .map_err(|error| error.at(page, block))?;
    }
    Ok(())
}
