// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) use super::render::Destination;
use super::render::{
    self, ExportError, Formatter, Kind, Mark, NodeInfo, ReferenceScope, RenderEvent,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub(crate) struct TypstResources<'a> {
    pub(crate) page_id: &'a str,
    pub(crate) links: &'a HashMap<String, Destination>,
    pub(crate) assets: &'a HashMap<String, String>,
    pub(crate) fields: &'a HashMap<String, String>,
    pub(crate) snippets: &'a HashMap<String, Value>,
    pub(crate) excluded_block_ids: &'a HashSet<String>,
    /// The `block-…` labels some link points at. Every other block goes
    /// without a label: each one is an element Typst tracks through layout.
    pub(crate) anchors: &'a HashSet<String>,
    /// The heading above the list of sources the document cites.
    pub(crate) references_heading: &'a str,
}

/// The numbering of one chapter's or one book's footnotes and citations.
#[derive(Default)]
pub(crate) struct TypstState {
    scope: ReferenceScope,
}

impl TypstState {
    /// A scope whose list anchors carry this name, unique within the output.
    pub(crate) fn named(name: &str) -> Self {
        Self {
            scope: ReferenceScope::named(name),
        }
    }
}

/// One document with its sources listed at the end under a level-two heading.
pub(crate) fn render_typst_document(
    document: &Value,
    resources: &TypstResources<'_>,
    offset: i16,
) -> Result<String, String> {
    let mut state = TypstState::default();
    let mut output = render_typst_document_in_scope(document, resources, offset, &mut state)?;
    output.push_str(&render_typst_references(
        &mut state,
        resources.page_id,
        Some((resources.references_heading, 2)),
        resources.anchors,
    )?);
    Ok(output)
}

/// The sources a scope has gathered, ready to follow the text that cited
/// them; empty when nothing was cited.
pub(crate) fn render_typst_references(
    state: &mut TypstState,
    page_id: &str,
    heading: Option<(&str, u16)>,
    anchors: &HashSet<String>,
) -> Result<String, String> {
    let Some(list) = state.scope.take_list(page_id, heading) else {
        return Ok(String::new());
    };
    render::render(
        &list,
        TypstFormatter {
            frames: Vec::new(),
            output: String::new(),
            output_reference_end: None,
            anchors,
        },
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn render_typst_document_in_scope(
    document: &Value,
    resources: &TypstResources<'_>,
    offset: i16,
    state: &mut TypstState,
) -> Result<String, String> {
    let page = render::prepare_page(
        resources.page_id,
        document,
        &render::Resources {
            links: resources.links,
            assets: resources.assets,
            fields: resources.fields,
            snippets: resources.snippets,
            excluded_block_ids: resources.excluded_block_ids,
        },
        offset,
        &mut state.scope,
    )
    .map_err(|error| error.to_string())?;
    render::render(
        &page,
        TypstFormatter {
            frames: Vec::new(),
            output: String::new(),
            output_reference_end: None,
            anchors: resources.anchors,
        },
    )
    .map_err(|error| error.to_string())
}

#[derive(Default)]
struct Frame {
    body: String,
    raw: bool,
    figure: bool,
    figure_kind: Option<&'static str>,
    row_anchor: Option<String>,
    reference: bool,
    /// Where the body ended after its last footnote; another footnote added
    /// there follows it directly and needs a separator.
    reference_end: Option<usize>,
}
struct TypstFormatter<'a> {
    frames: Vec<Frame>,
    output: String,
    output_reference_end: Option<usize>,
    anchors: &'a HashSet<String>,
}

/// Typst accepts braced Unicode escapes, unlike JSON string literals.
#[expect(
    clippy::expect_used,
    reason = "Formatting into an owned String cannot fail."
)]
pub(crate) fn quoted(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control.is_control() => {
                write!(output, "\\u{{{:x}}}", u32::from(control)).expect("String formatting");
            }
            other => output.push(other),
        }
    }
    output.push('"');
    output
}
fn anchor(id: Option<&str>) -> String {
    id.map_or_else(String::new, |id| {
        format!("#publication-anchor({})", quoted(&format!("block-{id}")))
    })
}
fn anchor_arg(id: Option<&str>) -> String {
    id.map_or_else(String::new, |id| {
        format!(", anchor: {}", quoted(&format!("block-{id}")))
    })
}

#[derive(Clone, Copy, Default)]
pub(crate) struct HeadingOptions<'a> {
    pub(crate) anchor: Option<&'a str>,
    pub(crate) chapter: bool,
    pub(crate) exclude_from_outline: bool,
}

/// Frame a native heading explicitly; its body is already serialized Typst.
pub(crate) fn heading(level: usize, body: &str, options: HeadingOptions<'_>) -> String {
    let mut arguments = format!("level: {level}");
    if options.chapter {
        arguments.push_str(", supplement: [Chapter]");
    }
    if options.exclude_from_outline {
        arguments.push_str(", outlined: false");
    }
    let label = options
        .anchor
        .map_or_else(String::new, |id| format!("#label({})", quoted(id)));
    format!(
        "#publication-block(\"heading{}\")[#heading({arguments})[{body}]{label}]\n\n",
        level.min(6)
    )
}

impl TypstFormatter<'_> {
    /// A block's ID, if some link points at it.
    fn targeted<'b>(&self, id: Option<&'b str>) -> Option<&'b str> {
        id.filter(|id| self.anchors.contains(&format!("block-{id}")))
    }

    /// Adds output to the open frame. Footnotes written one after another get
    /// a raised comma between them, so their markers do not read as one number.
    fn append(&mut self, body: &str, reference: bool) {
        let (text, reference_end) = match self.frames.last_mut() {
            Some(frame) => (&mut frame.body, &mut frame.reference_end),
            None => (&mut self.output, &mut self.output_reference_end),
        };
        if reference && *reference_end == Some(text.len()) {
            text.push_str("#super[,]");
        }
        text.push_str(body);
        if reference {
            *reference_end = Some(text.len());
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Exhaustive serialization of typed events; no source traversal occurs here."
    )]
    #[expect(
        clippy::unwrap_used,
        reason = "Formatting only strings and primitive values into String is infallible."
    )]
    fn node(&self, node: &NodeInfo, frame: &Frame) -> String {
        let body = &frame.body;
        let id = self.targeted(node.id.as_deref());
        let argument = anchor_arg(id);
        match &node.kind {
            Kind::Text(text) => {
                if frame.raw {
                    quoted(text)
                } else {
                    format!("#text({})", quoted(text))
                }
            }
            Kind::HardBreak => "#linebreak()".into(),
            Kind::Paragraph if frame.reference => format!("{}{body}", anchor(id)),
            Kind::Paragraph => format!(
                "#publication-par({})[{body}]\n\n",
                argument.trim_start_matches(", ")
            ),
            Kind::Heading { level } => heading(
                usize::from(*level),
                body,
                HeadingOptions {
                    anchor: id.map(|id| format!("block-{id}")).as_deref(),
                    ..HeadingOptions::default()
                },
            ),
            Kind::Quote => format!("#publication-block(\"quote\"{argument})[{body}]\n\n"),
            Kind::QuoteAttribution => format!(
                "#publication-block(\"attribution\"{argument})[#text({}){body}]\n",
                quoted("\u{2014}\u{a0}")
            ),
            Kind::Box(kind) => format!(
                "#publication-block({}{argument})[{body}]\n\n",
                quoted(kind.role().name())
            ),
            Kind::BoxHeading => format!("#publication-block(\"boxHeading\"{argument})[{body}]\n"),
            Kind::Embed { .. } => format!("#publication-block(\"embed\"{argument})[{body}]\n\n"),
            Kind::List { ordered, start } => format!(
                "#publication-block(\"list\"{argument})[#{}({}{body})]\n\n",
                if *ordered { "enum" } else { "list" },
                if *ordered {
                    format!("start: {start},\n")
                } else {
                    "\n".into()
                }
            ),
            Kind::ListItem => format!("[{}{body}],\n", anchor(id)),
            Kind::Table { widths_percent, .. } => {
                let tracks = widths_percent
                    .iter()
                    .map(|width| format!("{width}fr"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "#publication-block(\"table\"{argument})[#table(columns: ({tracks},),\n{body})]\n\n"
                )
            }
            Kind::TableRow {
                repeat_header: true,
                ..
            } => format!("table.header(\n{body}),\n"),
            Kind::Document | Kind::TableRow { .. } => body.clone(),
            Kind::TableCell(cell) => format!(
                "publication-cell(column: {}, row: {}, colspan: {}, rowspan: {}, header: {})[{}{}{body}],\n",
                cell.column,
                cell.row,
                cell.colspan,
                cell.rowspan,
                cell.header,
                anchor(frame.row_anchor.as_deref()),
                anchor(id)
            ),
            Kind::Image(image) => {
                let mut image_source = format!(
                    "#publication-image({},{},",
                    quoted(&image.asset),
                    quoted(&image.alt)
                );
                if let Some(width) = image.width_pt {
                    write!(image_source, " width: {width}pt,").unwrap();
                }
                if let Some(height) = image.height_pt {
                    write!(image_source, " height: {height}pt,").unwrap();
                }
                if let Some(id) = id {
                    write!(image_source, " anchor: {},", quoted(&format!("block-{id}"))).unwrap();
                }
                image_source.push_str(")\n\n");
                image_source
            }
            Kind::CodeBlock { text, language } => format!(
                "#publication-code({}, language: {}{argument})\n\n",
                quoted(text),
                quoted(language)
            ),
            Kind::Rule => format!(
                "#publication-rule({})\n\n",
                argument.trim_start_matches(", ")
            ),
            Kind::Figure { caption } => format!(
                "#publication-figure(kind: {}, caption: {}{argument})[{body}]\n\n",
                frame.figure_kind.unwrap_or("image"),
                if caption.trim().is_empty() {
                    "none".into()
                } else {
                    format!("[#text({})]", quoted(caption))
                }
            ),
            Kind::Footnote(_) => format!("#footnote[{body}]"),
            Kind::Citations(citations) => {
                let mut entries = String::from("\\[");
                for (index, citation) in citations.iter().enumerate() {
                    if index > 0 {
                        entries.push_str(", ");
                    }
                    write!(
                        entries,
                        "#link(label({}))[{}]",
                        quoted(&citation.target),
                        citation.number,
                    )
                    .unwrap();
                }
                entries.push_str("\\]");
                entries
            }
            Kind::ReferenceList => format!("#publication-references[{body}]\n\n"),
            Kind::ReferenceItem(citation) => format!(
                "#publication-reference-item(\"{}\", {})[{body}]\n\n",
                citation.number,
                quoted(&citation.target)
            ),
        }
    }
}

impl Formatter for TypstFormatter<'_> {
    type Output = String;
    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
        match event {
            RenderEvent::Enter(node) => {
                if let Some(frame) = self.frames.last_mut().filter(|frame| frame.figure) {
                    frame.figure_kind = match node.kind {
                        Kind::Table { .. } => Some("table"),
                        Kind::CodeBlock { .. } => Some("raw"),
                        Kind::Image(_) => Some("image"),
                        _ => None,
                    };
                }
                let raw = self.frames.last().is_some_and(|frame| frame.raw);
                let reference = matches!(node.kind, Kind::Footnote(_))
                    || self.frames.last().is_some_and(|frame| frame.reference);
                let row_anchor = if matches!(node.kind, Kind::TableCell(_)) {
                    self.frames
                        .last_mut()
                        .and_then(|frame| frame.row_anchor.take())
                } else {
                    None
                };
                let own_row = if matches!(node.kind, Kind::TableRow { .. }) {
                    self.targeted(node.id.as_deref()).map(str::to_owned)
                } else {
                    row_anchor
                };
                self.frames.push(Frame {
                    raw,
                    reference,
                    figure: matches!(node.kind, Kind::Figure { .. }),
                    row_anchor: own_row,
                    ..Frame::default()
                });
            }
            RenderEvent::Exit(node) => {
                let frame = self
                    .frames
                    .pop()
                    .ok_or_else(|| ExportError::new("Unbalanced Typst node event"))?;
                let value = self.node(node, &frame);
                self.append(&value, matches!(node.kind, Kind::Footnote(_)));
            }
            RenderEvent::EnterMark(mark) => self.frames.push(Frame {
                raw: matches!(mark, Mark::Code),
                ..Frame::default()
            }),
            RenderEvent::ExitMark(mark) => {
                let frame = self
                    .frames
                    .pop()
                    .ok_or_else(|| ExportError::new("Unbalanced Typst mark event"))?;
                let body = frame.body;
                let value = match mark {
                    Mark::Strong => format!("#strong[{body}]"),
                    Mark::Emphasis => format!("#emph[{body}]"),
                    Mark::Underline => format!("#underline[{body}]"),
                    Mark::Strike => format!("#strike[{body}]"),
                    Mark::Superscript => format!("#super[{body}]"),
                    Mark::Subscript => format!("#sub[{body}]"),
                    Mark::Code => format!("#raw({body})"),
                    Mark::Link(destination) => {
                        let target = match destination {
                            Destination::Url(url) => quoted(url),
                            Destination::Anchor(anchor) => format!("label({})", quoted(anchor)),
                            Destination::PlainText => unreachable!(),
                        };
                        format!("#link({target})[{body}]")
                    }
                };
                self.append(&value, false);
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<String, ExportError> {
        if self.frames.is_empty() {
            Ok(self.output)
        } else {
            Err(ExportError::new("Unclosed Typst events"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn code_remains_literal_and_titles_never_enter_the_code_payload() {
        let document = json!({"type":"doc","content":[{"type":"figure","attrs":{"id":"figure"},"content":[{"type":"blockTitle","content":[{"type":"text","text":"Caption # <title>"}]},{"type":"code_block","content":[{"type":"text","text":"fn main() {\n  println!(\"#literal\");\n}"}]}]}]});
        let empty = HashMap::new();
        let snippets = HashMap::new();
        let exclusions = HashSet::new();
        let render = |anchors: &HashSet<String>| {
            render_typst_document(
                &document,
                &TypstResources {
                    page_id: "page",
                    references_heading: "References",
                    links: &HashMap::new(),
                    assets: &empty,
                    fields: &empty,
                    snippets: &snippets,
                    excluded_block_ids: &exclusions,
                    anchors,
                },
                0,
            )
            .unwrap()
        };
        // A block keeps its label only while something links to it.
        assert!(!render(&HashSet::new()).contains("anchor:"));
        let output = render(&HashSet::from(["block-figure".to_owned()]));
        assert!(
            output
                .contains("#publication-code(\"fn main() {\\n  println!(\\\"#literal\\\");\\n}\"")
        );
        assert!(output.contains("caption: [#text(\"Caption # <title>\")]"));
        assert!(!output.contains("#publication-code(\"Caption"));
        assert!(output.contains("anchor: \"block-figure\""));
    }

    #[test]
    fn adjacent_footnotes_are_separated_so_their_numbers_stay_apart() {
        let cite = |text: &str| {
            json!({"type":"footnote","attrs":{"paragraph":
                {"type":"paragraph","content":[{"type":"text","text":text}]}}})
        };
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Claim."}, cite("First"), cite("Second"),
            {"type":"text","text":" More."}, cite("Third")
        ]}]});
        let empty = HashMap::new();
        let output = render_typst_document(
            &document,
            &TypstResources {
                page_id: "page",
                references_heading: "References",
                links: &HashMap::new(),
                assets: &empty,
                fields: &empty,
                snippets: &HashMap::new(),
                excluded_block_ids: &HashSet::new(),
                anchors: &HashSet::new(),
            },
            0,
        )
        .unwrap();
        assert_eq!(output.matches("#super[,]").count(), 1, "{output}");
        assert!(output.contains("#footnote[#text(\"First\")]#super[,]#footnote["));
        assert!(output.contains("#text(\" More.\")#footnote["));
    }
}

#[cfg(test)]
mod escaping_tests {
    #[test]
    fn controls_use_typst_unicode_escapes_and_markup_stays_literal() {
        assert_eq!(
            super::quoted("\"\\\n\r\t\u{7}\u{8}\u{c}#[] café"),
            "\"\\\"\\\\\\n\\r\\t\\u{7}\\u{8}\\u{c}#[] café\""
        );
    }
}
