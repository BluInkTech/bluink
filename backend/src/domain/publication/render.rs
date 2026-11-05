// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Private, format-independent preparation and recursive traversal.
//! Serializers receive events without access to source JSON or child collections.

mod prepare;
mod resources;
#[cfg(test)]
mod tests;

use std::{collections::HashMap, error::Error, fmt};

pub(crate) use prepare::{Resources, prepare_page};
pub(crate) use resources::safe_output_url;
pub(crate) use resources::{linked_anchors, native_links, native_links_by_page};

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

/// A framed box: plain, or one of five intents. A quote box is prepared as
/// a quotation instead, since its meaning is someone's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxKind {
    Plain,
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl BoxKind {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "plain" => Ok(Self::Plain),
            "note" => Ok(Self::Note),
            "tip" => Ok(Self::Tip),
            "important" => Ok(Self::Important),
            "warning" => Ok(Self::Warning),
            "caution" => Ok(Self::Caution),
            _ => Err(format!("Unsupported box kind: {value}")),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Caution => "caution",
        }
    }

    /// The theme role that styles this box.
    pub(crate) const fn role(self) -> crate::domain::publication::theme_format::Role {
        use crate::domain::publication::theme_format::Role;
        match self {
            Self::Plain => Role::Box,
            Self::Note => Role::BoxNote,
            Self::Tip => Role::BoxTip,
            Self::Important => Role::BoxImportant,
            Self::Warning => Role::BoxWarning,
            Self::Caution => Role::BoxCaution,
        }
    }

    /// The heading an intent box shows when its author gave it none, so its
    /// meaning never rests on colour alone. A plain frame needs none.
    pub(crate) const fn label(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Note => Some("Note"),
            Self::Tip => Some("Tip"),
            Self::Important => Some("Important"),
            Self::Warning => Some("Warning"),
            Self::Caution => Some("Caution"),
        }
    }
}

/// A footnote, numbered within its scope. Its content follows as children,
/// for each format to place at the foot of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Footnote {
    pub(crate) number: usize,
}

/// A cited source: its number in the reference list, and the anchor its
/// entry there carries so markers can link to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Citation {
    pub(crate) number: usize,
    pub(crate) target: String,
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
    /// Who said a quotation, after its words.
    QuoteAttribution,
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
    Footnote(Footnote),
    /// Citations side by side, shown as one marker such as [3, 7].
    Citations(Vec<Citation>),
    /// A scope's cited sources, gathered at the end of a chapter or book:
    /// a heading, then one item per source.
    ReferenceList,
    /// One source in a reference list; its children are the source's content.
    ReferenceItem(Citation),
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

/// Numbers footnotes and citations for one chapter or one book. Footnotes and
/// citations count separately; a source cited again keeps its number, and
/// each source's content waits here until its list is taken.
#[derive(Debug, Default)]
pub(crate) struct ReferenceScope {
    /// Namespaces list anchors when several scopes share one output.
    name: String,
    footnotes: usize,
    citations: usize,
    sources: HashMap<String, usize>,
    items: Vec<(Citation, Vec<Node>)>,
}

impl ReferenceScope {
    /// A scope whose list anchors carry this name, such as a chapter's page ID.
    pub(crate) fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    fn target(&self, number: usize) -> String {
        if self.name.is_empty() {
            format!("ref-{number}")
        } else {
            format!("ref-{}-{number}", self.name)
        }
    }

    /// The sources cited so far, as a list to render after the text that
    /// cited them, or nothing when none were. Taking the list empties it.
    /// Without a heading, the caller provides its own, as a book does for a
    /// list of its own in the contents.
    pub(crate) fn take_list(
        &mut self,
        page_id: &str,
        heading: Option<(&str, u16)>,
    ) -> Option<PreparedPage> {
        if self.items.is_empty() {
            return None;
        }
        let leaf = |kind| Node {
            info: NodeInfo { id: None, kind },
            marks: Vec::new(),
            children: Vec::new(),
        };
        let mut children: Vec<Node> = heading
            .map(|(text, level)| Node {
                children: vec![leaf(Kind::Text(text.to_owned()))],
                ..leaf(Kind::Heading { level })
            })
            .into_iter()
            .collect();
        children.extend(self.items.drain(..).map(|(citation, content)| Node {
            children: content,
            ..leaf(Kind::ReferenceItem(citation))
        }));
        Some(PreparedPage {
            id: page_id.to_owned(),
            root: Node {
                children,
                ..leaf(Kind::ReferenceList)
            },
        })
    }
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

/// Names the page an error came from, unless the error already starts with it.
pub(crate) fn on_page(page_id: &str, error: String) -> String {
    if error.starts_with(&format!("Page {page_id}")) {
        error
    } else {
        format!("Page {page_id}: {error}")
    }
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
