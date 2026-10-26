// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::render::{
    self, Destination, ExportError, Formatter, Kind, Mark, NodeInfo, PreparedPage, ReferenceScope,
    RenderEvent,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

pub(crate) struct HtmlResources<'a> {
    pub(crate) page_id: &'a str,
    pub(crate) links: &'a HashMap<String, String>,
    pub(crate) assets: &'a HashMap<String, String>,
    pub(crate) fields: &'a HashMap<String, String>,
    pub(crate) snippets: &'a HashMap<String, Value>,
    pub(crate) excluded_block_ids: &'a HashSet<String>,
}

#[derive(Default)]
pub(crate) struct HtmlState {
    scope: ReferenceScope,
    notes: BTreeMap<usize, String>,
    occurrences: BTreeMap<usize, usize>,
    xhtml: bool,
}

/// EPUB uses the same semantic renderer, with XML-compatible void elements
/// and ebook reference semantics. Each chapter owns its reference scope.
pub(crate) fn render_xhtml_document(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
) -> Result<String, String> {
    let mut state = HtmlState {
        xhtml: true,
        ..HtmlState::default()
    };
    let mut body = render_html_document_in_scope(document, resources, heading_offset, &mut state)?;
    body.push_str(&html_endnotes(&state));
    crate::engine::xml::valid_characters(&body)?;
    Ok(body)
}

pub(crate) fn render_html_document(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
) -> Result<String, String> {
    let mut state = HtmlState::default();
    let mut html = render_html_document_in_scope(document, resources, heading_offset, &mut state)?;
    html.push_str(&html_endnotes(&state));
    Ok(html)
}

pub(crate) fn render_html_document_in_scope(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
) -> Result<String, String> {
    let prepared = prepare_html_document(document, resources, heading_offset, &mut state.scope)?;
    render_prepared_in_scope(&prepared, state)
}

pub(crate) fn prepare_html_document(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    scope: &mut ReferenceScope,
) -> Result<PreparedPage, String> {
    let links = resources
        .links
        .iter()
        .map(|(source, target)| {
            (
                source.clone(),
                if target.is_empty() {
                    Destination::PlainText
                } else {
                    Destination::Url(target.clone())
                },
            )
        })
        .collect();
    render::prepare_page(
        resources.page_id,
        document,
        &render::Resources {
            links: &links,
            assets: resources.assets,
            fields: resources.fields,
            snippets: resources.snippets,
            excluded_block_ids: resources.excluded_block_ids,
        },
        heading_offset,
        scope,
    )
    .map_err(|error| error.to_string())
}

/// Reuse the exact prepared page for HTML, search and on-page navigation.
pub(crate) fn render_prepared_html_document(page: &PreparedPage) -> Result<String, String> {
    let mut state = HtmlState::default();
    let mut html = render_prepared_in_scope(page, &mut state)?;
    html.push_str(&html_endnotes(&state));
    Ok(html)
}

fn render_prepared_in_scope(page: &PreparedPage, state: &mut HtmlState) -> Result<String, String> {
    render::render(
        page,
        HtmlFormatter {
            state,
            frames: Vec::new(),
            output: String::new(),
        },
    )
    .map_err(|error| error.to_string())
}

#[expect(
    clippy::unwrap_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(crate) fn html_endnotes(state: &HtmlState) -> String {
    let mut html = String::new();
    if state.notes.is_empty() {
        return html;
    }
    html.push_str(
        "<section class=\"footnotes\" aria-label=\"Footnotes\" role=\"doc-endnotes\"><ol>\n",
    );
    for (number, note) in &state.notes {
        let tag = if state.xhtml {
            html.push_str("<li>");
            "aside"
        } else {
            "li"
        };
        let semantic = if state.xhtml {
            " epub:type=\"footnote\""
        } else {
            ""
        };
        write!(
            html,
            "<{tag} id=\"fn-{number}\" role=\"doc-footnote\"{semantic}>{note}"
        )
        .unwrap();
        for occurrence in 1..=state.occurrences[number] {
            write!(html," <a href=\"#fnref-{number}-{occurrence}\" aria-label=\"Back to reference {number}, occurrence {occurrence}\">↩</a>").unwrap();
        }
        if state.xhtml {
            html.push_str("</aside>");
        }
        html.push_str("</li>\n");
    }
    html.push_str("</ol></section>\n");
    html
}

struct Frame {
    body: String,
    title: Option<String>,
    caption: Option<String>,
}
struct HtmlFormatter<'a> {
    state: &'a mut HtmlState,
    frames: Vec<Frame>,
    output: String,
}

impl HtmlFormatter<'_> {
    fn append(&mut self, value: &str) {
        if let Some(frame) = self.frames.last_mut() {
            frame.body.push_str(value);
        } else {
            self.output.push_str(value);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Exhaustive serialization of typed node events without source traversal."
    )]
    #[expect(
        clippy::unwrap_used,
        reason = "Formatting only strings and primitive values into String is infallible."
    )]
    fn node(&mut self, node: &NodeInfo, body: &str, title: Option<&str>) -> String {
        let mut html = String::new();
        let close = if self.state.xhtml { " />" } else { ">" };
        let id = node
            .id
            .as_deref()
            .map(|id| format!(" id=\"block-{}\"", escape_html(id)))
            .unwrap_or_default();
        match &node.kind {
            Kind::Document => html.push_str(body),
            Kind::Text(text) => html.push_str(&escape_html(text)),
            Kind::HardBreak => write!(html, "<br{close}").unwrap(),
            Kind::Rule => writeln!(html, "<hr{id}{close}").unwrap(),
            Kind::Figure { .. } => {
                writeln!(html, "<div class=\"titled-block\"{id}>{body}</div>").unwrap();
            }
            Kind::Embed { .. } => write!(
                html,
                "<section class=\"embedded-passage\"{id}>\n{body}</section>\n"
            )
            .unwrap(),
            Kind::Image(image) => {
                if title.is_some() {
                    html.push_str("<figure>");
                }
                write!(
                    html,
                    "<img{id} src=\"{}\" alt=\"{}\"",
                    escape_html(&image.asset),
                    escape_html(&image.alt)
                )
                .unwrap();
                for (name, size) in [("width", image.width_pt), ("height", image.height_pt)] {
                    if let Some(size) = size {
                        write!(html, " {name}=\"{:.0}\"", size / 0.75).unwrap();
                    }
                }
                html.push_str(close);
                html.push('\n');
                if let Some(title) = title {
                    writeln!(
                        html,
                        "<figcaption>{}</figcaption></figure>",
                        escape_html(title)
                    )
                    .unwrap();
                }
            }
            Kind::CodeBlock { text, language } => {
                if title.is_some() {
                    html.push_str("<figure class=\"code-block\">");
                }
                write!(html, "<pre{id}").unwrap();
                if language != "plain" {
                    write!(html, " data-language=\"{}\"", escape_html(language)).unwrap();
                }
                writeln!(html, "><code>{}</code></pre>", escape_html(text)).unwrap();
                if let Some(title) = title {
                    writeln!(
                        html,
                        "<figcaption>{}</figcaption></figure>",
                        escape_html(title)
                    )
                    .unwrap();
                }
            }
            Kind::Reference(reference) => {
                if reference.first {
                    self.state.notes.insert(reference.number, body.to_owned());
                }
                self.state
                    .occurrences
                    .insert(reference.number, reference.occurrence);
                let semantic = if self.state.xhtml {
                    " epub:type=\"noteref\""
                } else {
                    ""
                };
                write!(html,"<sup><a id=\"fnref-{}-{}\" href=\"#fn-{}\" role=\"doc-noteref\"{semantic}>{}</a></sup>",reference.number,reference.occurrence,reference.number,reference.number).unwrap();
            }
            kind => {
                let (tag, extra) = match kind {
                    Kind::Paragraph => ("p".to_owned(), String::new()),
                    Kind::Heading { level } => (format!("h{}", level.min(&6)), String::new()),
                    Kind::Quote => ("blockquote".into(), String::new()),
                    Kind::Box(kind) => (
                        "aside".into(),
                        format!(" class=\"callout\" data-kind=\"{}\"", kind.name()),
                    ),
                    Kind::BoxHeading => ("p".into(), " class=\"callout-title\"".into()),
                    Kind::List {
                        ordered: true,
                        start,
                    } => ("ol".into(), format!(" start=\"{start}\"")),
                    Kind::List { .. } => ("ul".into(), String::new()),
                    Kind::ListItem => ("li".into(), String::new()),
                    Kind::Table { .. } => ("table".into(), String::new()),
                    Kind::TableRow { .. } => ("tr".into(), String::new()),
                    Kind::TableCell(cell) => {
                        let mut extra = if cell.header {
                            " scope=\"col\"".to_owned()
                        } else {
                            String::new()
                        };
                        for (name, value) in [("colspan", cell.colspan), ("rowspan", cell.rowspan)]
                        {
                            if value > 1 {
                                write!(extra, " {name}=\"{value}\"").unwrap();
                            }
                        }
                        (if cell.header { "th" } else { "td" }.into(), extra)
                    }
                    _ => unreachable!("Leaf cases handled above"),
                };
                write!(html, "<{tag}{id}{extra}>").unwrap();
                if let Kind::Table { widths_percent, .. } = kind {
                    if let Some(title) = title {
                        write!(html, "<caption>{}</caption>", escape_html(title)).unwrap();
                    }
                    html.push_str("<colgroup>");
                    for width in widths_percent {
                        write!(html, "<col style=\"width: {width}%\"{close}").unwrap();
                    }
                    html.push_str("</colgroup><tbody>");
                }
                html.push_str(body);
                if matches!(kind, Kind::Table { .. }) {
                    html.push_str("</tbody>");
                }
                write!(html, "</{tag}>").unwrap();
                if !matches!(kind, Kind::TableCell(_)) {
                    html.push('\n');
                }
            }
        }
        html
    }
}

impl Formatter for HtmlFormatter<'_> {
    type Output = String;
    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
        match event {
            RenderEvent::Enter(node) => {
                let title = self.frames.last().and_then(|frame| frame.caption.clone());
                let caption = if let Kind::Figure { caption } = &node.kind {
                    Some(caption.clone()).filter(|value| !value.trim().is_empty())
                } else {
                    None
                };
                self.frames.push(Frame {
                    body: String::new(),
                    title,
                    caption,
                });
            }
            RenderEvent::EnterMark(_) => self.frames.push(Frame {
                body: String::new(),
                title: None,
                caption: None,
            }),
            RenderEvent::Exit(node) => {
                let frame = self
                    .frames
                    .pop()
                    .ok_or_else(|| ExportError::new("Unbalanced HTML node event"))?;
                let value = self.node(node, &frame.body, frame.title.as_deref());
                self.append(&value);
            }
            RenderEvent::ExitMark(mark) => {
                let frame = self
                    .frames
                    .pop()
                    .ok_or_else(|| ExportError::new("Unbalanced HTML mark event"))?;
                let (tag, extra) = match mark {
                    Mark::Strong => ("strong", String::new()),
                    Mark::Emphasis => ("em", String::new()),
                    Mark::Underline => ("u", String::new()),
                    Mark::Strike => ("s", String::new()),
                    Mark::Code => ("code", String::new()),
                    Mark::Superscript => ("sup", String::new()),
                    Mark::Subscript => ("sub", String::new()),
                    Mark::Link(destination) => {
                        let target = match destination {
                            Destination::Url(url) => url.clone(),
                            Destination::Anchor(anchor) => format!("#{anchor}"),
                            Destination::PlainText => unreachable!(),
                        };
                        (
                            "a",
                            format!(
                                " href=\"{}\" rel=\"noopener noreferrer\"",
                                escape_html(&target)
                            ),
                        )
                    }
                };
                self.append(&format!("<{tag}{extra}>{}</{tag}>", frame.body));
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<String, ExportError> {
        if self.frames.is_empty() {
            Ok(self.output)
        } else {
            Err(ExportError::new("Unclosed HTML events"))
        }
    }
}

#[must_use]
pub(crate) fn escape_html(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            value => output.push(value),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{HtmlResources, render_html_document};
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    #[test]
    fn renders_nested_blocks_and_escapes_authored_values() {
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"level":1,"id":"01a0e2a0-0000-7000-8000-000000000001"},"content":[{"type":"text","text":"Heading <one>"}]},
            {"type":"paragraph","attrs":{"id":"01a0e2a0-0000-7000-8000-000000000002"},"content":[
                {"type":"text","text":"Link & ","marks":[{"type":"link","attrs":{"href":"target"}}]},
                {"type":"text","text":"bold","marks":[{"type":"strong"}]}
            ]},
            {"type":"table","attrs":{"columnWidthsPercent":[35,65]},"content":[{"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"A"}]}]},{"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"B"}]}]}]}]},
            {"type":"image","attrs":{"src":"asset","alt":"Diagram <sky>"}}
        ]});
        let links = HashMap::from([("target".to_owned(), "chapter.html#part".to_owned())]);
        let assets = HashMap::from([("asset".to_owned(), "assets/image.png".to_owned())]);
        let fields = HashMap::new();
        let snippets = HashMap::new();
        let excluded_block_ids = HashSet::from(["01a0e2a0-0000-7000-8000-000000000002".to_owned()]);
        let html = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &excluded_block_ids,
            },
            1,
        )
        .unwrap();
        assert!(html.contains(
            "<h2 id=\"block-01a0e2a0-0000-7000-8000-000000000001\">Heading &lt;one&gt;</h2>"
        ));
        assert!(!html.contains("Link &amp;"));
        assert!(html.contains("<colgroup><col style=\"width: 35%\"><col style=\"width: 65%\"></colgroup><tbody><tr><th scope=\"col\"><p>A</p>"));
        assert!(html.contains("alt=\"Diagram &lt;sky&gt;\""));
    }

    #[test]
    fn unresolved_dynamic_content_stops_rendering() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"field","attrs":{"key":"missing"}}]}]});
        let empty = HashMap::new();
        let snippets = HashMap::new();
        let exclusions = HashSet::new();
        let error = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap_err();
        assert!(error.contains("not resolved"));
    }

    #[test]
    fn resolved_links_are_escaped_and_unsafe_destinations_fail() {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"A & B","marks":[{"type":"link","attrs":{"href":"target"}}]}
        ]}]});
        let assets = HashMap::new();
        let fields = HashMap::new();
        let snippets = HashMap::new();
        let exclusions = HashSet::new();
        let good = HashMap::from([("target".to_owned(), "chapter.html?x=1&y=2".to_owned())]);
        let html = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &good,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap();
        assert!(html.contains("href=\"chapter.html?x=1&amp;y=2\""));
        assert!(html.contains(">A &amp; B</a>"));
        let bad = HashMap::from([("target".to_owned(), "javascript:alert(1)".to_owned())]);
        assert!(
            render_html_document(
                &document,
                &HtmlResources {
                    page_id: "test-page",
                    links: &bad,
                    assets: &assets,
                    fields: &fields,
                    snippets: &snippets,
                    excluded_block_ids: &exclusions,
                },
                0
            )
            .unwrap_err()
            .contains("Unsafe")
        );
    }

    #[test]
    fn renders_inline_footnotes_linked_citations_and_snippet_embeds() {
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[
                {"type":"text","text":"Text"},
                {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"A <footnote>"}]}}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}}
            ]},
            {"type":"embed","attrs":{"snippetId":"source_one"}}
        ]});
        let empty = HashMap::new();
        let snippets = HashMap::from([(
            "source_one".to_owned(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Passage & source"}]}),
        )]);
        let exclusions = HashSet::new();
        let html = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap();
        assert!(html.contains("id=\"fnref-1-1\""));
        assert!(html.contains("id=\"fnref-2-1\""));
        assert!(html.contains("id=\"fnref-2-2\""));
        assert!(html.contains("A &lt;footnote&gt;"));
        assert!(html.contains("<section class=\"embedded-passage\">"));
        assert!(html.contains("Passage &amp; source"));
        assert_eq!(html.matches("id=\"fn-2\"").count(), 1);
    }

    #[test]
    fn rejects_unresolved_snippets_and_nonterminal_paragraphs() {
        let document =
            json!({"type":"doc","content":[{"type":"embed","attrs":{"snippetId":"loop"}}]});
        let empty = HashMap::new();
        let empty_passages = HashMap::new();
        let exclusions = HashSet::new();
        let missing = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &empty_passages,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap_err();
        assert!(missing.contains("not resolved"));
        let snippets = HashMap::from([(
            "loop".to_owned(),
            json!({"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"loop"}}]}),
        )]);
        let cyclic = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap_err();
        assert!(cyclic.contains("text only"));
    }

    #[test]
    fn snippet_uses_the_planned_external_link_resolution() {
        let document =
            json!({"type":"doc","content":[{"type":"embed","attrs":{"snippetId":"source"}}]});
        let host_links = HashMap::from([(
            "https://private.example/source".to_owned(),
            "https://private.example/source".to_owned(),
        )]);
        let empty = HashMap::new();
        let exclusions = HashSet::new();
        let snippets = HashMap::from([(
            "source".to_owned(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Source link","marks":[{"type":"link","attrs":{"href":"https://private.example/source"}}]}]}),
        )]);
        let html = render_html_document(
            &document,
            &HtmlResources {
                page_id: "test-page",
                links: &host_links,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            0,
        )
        .unwrap();
        assert!(html.contains("href=\"https://private.example/source\""));
    }
}
