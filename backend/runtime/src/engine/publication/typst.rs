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
}

#[derive(Default)]
pub(crate) struct TypstState {
    scope: ReferenceScope,
}

pub(crate) fn render_typst_document(
    document: &Value,
    resources: &TypstResources<'_>,
    offset: i16,
) -> Result<String, String> {
    render_typst_document_in_scope(document, resources, offset, &mut TypstState::default())
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
    render::render(&page, TypstFormatter::default()).map_err(|error| error.to_string())
}

#[derive(Default)]
struct Frame {
    body: String,
    raw: bool,
    figure: bool,
    figure_kind: Option<&'static str>,
    row_anchor: Option<String>,
    reference: bool,
}
#[derive(Default)]
struct TypstFormatter {
    frames: Vec<Frame>,
    output: String,
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

impl TypstFormatter {
    fn append(&mut self, body: &str) {
        if let Some(frame) = self.frames.last_mut() {
            frame.body.push_str(body);
        } else {
            self.output.push_str(body);
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
    fn node(node: &NodeInfo, frame: &Frame) -> String {
        let body = &frame.body;
        let id = node.id.as_deref();
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
            Kind::Heading { level } => format!(
                "#heading(level: {level})[{body}]{}\n\n",
                id.map_or_else(String::new, |id| format!(
                    "#label({})",
                    quoted(&format!("block-{id}"))
                ))
            ),
            Kind::Quote => format!("#publication-block(\"quote\"{argument})[{body}]\n\n"),
            Kind::Box(kind) => {
                let name = kind.name();
                let role = format!("box{}{}", name[..1].to_ascii_uppercase(), &name[1..]);
                format!(
                    "#publication-block({}{argument})[{body}]\n\n",
                    quoted(&role)
                )
            }
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
            Kind::Reference(reference) => {
                if reference.first {
                    format!(
                        "#footnote[{body}]#label({})",
                        quoted(&format!("reference-{}", reference.number))
                    )
                } else {
                    format!(
                        "#footnote(label({}))",
                        quoted(&format!("reference-{}", reference.number))
                    )
                }
            }
        }
    }
}

impl Formatter for TypstFormatter {
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
                let reference = matches!(node.kind, Kind::Reference(_))
                    || self.frames.last().is_some_and(|frame| frame.reference);
                let row_anchor = if matches!(node.kind, Kind::TableCell(_)) {
                    self.frames
                        .last_mut()
                        .and_then(|frame| frame.row_anchor.take())
                } else {
                    None
                };
                let own_row = if matches!(node.kind, Kind::TableRow { .. }) {
                    node.id.clone()
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
                self.append(&Self::node(node, &frame));
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
                self.append(&value);
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
        let output = render_typst_document(
            &document,
            &TypstResources {
                page_id: "page",
                links: &HashMap::new(),
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap();
        assert!(
            output
                .contains("#publication-code(\"fn main() {\\n  println!(\\\"#literal\\\");\\n}\"")
        );
        assert!(output.contains("caption: [#text(\"Caption # <title>\")]"));
        assert!(!output.contains("#publication-code(\"Caption"));
        assert!(output.contains("anchor: \"block-figure\""));
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
