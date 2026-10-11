// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use minijinja::context;
use minijinja::value::Value as TemplateValue;
use serde_json::Value;
use url::Url;

use crate::engine::document::validation::validate_document;
use crate::engine::publication::macros::RenderMacros;

/// All destinations and dynamic values come from a completed source plan.
/// Missing entries are errors; the walker never guesses a destination.
pub(crate) struct HtmlResources<'a> {
    pub(crate) links: &'a HashMap<String, String>,
    pub(crate) assets: &'a HashMap<String, String>,
    pub(crate) fields: &'a HashMap<String, String>,
    pub(crate) snippets: &'a HashMap<String, Value>,
    pub(crate) excluded_block_ids: &'a HashSet<String>,
    pub(crate) macros: Option<&'a RenderMacros>,
}

#[derive(Default)]
pub(crate) struct HtmlState {
    footnotes: Vec<String>,
    citation_numbers: HashMap<String, usize>,
    reference_counts: HashMap<usize, usize>,
    anchor_prefix: String,
}

pub(crate) fn render_html_document(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
) -> Result<String, String> {
    render_html_document_with_prefix(document, resources, heading_offset, "")
}

pub(crate) fn render_html_document_with_prefix(
    document: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    anchor_prefix: &str,
) -> Result<String, String> {
    let mut state = HtmlState {
        anchor_prefix: anchor_prefix.to_owned(),
        ..Default::default()
    };
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
    validate_document(document).map_err(|error| error.to_string())?;
    for (id, paragraph) in resources.snippets {
        crate::engine::project::snippets::validate_reference_paragraph(paragraph, true)
            .map_err(|error| format!("Snippet {id}: {error}"))?;
    }
    let mut html = String::new();
    render_node(document, resources, heading_offset, state, &mut html)?;
    Ok(html)
}

pub(crate) fn html_endnotes(state: &HtmlState) -> String {
    let mut html = String::new();
    if !state.footnotes.is_empty() {
        html.push_str(
            "<section class=\"footnotes\" aria-label=\"Footnotes\" role=\"doc-endnotes\"><ol>\n",
        );
        for (index, note) in state.footnotes.iter().enumerate() {
            let number = index + 1;
            write!(
                html,
                "<li id=\"{}fn-{number}\" role=\"doc-footnote\">",
                state.anchor_prefix
            )
            .expect("writing to a String cannot fail");
            html.push_str(note);
            writeln!(html, " <a href=\"#{}fnref-{number}-1\" aria-label=\"Back to reference {number}\">↩</a></li>", state.anchor_prefix).expect("writing to a String cannot fail");
        }
        html.push_str("</ol></section>\n");
    }
    html
}

fn render_node(
    node: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    output: &mut String,
) -> Result<(), String> {
    render_node_inner(node, resources, heading_offset, state, output, None).map_err(|error| {
        let kind = node["type"].as_str().unwrap_or("unknown");
        let id = node["attrs"]["id"]
            .as_str()
            .map(|value| format!(" {value}"))
            .unwrap_or_default();
        format!("HTML node {kind}{id}: {error}")
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn render_node_inner(
    node: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    output: &mut String,
    title: Option<&str>,
) -> Result<(), String> {
    let kind = node["type"].as_str().ok_or("Document node has no type.")?;
    let attrs = &node["attrs"];
    if attrs["id"]
        .as_str()
        .is_some_and(|id| resources.excluded_block_ids.contains(id))
    {
        return Ok(());
    }
    if kind == "figure" {
        let parts = children(node);
        let title_node = parts
            .first()
            .filter(|child| child["type"] == "blockTitle")
            .ok_or("Figure has no title node.")?;
        let title = children(title_node)
            .iter()
            .map(|child| child["text"].as_str().ok_or("Title child has no text."))
            .collect::<Result<String, _>>()?;
        let content = parts.get(1).ok_or("Figure has no body.")?;
        output.push_str("<div class=\"titled-block\"");
        write_id(attrs, output);
        output.push('>');
        render_node_inner(
            content,
            resources,
            heading_offset,
            state,
            output,
            Some(title.as_str()).filter(|title| !title.trim().is_empty()),
        )?;
        output.push_str("</div>\n");
        return Ok(());
    }
    let mut body = String::new();
    let html = &mut body;
    if let Some(macros) = resources.macros.filter(|_| {
        matches!(
            kind,
            "heading" | "paragraph" | "image" | "list" | "table" | "code_block"
        )
    }) {
        html.push_str(&render_macro_node(
            kind,
            (node, title),
            resources,
            heading_offset,
            state,
            macros,
        )?);
    } else {
        match kind {
            "doc" => render_children(node, resources, heading_offset, state, html)?,
            "text" => escape_into(node["text"].as_str().ok_or("Text node has no text.")?, html),
            "hard_break" => html.push_str("<br>"),
            "horizontal_rule" | "image" | "embed" => {
                render_leaf(kind, attrs, resources, heading_offset, state, html, title)?;
            }
            "field" => {
                let key = attrs["key"].as_str().ok_or("Field has no key.")?;
                let value = resources
                    .fields
                    .get(key)
                    .ok_or_else(|| format!("Field {key} was not resolved by preflight."))?;
                escape_into(value, html);
            }
            "code_block" => {
                let titled = title.is_some();
                if titled {
                    html.push_str("<figure class=\"code-block\">");
                    write_caption(title, "figcaption", html);
                }
                html.push_str("<pre");
                write_id(attrs, html);
                if let Some(language) = attrs["language"].as_str() {
                    html.push_str(" data-language=\"");
                    escape_into(language, html);
                    html.push('"');
                }
                html.push_str("><code>");
                for child in children(node) {
                    escape_into(
                        child["text"].as_str().ok_or("Code child has no text.")?,
                        html,
                    );
                }
                html.push_str("</code></pre>\n");
                if titled {
                    html.push_str("</figure>\n");
                }
            }
            "paragraph" | "heading" | "blockquote" | "box" | "boxHeading" | "list" | "listItem"
            | "table" | "table_row" | "table_cell" | "table_header" => {
                let (tag, extra) = container_tag(kind, attrs, heading_offset)?;
                html.push('<');
                html.push_str(&tag);
                write_id(attrs, html);
                html.push_str(&extra);
                html.push('>');
                if kind == "table" {
                    write_caption(title, "caption", html);
                    write_column_group(attrs, html)?;
                    html.push_str("<tbody>");
                }
                render_children(node, resources, heading_offset, state, html)?;
                if kind == "table" {
                    html.push_str("</tbody>");
                }
                html.push_str("</");
                html.push_str(&tag);
                html.push('>');
                if !matches!(kind, "table_cell" | "table_header") {
                    html.push('\n');
                }
            }
            "footnote" | "citation" => {
                let label = if kind == "citation" && attrs["source"].as_str() == Some("snippet") {
                    Some(
                        attrs["snippetId"]
                            .as_str()
                            .ok_or("Snippet citation has no label key.")?,
                    )
                } else {
                    None
                };
                let number =
                    if let Some(existing) = label.and_then(|key| state.citation_numbers.get(key)) {
                        *existing
                    } else {
                        let number = state.footnotes.len() + 1;
                        state.footnotes.push(String::new());
                        let mut content = String::new();
                        if let Some(key) = label {
                            render_snippet(key, resources, heading_offset, state, &mut content)?;
                            state.citation_numbers.insert(key.to_owned(), number);
                        } else {
                            let fragment = attrs["paragraph"]
                                .as_object()
                                .map(|_| &attrs["paragraph"])
                                .ok_or("Reference has no local paragraph.")?;
                            render_node(fragment, resources, heading_offset, state, &mut content)?;
                        }
                        state.footnotes[number - 1] = content;
                        number
                    };
                let occurrence = state.reference_counts.entry(number).or_insert(0);
                *occurrence += 1;
                write!(html, "<sup><a id=\"{}fnref-{number}-{occurrence}\" href=\"#{}fn-{number}\" role=\"doc-noteref\">{number}</a></sup>", state.anchor_prefix, state.anchor_prefix).expect("writing to a String cannot fail");
            }
            _ => return Err(format!("Unsupported HTML node: {kind}.")),
        }
    }
    if let Some(marks) = node["marks"].as_array() {
        for mark in marks.iter().rev() {
            let name = mark["type"].as_str().ok_or("Mark has no type.")?;
            body = match name {
                "strong" | "superscript" | "subscript" if resources.macros.is_some() => resources
                    .macros
                    .unwrap()
                    .render(name, TemplateValue::from(()), &body)?,
                "link" if resources.macros.is_some() => {
                    let href = mark["attrs"]["href"]
                        .as_str()
                        .ok_or("Link has no destination.")?;
                    let target = resources
                        .links
                        .get(href)
                        .ok_or_else(|| format!("Link {href} was not resolved by preflight."))?;
                    if target.is_empty() {
                        continue;
                    }
                    if !safe_output_url(target, false) {
                        return Err(format!("Unsafe resolved link destination: {target}."));
                    }
                    let mut escaped = String::new();
                    escape_into(target, &mut escaped);
                    resources.macros.unwrap().render(
                        "link",
                        context! {target => TemplateValue::from_safe_string(escaped)},
                        &body,
                    )?
                }
                "link" => {
                    let href = mark["attrs"]["href"]
                        .as_str()
                        .ok_or("Link has no destination.")?;
                    let target = resources
                        .links
                        .get(href)
                        .ok_or_else(|| format!("Link {href} was not resolved by preflight."))?;
                    if target.is_empty() {
                        continue;
                    }
                    if !safe_output_url(target, false) {
                        return Err(format!("Unsafe resolved link destination: {target}."));
                    }
                    let mut escaped = String::new();
                    escape_into(target, &mut escaped);
                    format!("<a href=\"{escaped}\" rel=\"noopener noreferrer\">{body}</a>")
                }
                "strong" => format!("<strong>{body}</strong>"),
                "emphasis" => format!("<em>{body}</em>"),
                "underline" => format!("<u>{body}</u>"),
                "strike" => format!("<s>{body}</s>"),
                "code" => format!("<code>{body}</code>"),
                "superscript" => format!("<sup>{body}</sup>"),
                "subscript" => format!("<sub>{body}</sub>"),
                _ => return Err(format!("Unsupported HTML mark: {name}.")),
            };
        }
    }
    output.push_str(&body);
    Ok(())
}

fn render_macro_node(
    kind: &str,
    titled_node: (&Value, Option<&str>),
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    macros: &RenderMacros,
) -> Result<String, String> {
    let (node, title) = titled_node;
    let attrs = &node["attrs"];
    if kind == "image" {
        let mut content = String::new();
        render_leaf(
            kind,
            attrs,
            resources,
            heading_offset,
            state,
            &mut content,
            None,
        )?;
        let rendered = macros.render(kind, context! {title => title}, &content)?;
        require_block_anchor(attrs, &rendered)?;
        return Ok(rendered);
    }
    let (tag, extra) = if kind == "code_block" {
        ("figure".to_owned(), " class=\"code-block\"".to_owned())
    } else {
        container_tag(kind, attrs, heading_offset)?
    };
    let mut attributes = String::new();
    write_id(attrs, &mut attributes);
    attributes.push_str(&extra);
    let mut children_html = String::new();
    render_children(node, resources, heading_offset, state, &mut children_html)?;
    let rendered = macros.render(
        kind,
        context! {
            tag => tag,
            attributes => TemplateValue::from_safe_string(attributes),
            title => title,
            language => attrs["language"].as_str().unwrap_or("plain"),
            columns => attrs["columnWidthsPercent"].as_array().map(Vec::len),
            column_widths_percent => attrs["columnWidthsPercent"].as_array(),
        },
        &children_html,
    )?;
    require_block_anchor(attrs, &rendered)?;
    Ok(rendered)
}

fn require_block_anchor(attrs: &Value, rendered: &str) -> Result<(), String> {
    if let Some(id) = attrs["id"].as_str()
        && !rendered.contains(&format!("id=\"block-{id}\""))
    {
        return Err(format!(
            "Macro output omitted the required block anchor for {id}."
        ));
    }
    Ok(())
}

fn render_leaf(
    kind: &str,
    attrs: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    html: &mut String,
    title: Option<&str>,
) -> Result<(), String> {
    match kind {
        "horizontal_rule" => html.push_str("<hr>\n"),
        "image" => render_image(attrs, resources, html, title)?,
        "embed" => {
            let label = attrs["snippetId"]
                .as_str()
                .ok_or("Snippet embed has no key.")?;
            html.push_str("<section class=\"embedded-passage\">\n");
            render_snippet(label, resources, heading_offset, state, html)?;
            html.push_str("</section>\n");
        }
        _ => return Err(format!("Unsupported HTML leaf: {kind}.")),
    }
    Ok(())
}

fn render_children(
    node: &Value,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    html: &mut String,
) -> Result<(), String> {
    for child in children(node) {
        render_node(child, resources, heading_offset, state, html)?;
    }
    Ok(())
}

fn render_snippet(
    id: &str,
    resources: &HtmlResources<'_>,
    heading_offset: i16,
    state: &mut HtmlState,
    output: &mut String,
) -> Result<(), String> {
    let paragraph = resources
        .snippets
        .get(id)
        .ok_or_else(|| format!("Snippet {id} was not resolved by preflight."))?;
    render_node(paragraph, resources, heading_offset, state, output)
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map_or(&[][..], Vec::as_slice)
}

fn container_tag(
    kind: &str,
    attrs: &Value,
    heading_offset: i16,
) -> Result<(String, String), String> {
    let result = match kind {
        "paragraph" => ("p".to_owned(), String::new()),
        "heading" => {
            let level = i16::try_from(attrs["level"].as_u64().unwrap_or(1))
                .map_err(|_| "Heading level is too large.")?
                + heading_offset;
            if level < 1 {
                return Err("Heading level fell below 1 after article nesting.".to_owned());
            }
            let level = level.min(6);
            (format!("h{level}"), String::new())
        }
        "blockquote" => ("blockquote".to_owned(), String::new()),
        "box" => {
            let kind = attrs["kind"].as_str().unwrap_or("info");
            let mut extra = " class=\"callout\" data-kind=\"".to_owned();
            escape_into(kind, &mut extra);
            extra.push('"');
            ("aside".to_owned(), extra)
        }
        "boxHeading" => ("p".to_owned(), " class=\"callout-title\"".to_owned()),
        "list" if attrs["kind"].as_str() == Some("ordered") => {
            let order = attrs["order"].as_u64().unwrap_or(1);
            ("ol".to_owned(), format!(" start=\"{order}\""))
        }
        "list" => ("ul".to_owned(), String::new()),
        "listItem" => ("li".to_owned(), String::new()),
        "table" => ("table".to_owned(), String::new()),
        "table_row" => ("tr".to_owned(), String::new()),
        "table_cell" | "table_header" => {
            let tag = if kind == "table_header" { "th" } else { "td" };
            let mut extra = if tag == "th" {
                " scope=\"col\"".to_owned()
            } else {
                String::new()
            };
            for name in ["colspan", "rowspan"] {
                if let Some(value) = attrs[name].as_u64().filter(|value| *value > 1) {
                    write!(extra, " {name}=\"{value}\"").expect("writing to a String cannot fail");
                }
            }
            (tag.to_owned(), extra)
        }
        _ => return Err(format!("Unsupported HTML container: {kind}.")),
    };
    Ok(result)
}

fn render_image(
    attrs: &Value,
    resources: &HtmlResources<'_>,
    html: &mut String,
    title: Option<&str>,
) -> Result<(), String> {
    let src = attrs["src"].as_str().ok_or("Image has no source.")?;
    let alt = attrs["alt"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or("Image needs descriptive alt text.")?;
    let uri = resources
        .assets
        .get(src)
        .ok_or_else(|| format!("Image {src} was not resolved by preflight."))?;
    if !safe_output_url(uri, true) {
        return Err(format!("Unsafe resolved image destination: {uri}."));
    }
    let caption = resources.macros.is_none() && title.is_some();
    if caption {
        html.push_str("<figure>");
    }
    html.push_str("<img");
    write_id(attrs, html);
    html.push_str(" src=\"");
    escape_into(uri, html);
    html.push_str("\" alt=\"");
    escape_into(alt, html);
    html.push('"');
    for name in ["width", "height"] {
        if let Some(value) = attrs[name].as_u64() {
            write!(html, " {name}=\"{value}\"").expect("writing to a String cannot fail");
        }
    }
    html.push_str(">\n");
    if caption {
        write_caption(title, "figcaption", html);
        html.push_str("</figure>\n");
    }
    Ok(())
}

fn write_caption(title: Option<&str>, tag: &str, html: &mut String) {
    if let Some(title) = title {
        write!(html, "<{tag}>").expect("writing to a String cannot fail");
        escape_into(title, html);
        write!(html, "</{tag}>").expect("writing to a String cannot fail");
    }
}

fn write_column_group(attrs: &Value, html: &mut String) -> Result<(), String> {
    let widths = attrs["columnWidthsPercent"]
        .as_array()
        .ok_or("Table column percentages are required.")?;
    html.push_str("<colgroup>");
    for width in widths {
        let width = width.as_f64().ok_or("Invalid table column percentage.")?;
        write!(html, "<col style=\"width: {width}%\">").expect("writing to a String cannot fail");
    }
    html.push_str("</colgroup>");
    Ok(())
}

fn write_id(attrs: &Value, html: &mut String) {
    if let Some(id) = attrs["id"].as_str() {
        html.push_str(" id=\"block-");
        escape_into(id, html);
        html.push('"');
    }
}

fn safe_output_url(value: &str, image: bool) -> bool {
    if value.is_empty()
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    if let Ok(url) = Url::parse(value) {
        return if image {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || [
                    "data:image/png;base64,",
                    "data:image/jpeg;base64,",
                    "data:image/webp;base64,",
                    "data:image/gif;base64,",
                    "data:image/svg+xml;base64,",
                ]
                .iter()
                .any(|prefix| {
                    value.strip_prefix(prefix).is_some_and(|data| {
                        !data.is_empty()
                            && data.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                            })
                    })
                })
        } else {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || (url.scheme() == "mailto" && !url.path().is_empty())
        };
    }
    !value
        .split('/')
        .next()
        .is_some_and(|part| part.contains(':'))
}

fn escape_into(value: &str, html: &mut String) {
    for character in value.chars() {
        match character {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            '\'' => html.push_str("&#39;"),
            _ => html.push(character),
        }
    }
}

#[must_use]
pub(crate) fn escape_html(value: &str) -> String {
    let mut html = String::new();
    escape_into(value, &mut html);
    html
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use serde_json::json;

    use super::{HtmlResources, render_html_document};
    use crate::engine::publication::macros::RenderMacros;

    #[test]
    fn figure_titles_reach_body_macros_without_entering_verbatim_content() {
        let id = "01a0e2a0-0000-7000-8000-000000000005";
        let document = json!({"type":"doc","content":[
            {"type":"figure","attrs":{"id":id},"content":[
                {"type":"blockTitle","content":[{"type":"text","text":"Code <script> & title"}]},
                {"type":"code_block","attrs":{"language":"rust"},"content":[{"type":"text","text":"let x = \"<code>\";"}]}
            ]},
            {"type":"figure","content":[
                {"type":"blockTitle","content":[{"type":"text","text":"Image <title>"}]},
                {"type":"image","attrs":{"src":"asset","alt":"Diagram"}}
            ]},
            {"type":"figure","content":[
                {"type":"blockTitle","content":[{"type":"text","text":"Table & title"}]},
                {"type":"table","attrs":{"columnWidthsPercent":[100]},"content":[{"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"Cell"}]}]}]}]}
            ]}
        ]});
        let empty = HashMap::new();
        let assets = HashMap::from([("asset".to_owned(), "assets/image.png".to_owned())]);
        let snippets = HashMap::new();
        let excluded = HashSet::new();
        let defaults = RenderMacros::new("html", None).unwrap();
        for macros in [None, Some(&defaults)] {
            let output = render_html_document(
                &document,
                &HtmlResources {
                    links: &empty,
                    assets: &assets,
                    fields: &empty,
                    snippets: &snippets,
                    excluded_block_ids: &excluded,
                    macros,
                },
                0,
            )
            .unwrap();
            assert!(output.contains(&format!("id=\"block-{id}\"")));
            assert!(output.contains("<figcaption>Code &lt;script&gt; &amp; title</figcaption>"));
            assert!(output.contains("<figcaption>Image &lt;title&gt;</figcaption>"));
            assert!(output.contains("<caption>Table &amp; title</caption>"));
            assert!(output.contains("<code>let x = &quot;&lt;code&gt;&quot;;</code>"));
            assert!(!output.contains("<code>Code"));
            assert!(!output.contains("<script>"));
        }
        let overrides = RenderMacros::new("html", Some(
            "{% macro start_code_block(ctx) %}<figure{{ ctx.attributes }}><figcaption class=\"brand-title\">{{ ctx.title }}</figcaption><pre><code>{% endmacro %}"
        )).unwrap();
        let output = render_html_document(
            &document,
            &HtmlResources {
                links: &empty,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &excluded,
                macros: Some(&overrides),
            },
            0,
        )
        .unwrap();
        assert!(output.contains("class=\"brand-title\">Code &lt;script&gt; &amp; title"));
        let excluded = HashSet::from([id.to_owned()]);
        let output = render_html_document(
            &document,
            &HtmlResources {
                links: &empty,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &excluded,
                macros: Some(&defaults),
            },
            0,
        )
        .unwrap();
        assert!(!output.contains("Code &lt;script&gt;"));
        assert!(!output.contains("let x"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn custom_macros_wrap_nested_content_after_rust_resolves_it() {
        let macros = RenderMacros::new(
            "html",
            Some(r#"
{% macro heading(ctx, children) %}<{{ ctx.tag }}{{ ctx.attributes }} class="brand-heading">{{ children }}</{{ ctx.tag }}>{% endmacro %}
{% macro paragraph(ctx, children) %}<p{{ ctx.attributes }} class="brand-copy">{{ children }}</p>{% endmacro %}
{% macro strong(ctx, children) %}<b class="brand-strong">{{ children }}</b>{% endmacro %}
{% macro link(ctx, children) %}<a class="brand-link" href="{{ ctx.target }}">{{ children }}</a>{% endmacro %}
{% macro image(ctx, children) %}<figure class="brand-image">{{ children }}</figure>{% endmacro %}
{% macro list(ctx, children) %}<{{ ctx.tag }}{{ ctx.attributes }} class="brand-list">{{ children }}</{{ ctx.tag }}>{% endmacro %}
{% macro table(ctx, children) %}<table{{ ctx.attributes }} class="brand-table"><tbody>{{ children }}</tbody></table>{% endmacro %}
"#),
        )
        .unwrap();
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"id":"01a0e2a0-0000-7000-8000-000000000001","level":1},"content":[{"type":"text","text":"Head <one>"}]},
            {"type":"paragraph","content":[
                {"type":"text","text":"Bold & safe","marks":[{"type":"strong"}]},
                {"type":"text","text":" Link","marks":[{"type":"link","attrs":{"href":"target"}}]}
            ]},
            {"type":"list","attrs":{"kind":"bullet"},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"Nested"}]}]}]},
            {"type":"table","attrs":{"columnWidthsPercent":[100]},"content":[{"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"Cell"}]}]}]}]},
            {"type":"image","attrs":{"src":"asset","alt":"Sky <blue>"}}
        ]});
        let links = HashMap::from([(
            "target".to_owned(),
            "https://example.com/?a=1&b=2".to_owned(),
        )]);
        let assets = HashMap::from([("asset".to_owned(), "assets/sky.png".to_owned())]);
        let empty = HashMap::new();
        let snippets = HashMap::new();
        let rendered = render_html_document(
            &document,
            &HtmlResources {
                links: &links,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &HashSet::new(),
                macros: Some(&macros),
            },
            0,
        )
        .unwrap();
        assert!(rendered.contains("class=\"brand-heading\">Head &lt;one&gt;</h1>"));
        assert!(rendered.contains("<b class=\"brand-strong\">Bold &amp; safe</b>"));
        assert!(
            rendered.contains("href=\"https://example.com/?a=1&amp;b=2\"> Link</a>"),
            "{rendered}"
        );
        assert!(rendered.contains("class=\"brand-list\"><li><p class=\"brand-copy\">Nested</p>"));
        assert!(rendered.contains(
            "class=\"brand-table\"><tbody><tr><th scope=\"col\"><p class=\"brand-copy\">Cell</p>"
        ));
        assert!(rendered.contains(
            "<figure class=\"brand-image\"><img src=\"assets/sky.png\" alt=\"Sky &lt;blue&gt;\">"
        ));
        assert!(!rendered.contains("<one>"));
        let missing_anchor = RenderMacros::new(
            "html",
            Some("{% macro heading(ctx, children) %}<h1>{{ children }}</h1>{% endmacro %}"),
        )
        .unwrap();
        let error = render_html_document(
            &document,
            &HtmlResources {
                links: &links,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &HashSet::new(),
                macros: Some(&missing_anchor),
            },
            0,
        )
        .unwrap_err();
        assert!(error.contains("heading 01a0e2a0-0000-7000-8000-000000000001"));
        assert!(error.contains("omitted the required block anchor"));

        let omit_children = RenderMacros::new(
            "html",
            Some("{% macro paragraph(ctx, children) %}<p>Hidden</p>{% endmacro %}"),
        )
        .unwrap();
        let missing_field = json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"field","attrs":{"key":"missing"}}]}
        ]});
        let error = render_html_document(
            &missing_field,
            &HtmlResources {
                links: &links,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &HashSet::new(),
                macros: Some(&omit_children),
            },
            0,
        )
        .unwrap_err();
        assert!(error.contains("Field missing was not resolved"));

        let unsafe_links = HashMap::from([("target".to_owned(), "javascript:alert(1)".to_owned())]);
        let error = render_html_document(
            &document,
            &HtmlResources {
                links: &unsafe_links,
                assets: &assets,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &HashSet::new(),
                macros: Some(&macros),
            },
            0,
        )
        .unwrap_err();
        assert!(error.contains("Unsafe resolved link destination"));
    }

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
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &excluded_block_ids,
                macros: None,
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
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: None,
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
                links: &good,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: None,
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
                    links: &bad,
                    assets: &assets,
                    fields: &fields,
                    snippets: &snippets,
                    excluded_block_ids: &exclusions,
                    macros: None,
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
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: None,
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
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &empty_passages,
                excluded_block_ids: &exclusions,
                macros: None,
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
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: None,
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
                links: &host_links,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: None,
            },
            0,
        )
        .unwrap();
        assert!(html.contains("href=\"https://private.example/source\""));
    }
}
