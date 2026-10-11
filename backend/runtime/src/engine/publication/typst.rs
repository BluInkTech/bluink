// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use minijinja::context;
use minijinja::value::Value as TemplateValue;
use serde_json::Value;

use crate::engine::document::validation::validate_document;
use crate::engine::publication::macros::RenderMacros;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TypstLink {
    Label(String),
    Url(String),
    PlainText,
}

pub(crate) struct TypstResources<'a> {
    pub(crate) links: &'a HashMap<String, TypstLink>,
    pub(crate) assets: &'a HashMap<String, String>,
    pub(crate) fields: &'a HashMap<String, String>,
    pub(crate) snippets: &'a HashMap<String, Value>,
    pub(crate) excluded_block_ids: &'a HashSet<String>,
    pub(crate) macros: Option<&'a RenderMacros>,
}

#[derive(Default)]
pub(crate) struct TypstState {
    citations: HashMap<String, usize>,
}

pub(crate) fn render_typst_document(
    document: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
) -> Result<String, String> {
    render_typst_document_in_scope(
        document,
        resources,
        heading_offset,
        &mut TypstState::default(),
    )
}

pub(crate) fn render_typst_document_in_scope(
    document: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
) -> Result<String, String> {
    validate_document(document).map_err(|error| error.to_string())?;
    for (id, paragraph) in resources.snippets {
        crate::engine::project::snippets::validate_reference_paragraph(paragraph, true)
            .map_err(|error| format!("Snippet {id}: {error}"))?;
    }
    let mut source = String::new();
    render_node(document, resources, heading_offset, state, &mut source)?;
    Ok(source)
}

fn render_node(
    node: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
    output: &mut String,
) -> Result<(), String> {
    render_node_inner(node, resources, heading_offset, state, output, None).map_err(|error| {
        let kind = node["type"].as_str().unwrap_or("unknown");
        let id = node["attrs"]["id"]
            .as_str()
            .map(|value| format!(" {value}"))
            .unwrap_or_default();
        format!("Typst node {kind}{id}: {error}")
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn render_node_inner(
    node: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
    destination: &mut String,
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
    let marks = node["marks"].as_array().map_or(&[][..], Vec::as_slice);
    let mut body = String::new();
    let output = &mut body;
    match kind {
        "figure" => {
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
            output.push_str("#block[\n");
            render_node_inner(
                content,
                resources,
                heading_offset,
                state,
                output,
                Some(title.as_str()).filter(|title| !title.trim().is_empty()),
            )?;
            output.push(']');
            if let Some(id) = attrs["id"].as_str() {
                write!(output, " <block-{id}>").expect("writing to a String cannot fail");
            }
            output.push_str("\n\n");
        }
        "doc" | "table_row" => render_children(node, resources, heading_offset, state, output)?,
        "text" => {
            let text = node["text"].as_str().ok_or("Text node has no text.")?;
            if marks.iter().any(|mark| mark["type"] == "code") {
                output.push_str(&quote(text)?);
            } else {
                write_text(text, output)?;
            }
        }
        "hard_break" => output.push_str("#linebreak()"),
        "horizontal_rule" => output.push_str("#line(length: 100%, stroke: muted)\n\n"),
        "field" => {
            let key = attrs["key"].as_str().ok_or("Field has no key.")?;
            let value = resources
                .fields
                .get(key)
                .ok_or_else(|| format!("Field {key} was not resolved by preflight."))?;
            write_text(value, output)?;
        }
        "code_block" => {
            let text = children(node)
                .iter()
                .filter_map(|child| child["text"].as_str())
                .collect::<String>();
            let language = attrs["language"].as_str().unwrap_or("plain");
            write!(
                output,
                "#raw({}, block: true, lang: {})\n\n",
                quote(&text)?,
                quote(language)?
            )
            .expect("writing to a String cannot fail");
            if resources.macros.is_none() {
                let mut titled = String::new();
                write_caption(title, true, &mut titled)?;
                titled.push_str(output);
                *output = titled;
            }
        }
        "image" => {
            let src = attrs["src"].as_str().ok_or("Image has no source.")?;
            let path = resources
                .assets
                .get(src)
                .ok_or_else(|| format!("Image {src} was not resolved by preflight."))?;
            let alt = attrs["alt"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .ok_or("Image needs descriptive alt text.")?;
            write!(
                output,
                "#align(center)[#image({}, alt: {}",
                quote(path)?,
                quote(alt)?
            )
            .expect("writing to a String cannot fail");
            for dimension in ["width", "height"] {
                if let Some(value) = attrs[dimension].as_u64() {
                    write!(output, ", {dimension}: {value}pt")
                        .expect("writing to a String cannot fail");
                }
            }
            output.push_str(")]\n\n");
            if resources.macros.is_none() {
                write_caption(title, false, output)?;
            }
        }
        "paragraph" => {
            let mut children = String::new();
            render_children(node, resources, heading_offset, state, &mut children)?;
            if let Some(macros) = resources.macros {
                output.push_str(&macros.render("paragraph", TemplateValue::from(()), &children)?);
            } else {
                write!(output, "#par[{children}]\n\n").expect("writing to a String cannot fail");
            }
        }
        "heading" => {
            let level = i16::try_from(attrs["level"].as_u64().unwrap_or(1))
                .map_err(|_| "Heading level is too large.")?
                + heading_offset;
            if level < 1 {
                return Err("Heading level fell below 1 after book nesting.".to_owned());
            }
            let mut children = String::new();
            render_children(node, resources, heading_offset, state, &mut children)?;
            let anchor = attrs["id"].as_str().map(|id| format!("block-{id}"));
            if let Some(macros) = resources.macros {
                let rendered = macros.render(
                    "heading",
                    context! {level => level, anchor => anchor},
                    &children,
                )?;
                if let Some(id) = attrs["id"].as_str()
                    && !rendered.contains(&format!("<block-{id}>"))
                {
                    return Err(format!(
                        "Macro output omitted the required block anchor for {id}."
                    ));
                }
                output.push_str(&rendered);
            } else {
                write!(output, "#heading(level: {level})[{children}]")
                    .expect("writing to a String cannot fail");
                if let Some(anchor) = anchor {
                    write!(output, " <{anchor}>").expect("writing to a String cannot fail");
                }
                output.push_str("\n\n");
            }
        }
        "blockquote" => {
            output.push_str("#quote(block: true)[");
            render_children(node, resources, heading_offset, state, output)?;
            output.push_str("]\n\n");
        }
        "box" => {
            output.push_str(
                "#block(fill: surface, stroke: accent, inset: 10pt, radius: 4pt)[#strong[",
            );
            write_text(attrs["kind"].as_str().unwrap_or("Info"), output)?;
            output.push_str("]#linebreak()\n");
            render_children(node, resources, heading_offset, state, output)?;
            output.push_str("]\n\n");
        }
        "boxHeading" => {
            output.push_str("#strong[");
            render_children(node, resources, heading_offset, state, output)?;
            output.push_str("]\n");
        }
        "list" => {
            let ordered = attrs["kind"] == "ordered";
            let start = attrs["order"].as_u64().unwrap_or(1);
            let mut children = String::new();
            render_children(node, resources, heading_offset, state, &mut children)?;
            if let Some(macros) = resources.macros {
                output.push_str(&macros.render(
                    "list",
                    context! {ordered => ordered, start => start},
                    &children,
                )?);
            } else if ordered {
                write!(output, "#enum(start: {start},\n{children})\n\n")
                    .expect("writing to a String cannot fail");
            } else {
                write!(output, "#list(\n{children})\n\n").expect("writing to a String cannot fail");
            }
        }
        "listItem" => {
            output.push('[');
            render_children(node, resources, heading_offset, state, output)?;
            output.push_str("],\n");
        }
        "table" => render_table(node, resources, heading_offset, state, output, title)?,
        "table_cell" | "table_header" => {
            let colspan = attrs["colspan"].as_u64().unwrap_or(1);
            let rowspan = attrs["rowspan"].as_u64().unwrap_or(1);
            if colspan > 1 || rowspan > 1 {
                write!(
                    output,
                    "table.cell(colspan: {colspan}, rowspan: {rowspan})["
                )
                .expect("writing to a String cannot fail");
            } else {
                output.push('[');
            }
            render_children(node, resources, heading_offset, state, output)?;
            output.push_str("],\n");
        }
        "footnote" | "citation" => {
            let label = if kind == "citation" && attrs["source"] == "snippet" {
                Some(
                    attrs["snippetId"]
                        .as_str()
                        .ok_or("Snippet citation has no key.")?,
                )
            } else {
                None
            };
            if let Some(key) = label
                && let Some(number) = state.citations.get(key)
            {
                write!(output, "#footnote(label(\"citation-{number}\"))")
                    .expect("writing to a String cannot fail");
            } else {
                output.push_str("#footnote[");
                if let Some(key) = label {
                    let number = state.citations.len() + 1;
                    state.citations.insert(key.to_owned(), number);
                    render_snippet(key, resources, heading_offset, state, output)?;
                    write!(output, "] <citation-{number}>")
                        .expect("writing to a String cannot fail");
                } else {
                    let fragment = attrs["paragraph"]
                        .as_object()
                        .map(|_| &attrs["paragraph"])
                        .ok_or("Reference has no local paragraph.")?;
                    render_node(fragment, resources, heading_offset, state, output)?;
                    output.push(']');
                }
            }
        }
        "embed" => {
            let label = attrs["snippetId"]
                .as_str()
                .ok_or("Snippet embed has no key.")?;
            render_snippet(label, resources, heading_offset, state, output)?;
        }
        _ => return Err(format!("Unsupported Typst node: {kind}.")),
    }
    if matches!(kind, "image" | "code_block")
        && let Some(macros) = resources.macros
    {
        let title = title.map(quote).transpose()?;
        body = macros.render(
            kind,
            context! {title => title, language => attrs["language"].as_str().unwrap_or("plain")},
            &body,
        )?;
    }
    for mark in marks.iter().rev() {
        let name = mark["type"].as_str().ok_or("Mark has no type.")?;
        body = match name {
            "strong" | "superscript" | "subscript" if resources.macros.is_some() => resources
                .macros
                .unwrap()
                .render(name, TemplateValue::from(()), &body)?,
            "strong" => format!("#strong[{body}]"),
            "emphasis" => format!("#emph[{body}]"),
            "underline" => format!("#underline[{body}]"),
            "strike" => format!("#strike[{body}]"),
            "code" => format!("#raw({body})"),
            "superscript" => format!("#super[{body}]"),
            "subscript" => format!("#sub[{body}]"),
            "link" => {
                let href = mark["attrs"]["href"]
                    .as_str()
                    .ok_or("Link has no destination.")?;
                let target = resources
                    .links
                    .get(href)
                    .ok_or_else(|| format!("Link {href} was not resolved by preflight."))?;
                let expression = match target {
                    TypstLink::Label(label) => format!("label({})", quote(label)?),
                    TypstLink::Url(url) => quote(url)?,
                    TypstLink::PlainText => continue,
                };
                if let Some(macros) = resources.macros {
                    macros.render("link", context! {destination => expression}, &body)?
                } else {
                    format!("#link({expression})[{body}]")
                }
            }
            other => return Err(format!("Unsupported Typst mark: {other}.")),
        };
    }
    destination.push_str(&body);
    Ok(())
}

fn render_children(
    node: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
    output: &mut String,
) -> Result<(), String> {
    for child in children(node) {
        render_node(child, resources, heading_offset, state, output)?;
    }
    Ok(())
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map_or(&[][..], Vec::as_slice)
}

fn render_snippet(
    id: &str,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
    output: &mut String,
) -> Result<(), String> {
    let paragraph = resources
        .snippets
        .get(id)
        .ok_or_else(|| format!("Snippet {id} was not resolved by preflight."))?;
    render_node(paragraph, resources, heading_offset, state, output)
}

fn render_table(
    node: &Value,
    resources: &TypstResources<'_>,
    heading_offset: i16,
    state: &mut TypstState,
    output: &mut String,
    title: Option<&str>,
) -> Result<(), String> {
    let widths = node["attrs"]["columnWidthsPercent"]
        .as_array()
        .ok_or("Table column percentages are required.")?;
    let tracks = widths
        .iter()
        .map(|width| {
            width
                .as_f64()
                .map(|width| format!("{width}fr"))
                .ok_or_else(|| "Invalid table column percentage.".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let quoted_title = title.map(quote).transpose()?;
    let mut content = String::new();
    for (index, row) in children(node).iter().enumerate() {
        let header = index == 0
            && !children(row).is_empty()
            && children(row)
                .iter()
                .all(|cell| cell["type"] == "table_header");
        if header {
            content.push_str("table.header(\n");
        }
        render_children(row, resources, heading_offset, state, &mut content)?;
        if header {
            content.push_str("),\n");
        }
    }
    if let Some(macros) = resources.macros {
        output.push_str(&macros.render(
            "table",
            context! {
                columns => widths.len(), column_widths_percent => widths,
                column_tracks => format!("({tracks},)"), title => quoted_title,
            },
            &content,
        )?);
    } else {
        write_caption(title, true, output)?;
        write!(output, "#table(columns: ({tracks},),\n{content})\n\n")
            .expect("writing to a String cannot fail");
    }
    Ok(())
}

fn write_caption(title: Option<&str>, sticky: bool, output: &mut String) -> Result<(), String> {
    if let Some(title) = title {
        write!(
            output,
            "#block(sticky: {sticky}, above: 0.4em, below: 0.4em)[#text(size: 0.9em, {})]\n\n",
            quote(title)?
        )
        .expect("writing to a String cannot fail");
    }
    Ok(())
}

fn write_text(value: &str, output: &mut String) -> Result<(), String> {
    output.push_str("#text(");
    output.push_str(&quote(value)?);
    output.push(')');
    Ok(())
}

fn quote(value: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    use serde_json::json;

    use super::{TypstLink, TypstResources, render_typst_document};
    use crate::engine::publication::macros::RenderMacros;
    use crate::engine::publication::plan::PublicationFormat;
    use crate::engine::publication::style::typst_values;
    use crate::engine::publication::theme::capture_theme;

    #[test]
    fn title_nodes_are_quoted_caption_context_and_never_code_children() {
        let document = json!({"type":"doc","content":[{"type":"figure","content":[
            {"type":"blockTitle","content":[{"type":"text","text":"Caption # \"quoted\""}]},
            {"type":"code_block","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn main() {\n  println!(\"#literal\");\n}"}]}
        ]}]});
        let empty = HashMap::new();
        let links = HashMap::new();
        let snippets = HashMap::new();
        let excluded = HashSet::new();
        let defaults = RenderMacros::new("pdf", None).unwrap();
        let overrides = RenderMacros::new(
            "pdf",
            Some(
                "{% macro start_code_block(ctx) %}#text(fill: red, {{ ctx.title }}){% endmacro %}",
            ),
        )
        .unwrap();
        for macros in [None, Some(&defaults), Some(&overrides)] {
            let output = render_typst_document(
                &document,
                &TypstResources {
                    links: &links,
                    assets: &empty,
                    fields: &empty,
                    snippets: &snippets,
                    excluded_block_ids: &excluded,
                    macros,
                },
                0,
            )
            .unwrap();
            assert!(output.contains("\"Caption # \\\"quoted\\\"\""));
            assert!(output.contains("#raw(\"fn main() {\\n  println!(\\\"#literal\\\");\\n}\", block: true, lang: \"rust\")"));
            assert!(!output.contains("#raw(\"Caption"));
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn renders_document_and_compiles_with_the_packaged_typst() {
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"level":1,"id":"01a0e2a0-0000-7000-8000-000000000001"},"content":[{"type":"text","text":"A <heading>"}]},
            {"type":"paragraph","content":[
                {"type":"text","text":"Linked & safe","marks":[{"type":"link","attrs":{"href":"external"}}]},
                {"type":"text","text":" Strong","marks":[{"type":"strong"}]},
                {"type":"text","text":"2","marks":[{"type":"superscript"}]},
                {"type":"text","text":"O","marks":[{"type":"subscript"}]},
                {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Footnote text"}]}}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}}
            ]},
            {"type":"list","attrs":{"kind":"ordered","order":2},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"Item"}]}]}]},
            {"type":"figure","content":[{"type":"blockTitle","content":[{"type":"text","text":"Measurements"}]},{"type":"table","attrs":{"columnWidthsPercent":[35,65]},"content":[{"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"A"}]}]},{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"B"}]}]}]},{"type":"table_row","content":[{"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"1"}]}]},{"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"2"}]}]}]}]}]},
            {"type":"box","attrs":{"kind":"warning"},"content":[{"type":"paragraph","content":[{"type":"text","text":"Caution"}]}]},
            {"type":"figure","content":[{"type":"blockTitle","content":[{"type":"text","text":"An image # <title>"}]},{"type":"image","attrs":{"src":"/assets/test/image.png","alt":"One transparent pixel","width":24}}]},
            {"type":"figure","content":[{"type":"blockTitle","content":[{"type":"text","text":"Code # <title>"}]},{"type":"code_block","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn main() {}"}]}]},
            {"type":"horizontal_rule"},
            {"type":"embed","attrs":{"snippetId":"source_one"}}
        ]});
        let links = HashMap::from([(
            "external".to_owned(),
            TypstLink::Url("https://example.com".to_owned()),
        )]);
        let assets = HashMap::from([(
            "/assets/test/image.png".to_owned(),
            "assets/test.png".to_owned(),
        )]);
        let fields = HashMap::new();
        let snippets = HashMap::from([(
            "source_one".to_owned(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Source passage"}]}),
        )]);
        let excluded = HashSet::new();
        let content = render_typst_document(
            &document,
            &TypstResources {
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &excluded,
                macros: None,
            },
            0,
        )
        .unwrap();
        assert!(content.contains("#text(\"A <heading>\")"));
        assert!(content.contains("#link(\"https://example.com\")"));
        assert!(content.contains("#footnote["));
        assert!(content.contains("#super[#text(\"2\")]"));
        assert!(content.contains("#sub[#text(\"O\")]"));
        assert!(content.contains("columns: (35fr, 65fr,)"));
        assert!(content.contains("\"Code # <title>\""));
        let binary = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "linux-x64/typst"
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "windows-x64/typst.exe"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-arm64/typst"
        } else {
            return;
        };
        let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");
        let typst_binary = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../typst/assets")
            .join(binary);
        if !typst_binary.is_file() {
            return;
        }
        let theme = capture_theme(
            Path::new("/unused-bluink-theme-root"),
            "classic",
            PublicationFormat::Pdf,
        )
        .unwrap();
        let temp = std::env::temp_dir().join(format!("bluink-typst-doc-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::create_dir_all(temp.join("assets")).unwrap();
        std::fs::copy(
            vendor.join("../test-fixtures/one-pixel.png"),
            temp.join("assets/test.png"),
        )
        .unwrap();
        std::fs::write(
            temp.join("content.typ"),
            format!("#import \"theme-values.typ\": *\n{content}"),
        )
        .unwrap();
        std::fs::write(
            temp.join("theme-values.typ"),
            typst_values(&theme.settings).unwrap(),
        )
        .unwrap();
        std::fs::copy(
            vendor.join("themes/classic/pdf/document.typ"),
            temp.join("document.typ"),
        )
        .unwrap();
        let result = std::process::Command::new(&typst_binary)
            .arg("compile")
            .arg(temp.join("document.typ"))
            .arg(temp.join("document.pdf"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            std::fs::read(temp.join("document.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let macros = RenderMacros::new("pdf", Some(r"
{% macro heading(ctx, children) %}#heading(level: {{ ctx.level }})[#emph[{{ children }}]]{% if ctx.anchor %} <{{ ctx.anchor }}>{% endif %}

{% endmacro %}
{% macro paragraph(ctx, children) %}#block(inset: 6pt, fill: surface)[{{ children }}]

{% endmacro %}
{% macro strong(ctx, children) %}#strong[#emph[{{ children }}]]{% endmacro %}
{% macro link(ctx, children) %}#link({{ ctx.destination }})[#underline[{{ children }}]]{% endmacro %}
{% macro image(ctx, children) %}#block(inset: 2pt)[{{ children }}]{% endmacro %}
{% macro list(ctx, children) %}{% if ctx.ordered %}#enum(start: {{ ctx.start }},
{% else %}#list(
{% endif %}{{ children }})

{% endmacro %}
{% macro table(ctx, children) %}#table(columns: {{ ctx.columns }},
{{ children }})

{% endmacro %}
")).unwrap();
        let wrapped = render_typst_document(
            &document,
            &TypstResources {
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &excluded,
                macros: Some(&macros),
            },
            0,
        )
        .unwrap();
        assert!(wrapped.contains("#block(inset: 6pt, fill: surface)["));
        assert!(wrapped.contains("#heading(level: 1)[#emph["));
        assert!(wrapped.contains("#strong[#emph["));
        assert!(wrapped.contains("#link(\"https://example.com\")[#underline["));
        assert!(wrapped.contains("#block(inset: 2pt)[#align(center)[#image("));
        assert!(wrapped.contains("#enum(start: 2,"));
        assert!(wrapped.contains("#table(columns: 2,"));
        let missing_anchor = RenderMacros::new("pdf", Some(
            "{% macro heading(ctx, children) %}#heading(level: 1)[{{ children }}]{% endmacro %}",
        )).unwrap();
        let error = render_typst_document(
            &document,
            &TypstResources {
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &excluded,
                macros: Some(&missing_anchor),
            },
            0,
        )
        .unwrap_err();
        assert!(error.contains("heading 01a0e2a0-0000-7000-8000-000000000001"));
        assert!(error.contains("omitted the required block anchor"));
        std::fs::write(
            temp.join("content.typ"),
            format!("#import \"theme-values.typ\": *\n{wrapped}"),
        )
        .unwrap();
        let wrapped_result = std::process::Command::new(&typst_binary)
            .arg("compile")
            .arg(temp.join("document.typ"))
            .arg(temp.join("wrapped.pdf"))
            .output()
            .unwrap();
        assert!(
            wrapped_result.status.success(),
            "{}",
            String::from_utf8_lossy(&wrapped_result.stderr)
        );
        assert!(
            std::fs::read(temp.join("wrapped.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        std::fs::remove_dir_all(temp).unwrap();
    }
}
