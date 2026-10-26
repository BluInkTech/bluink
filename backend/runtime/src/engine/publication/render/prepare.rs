// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::engine::document::validation::validate_document;
use crate::engine::project::snippets::validate_reference_paragraph;

use super::{
    BoxKind, Cell, Destination, ExportError, Image, Kind, Mark, Node, NodeInfo, PreparedPage,
    Reference, ReferenceKind, ReferenceScope, safe_output_url,
};

pub(crate) struct Resources<'a> {
    pub(crate) links: &'a HashMap<String, Destination>,
    pub(crate) assets: &'a HashMap<String, String>,
    pub(crate) fields: &'a HashMap<String, String>,
    pub(crate) snippets: &'a HashMap<String, Value>,
    pub(crate) excluded_block_ids: &'a HashSet<String>,
}

/// Reference changes are transactional: a failed page leaves its scope untouched.
/// Only this page's changes are staged, rather than cloning the whole document scope.
struct Draft<'a> {
    base: &'a ReferenceScope,
    next: usize,
    citations: HashMap<String, usize>,
    occurrences: HashMap<usize, usize>,
}

impl Draft<'_> {
    fn reference(&mut self, kind: ReferenceKind, key: Option<&str>) -> Reference {
        let existing = key
            .and_then(|key| {
                self.citations
                    .get(key)
                    .or_else(|| self.base.citations.get(key))
            })
            .copied();
        let number = existing.unwrap_or_else(|| {
            self.next += 1;
            if let Some(key) = key {
                self.citations.insert(key.to_owned(), self.next);
            }
            self.next
        });
        let occurrence = self
            .occurrences
            .entry(number)
            .or_insert_with(|| self.base.occurrences.get(&number).copied().unwrap_or(0));
        *occurrence += 1;
        Reference {
            number,
            occurrence: *occurrence,
            kind,
            snippet_id: key.map(str::to_owned),
            first: existing.is_none(),
        }
    }
}

struct Preparer<'a, 'b> {
    page: &'a str,
    resources: &'a Resources<'b>,
    heading_offset: i16,
    references: Draft<'a>,
}

pub(crate) fn prepare_page(
    page_id: &str,
    document: &Value,
    resources: &Resources<'_>,
    heading_offset: i16,
    scope: &mut ReferenceScope,
) -> Result<PreparedPage, ExportError> {
    validate_document(document)
        .map_err(|error| ExportError::new(error.to_string()).at(page_id, None))?;
    let mut preparer = Preparer {
        page: page_id,
        resources,
        heading_offset,
        references: Draft {
            base: scope,
            next: scope.next,
            citations: HashMap::new(),
            occurrences: HashMap::new(),
        },
    };
    let root = preparer.node(document, None)?.ok_or_else(|| {
        ExportError::new("The document root cannot be excluded").at(page_id, None)
    })?;
    let Draft {
        next,
        citations,
        occurrences,
        ..
    } = preparer.references;
    scope.next = next;
    scope.citations.extend(citations);
    scope.occurrences.extend(occurrences);
    Ok(PreparedPage {
        id: page_id.to_owned(),
        root,
    })
}

impl Preparer<'_, '_> {
    fn error(&self, block: Option<&str>, message: impl Into<String>) -> ExportError {
        ExportError::new(message).at(self.page, block)
    }

    fn excluded(&self, node: &Value) -> bool {
        node["attrs"]["id"]
            .as_str()
            .is_some_and(|id| self.resources.excluded_block_ids.contains(id))
    }

    fn node(&mut self, source: &Value, parent: Option<&str>) -> Result<Option<Node>, ExportError> {
        if self.excluded(source) {
            return Ok(None);
        }
        let id = source["attrs"]["id"].as_str().map(str::to_owned);
        let block = id.as_deref().or(parent);
        let kind = self.kind(source, block)?;
        let marks = self.marks(source, block)?;
        let children = match &kind {
            Kind::Text(_)
            | Kind::Image(_)
            | Kind::CodeBlock { .. }
            | Kind::HardBreak
            | Kind::Rule => Vec::new(),
            Kind::Figure { .. } => self
                .node(&children(source)[1], block)?
                .into_iter()
                .collect(),
            Kind::Embed { snippet_id } => vec![self.snippet(snippet_id, block)?],
            Kind::Reference(reference) => {
                if reference.first {
                    let body = if let Some(key) = &reference.snippet_id {
                        self.snippet(key, block)?
                    } else {
                        self.node(&source["attrs"]["paragraph"], block)?
                            .ok_or_else(|| self.error(block, "Missing reference paragraph"))?
                    };
                    vec![body]
                } else {
                    Vec::new()
                }
            }
            Kind::Table { widths_percent, .. } => {
                self.table_rows(source, widths_percent.len(), block)?
            }
            _ => self.children(source, block)?,
        };
        Ok(Some(Node {
            info: NodeInfo { id, kind },
            marks,
            children,
        }))
    }

    fn children(&mut self, source: &Value, block: Option<&str>) -> Result<Vec<Node>, ExportError> {
        children(source)
            .iter()
            .filter_map(|child| self.node(child, block).transpose())
            .collect()
    }

    fn snippet(&mut self, key: &str, block: Option<&str>) -> Result<Node, ExportError> {
        let paragraph = self.resources.snippets.get(key).ok_or_else(|| {
            self.error(
                block,
                format!("Snippet {key} was not resolved by preflight"),
            )
        })?;
        validate_reference_paragraph(paragraph, true)
            .map_err(|error| self.error(block, format!("Snippet {key}: {error}")))?;
        self.node(paragraph, block)?
            .ok_or_else(|| self.error(block, "Snippet content cannot be excluded"))
    }

    fn marks(&self, source: &Value, block: Option<&str>) -> Result<Vec<Mark>, ExportError> {
        let mut marks = Vec::new();
        for mark in source["marks"].as_array().into_iter().flatten() {
            let kind = match mark["type"].as_str().unwrap_or("") {
                "strong" => Mark::Strong,
                "emphasis" => Mark::Emphasis,
                "underline" => Mark::Underline,
                "strike" => Mark::Strike,
                "code" => Mark::Code,
                "superscript" => Mark::Superscript,
                "subscript" => Mark::Subscript,
                "link" => {
                    let href = mark["attrs"]["href"]
                        .as_str()
                        .ok_or_else(|| self.error(block, "Link has no destination"))?;
                    let destination = self.resources.links.get(href).ok_or_else(|| {
                        self.error(block, format!("Link {href} was not resolved by preflight"))
                    })?;
                    match destination {
                        Destination::PlainText => continue,
                        Destination::Url(value) if !safe_output_url(value, false) => {
                            return Err(self.error(
                                block,
                                format!("Unsafe resolved link destination: {value}"),
                            ));
                        }
                        Destination::Anchor(value)
                            if value.is_empty() || value.chars().any(char::is_control) =>
                        {
                            return Err(self.error(block, "Invalid resolved anchor"));
                        }
                        _ => {}
                    }
                    Mark::Link(destination.clone())
                }
                other => return Err(self.error(block, format!("Unsupported mark: {other}"))),
            };
            marks.push(kind);
        }
        // Links wrap semantic marks so an explicit link style cannot erase an
        // authored strong/superscript mark merely because of source mark order.
        marks.sort_by_key(|mark| !matches!(mark, Mark::Link(_)));
        Ok(marks)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "One exhaustive dispatch translates the validated document schema into typed export semantics."
    )]
    fn kind(&mut self, source: &Value, block: Option<&str>) -> Result<Kind, ExportError> {
        let attrs = &source["attrs"];
        Ok(match source["type"].as_str().unwrap_or("") {
            "doc" => Kind::Document,
            "paragraph" => Kind::Paragraph,
            "heading" => {
                let level = i32::try_from(attrs["level"].as_u64().unwrap_or(1))
                    .map_err(|_| self.error(block, "Invalid heading level"))?
                    + i32::from(self.heading_offset);
                let level = u16::try_from(level)
                    .ok()
                    .filter(|level| *level >= 1)
                    .ok_or_else(|| {
                        self.error(
                            block,
                            "Heading level fell below 1 or exceeded supported nesting",
                        )
                    })?;
                Kind::Heading { level }
            }
            "blockquote" => Kind::Quote,
            "box" => Kind::Box(
                BoxKind::parse(attrs["kind"].as_str().unwrap_or("info"))
                    .map_err(|error| self.error(block, error))?,
            ),
            "boxHeading" => Kind::BoxHeading,
            "list" => Kind::List {
                ordered: attrs["kind"] == "ordered",
                start: attrs["order"].as_u64().unwrap_or(1),
            },
            "listItem" => Kind::ListItem,
            "figure" => {
                let caption = children(&children(source)[0])
                    .iter()
                    .map(|child| child["text"].as_str().unwrap_or(""))
                    .collect();
                Kind::Figure { caption }
            }
            "image" => {
                let src = attrs["src"]
                    .as_str()
                    .ok_or_else(|| self.error(block, "Image has no source"))?;
                let asset = self.resources.assets.get(src).ok_or_else(|| {
                    self.error(block, format!("Image {src} was not resolved by preflight"))
                })?;
                if !safe_output_url(asset, true) {
                    return Err(
                        self.error(block, format!("Unsafe resolved image destination: {asset}"))
                    );
                }
                let alt = attrs["alt"]
                    .as_str()
                    .filter(|alt| !alt.trim().is_empty())
                    .ok_or_else(|| self.error(block, "Image needs descriptive alt text"))?;
                let dimension = |key: &str| {
                    attrs[key]
                        .as_u64()
                        .and_then(|size| u32::try_from(size).ok())
                        .map(|size| f64::from(size) * 0.75)
                };
                Kind::Image(Image {
                    source: src.to_owned(),
                    asset: asset.clone(),
                    alt: alt.to_owned(),
                    width_pt: dimension("width"),
                    height_pt: dimension("height"),
                })
            }
            "code_block" => Kind::CodeBlock {
                text: children(source)
                    .iter()
                    .map(|child| child["text"].as_str().unwrap_or(""))
                    .collect(),
                language: attrs["language"].as_str().unwrap_or("plain").to_owned(),
            },
            "text" => Kind::Text(
                source["text"]
                    .as_str()
                    .ok_or_else(|| self.error(block, "Text node has no text"))?
                    .to_owned(),
            ),
            "field" => {
                let key = attrs["key"]
                    .as_str()
                    .ok_or_else(|| self.error(block, "Field has no key"))?;
                Kind::Text(
                    self.resources
                        .fields
                        .get(key)
                        .ok_or_else(|| {
                            self.error(block, format!("Field {key} was not resolved by preflight"))
                        })?
                        .clone(),
                )
            }
            "hard_break" => Kind::HardBreak,
            "horizontal_rule" => Kind::Rule,
            "embed" => Kind::Embed {
                snippet_id: attrs["snippetId"]
                    .as_str()
                    .ok_or_else(|| self.error(block, "Snippet embed has no key"))?
                    .to_owned(),
            },
            "footnote" | "citation" => {
                let kind = if source["type"] == "footnote" {
                    ReferenceKind::Footnote
                } else {
                    ReferenceKind::Citation
                };
                let key = if kind == ReferenceKind::Citation && attrs["source"] == "snippet" {
                    attrs["snippetId"].as_str()
                } else {
                    None
                };
                Kind::Reference(self.references.reference(kind, key))
            }
            "table" => Kind::Table {
                widths_percent: attrs["columnWidthsPercent"]
                    .as_array()
                    .ok_or_else(|| self.error(block, "Table has no column widths"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .ok_or_else(|| self.error(block, "Invalid table width"))
                    })
                    .collect::<Result<_, _>>()?,
                rows: children(source).len(),
            },
            other => return Err(self.error(block, format!("Unsupported document node: {other}"))),
        })
    }

    fn table_rows(
        &mut self,
        source: &Value,
        columns: usize,
        block: Option<&str>,
    ) -> Result<Vec<Node>, ExportError> {
        let mut rows = Vec::new();
        let mut occupied_until = vec![0; columns];
        for (index, source_row) in children(source).iter().enumerate() {
            let suppressed_row = self.excluded(source_row);
            let row_id = (!suppressed_row)
                .then(|| source_row["attrs"]["id"].as_str())
                .flatten();
            let row_block = row_id.or(block);
            let mut cells = Vec::new();
            let mut column = 0;
            for source_cell in children(source_row) {
                while column < columns && occupied_until[column] > index {
                    column += 1;
                }
                let colspan =
                    usize::try_from(source_cell["attrs"]["colspan"].as_u64().unwrap_or(1))
                        .map_err(|_| self.error(row_block, "Invalid colspan"))?;
                let rowspan =
                    usize::try_from(source_cell["attrs"]["rowspan"].as_u64().unwrap_or(1))
                        .map_err(|_| self.error(row_block, "Invalid rowspan"))?;
                let end = column
                    .checked_add(colspan)
                    .filter(|end| *end <= columns)
                    .ok_or_else(|| self.error(row_block, "Table cell exceeds its grid"))?;
                occupied_until[column..end].fill(index + rowspan);
                let suppressed = suppressed_row || self.excluded(source_cell);
                let id = (!suppressed)
                    .then(|| source_cell["attrs"]["id"].as_str())
                    .flatten();
                let children = if suppressed {
                    Vec::new()
                } else {
                    self.children(source_cell, id.or(row_block))?
                };
                let marks = if suppressed {
                    Vec::new()
                } else {
                    self.marks(source_cell, id.or(row_block))?
                };
                let header = source_cell["type"] == "table_header";
                cells.push(Node {
                    info: NodeInfo {
                        id: id.map(str::to_owned),
                        kind: Kind::TableCell(Cell {
                            row: index,
                            column,
                            colspan,
                            rowspan,
                            header,
                        }),
                    },
                    marks,
                    children,
                });
                column = end;
            }
            let repeat_header = index == 0
                && !suppressed_row
                && !cells.is_empty()
                && cells
                    .iter()
                    // Repeating a partial row-spanning header would overlap
                    // body cells on the following page in paginated formats.
                    .all(|cell| matches!(&cell.info.kind, Kind::TableCell(cell) if cell.header && cell.rowspan == 1));
            rows.push(Node {
                info: NodeInfo {
                    id: row_id.map(str::to_owned),
                    kind: Kind::TableRow {
                        index,
                        repeat_header,
                        joins_next_row: occupied_until.iter().any(|end| *end > index + 1),
                    },
                },
                marks: if suppressed_row {
                    Vec::new()
                } else {
                    self.marks(source_row, row_block)?
                },
                children: cells,
            });
        }
        Ok(rows)
    }
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map_or(&[], Vec::as_slice)
}
