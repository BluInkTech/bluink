// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{Overrides, ResolvedTheme, Role, css, resolve, typst};
use crate::domain::publication::PdfProfile;
use crate::export_jobs::pdf::{PdfBundle, publish_pdf};
use serde_json::json;
use std::{collections::BTreeMap, fmt::Write, path::Path};

fn theme() -> ResolvedTheme {
    resolve(&super::tests::fixture(), "pdf", &Overrides::default()).unwrap()
}

#[test]
fn every_role_has_stable_names_and_compiles_to_root_relative_css_and_typed_typst_data() {
    let theme = theme();
    let css = css::stylesheet(&theme, "Theme proof").unwrap();
    assert!(css.contains(":root { font-size: 12.000000pt;"));
    assert!(css.contains("font-size: 2.000000rem;"));
    assert!(css.contains("line-height: 1.200000;"));
    assert!(css.contains("margin-block: 2.000000rem 0.750000rem;"));
    assert!(css.contains("font-family: \"Source Sans 3\";"));
    let values = typst::values(&theme).unwrap();
    let first_line = values.lines().next().unwrap();
    let literal = first_line
        .strip_prefix("#let publication-theme = json(bytes(")
        .unwrap()
        .strip_suffix("))")
        .unwrap();
    let data: String = serde_json::from_str(literal).unwrap();
    let data: serde_json::Value = serde_json::from_str(&data).unwrap();
    for role in Role::ALL {
        assert_eq!(
            serde_json::to_value(role).unwrap().as_str(),
            Some(role.name())
        );
        assert!(data["styles"].get(role.name()).is_some());
        if role != Role::Body {
            assert!(css.contains(&format!("/* {} */", role.name())));
        }
    }
    assert_eq!(data["styles"]["heading1"]["fontFamily"], "Source Sans 3");
    assert_eq!(data["styles"]["heading1"]["fontSizePt"], 24.0);
}

#[test]
fn overrides_reach_both_adapters_after_resolution_and_print_geometry_stays_physical() {
    let overrides = serde_json::from_value(json!({"shared":{
        "tokens":{"root-size":{"type":"length","value":"18pt"}},
        "styles":{"heading1":{"fontSize":"3rem","alignment":"center","keepTogether":true},"boxWarning":{"borderStyle":"dashed","padding":"2rem"}},
        "page":{"width":"8.5in","height":"11in","orientation":"landscape","marginLeft":"1in","chapterStart":"continuous"}
    },"targets":{}})).unwrap();
    let resolved = resolve(&super::tests::fixture(), "html", &overrides).unwrap();
    let css = css::stylesheet(&resolved, "Proof").unwrap();
    assert!(css.contains("font-size: 3.000000rem;"));
    assert!(css.contains("size: 792.000000pt 612.000000pt;"));
    assert!(css.contains("72.000000pt;"));
    assert!(!css.contains(".publication-chapter { break-before: page"));
    assert!(css.contains("nav.publication-contents { break-before: page"));
    assert!(typst::values(&resolved).unwrap().contains("54.0"));
}

#[test]
fn authored_running_text_and_metadata_cannot_close_css_or_execute_typst() {
    let title = "</style><script>bad()</script>\"\\\n#panic(\"bad\")";
    let mut theme = theme();
    theme.page.header.left = vec![super::RunningPart::Text(title.into())];
    let css = css::stylesheet(&theme, title).unwrap();
    assert!(!css.contains("</style>"));
    assert!(css.contains("\\3c /style>"));
    assert!(css.contains("\\a "));
    assert!(typst::document_shell(title, "en").contains("\\n#panic(\\\"bad\\\")"));
    let mut invalid = theme;
    invalid.styles.get_mut(&Role::Body).unwrap().font_family = "missing".into();
    assert!(css::stylesheet(&invalid, "Proof").is_err());
    assert!(typst::values(&invalid).is_err());
}

#[test]
fn a_font_selected_only_by_a_semantic_role_is_packaged_for_offline_html() {
    let mut definition = super::tests::fixture();
    definition
        .styles
        .get_mut(&Role::Caption)
        .unwrap()
        .font_family = Some(super::Value::Literal("inter".into()));
    let theme = resolve(&definition, "html", &Overrides::default()).unwrap();
    assert!(theme.fonts.contains("inter"));
    let html = crate::export_jobs::fonts::embed_html_fonts(
        "<html><head></head><body></body></html>",
        theme.fonts.iter().map(String::as_str),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web"),
    )
    .unwrap();
    assert!(html.contains("font-family: 'Inter'"));
    assert!(html.contains("data:font/woff2;base64,"));
    assert!(!html.contains("url('./"));
}

fn sample() -> String {
    let mut content = String::from("#import \"theme-values.typ\": *\n");
    content.push_str(r#"
#publication-chapter("First chapter", first: true)[
  #heading(level: 1)[Portable theme proof#context metadata((kind:"heading-style",size:text.size.pt(),font:text.font)) <heading-style>]
  #publication-par[Line one#metadata("line-one") <line-one>#linebreak()Line two#metadata("line-two") <line-two>]
  #publication-par[Body with #strong[strong], #emph[emphasis], #underline[underline], #strike[strike],
    #super[2], #sub[O], #raw("x < y"), #link("https://example.com")[a link],
    and a rich #footnote[Note with #strong[strong text].] <first-note> repeated #footnote(label("first-note")).]
  #publication-block("quote")[#publication-par[A quote inherits its italic role.]]
  #publication-block("embed")[#publication-par[An embedded passage.]]
  #publication-block("list")[#list([First item], [Second item])]
  #publication-block("list")[#enum(start: 7, [Number seven], [Number eight])]
  #publication-block("table")[#table(columns: (20fr,30fr,50fr),
    table.header(publication-cell(column:0,row:0,header:true)[A],publication-cell(column:1,row:0,header:true)[B],publication-cell(column:2,row:0,header:true)[C]),
    publication-cell(column:0,row:1,rowspan:2)[Two rows],publication-cell(column:1,row:1,colspan:2)[Two columns],
    publication-cell(column:1,row:2)[One],publication-cell(column:2,row:2)[Two])]
  #publication-figure(caption: [Image caption])[#image("pixel.png", width:48pt,height:36pt,alt:"Transparent pixel")]
  #publication-figure(kind: raw, caption: [Code caption])[#publication-code("fn main() {}",language:"rust")]
  #publication-rule()
]
"#);
    for role in [
        Role::Box,
        Role::BoxNote,
        Role::BoxTip,
        Role::BoxImportant,
        Role::BoxWarning,
        Role::BoxCaution,
    ] {
        writeln!(content, "#publication-block(\"{}\")[#publication-block(\"boxHeading\")[{}]#publication-par[Theme-controlled box content.]]", role.name(), role.name()).unwrap();
    }
    content.push_str(r#"
#publication-chapter("Second chapter")[
  #heading(level:2)[Second chapter]
  #heading(level:3)[Heading three]
  #heading(level:4)[Heading four]
  #heading(level:5)[Heading five]
  #heading(level:6)[Heading six]
  #publication-par[The final chapter uses the same theme.\[#link(label("ref-chapter-1"))[1], #link(label("ref-chapter-2"))[2]\]]
  #publication-references[
    #publication-reference-item("1", "ref-chapter-1")[A cited source whose text is long enough to wrap onto a second line beneath its number.]
    #publication-reference-item("2", "ref-chapter-2")[Another source.]
  ]
]
#context metadata((kind:"style-proof",pages:counter(page).final().first(),
  baseline:query(label("line-two")).first().location().position().y.pt() - query(label("line-one")).first().location().position().y.pt())) <style-proof>
"#);
    content
}

fn bundle(theme: &ResolvedTheme) -> PdfBundle {
    PdfBundle {
        shell_path: "document.typ".into(),
        page_lines: Vec::new(),
        files: BTreeMap::from([
            (
                "document.typ".into(),
                typst::document_shell("Portable <theme> proof", "en").into_bytes(),
            ),
            (
                "theme-values.typ".into(),
                typst::values(theme).unwrap().into_bytes(),
            ),
            ("content.typ".into(), sample().into_bytes()),
            (
                "pixel.png".into(),
                include_bytes!("../../../../test-fixtures/one-pixel.png").to_vec(),
            ),
        ]),
    }
}

fn rounded(values: &[serde_json::Value]) -> serde_json::Value {
    fn round(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Number(number) if number.is_f64() => {
                json!((number.as_f64().unwrap() * 1e6).round() / 1e6)
            }
            serde_json::Value::Array(items) => items.iter().map(round).collect(),
            serde_json::Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| (key.clone(), round(value)))
                .collect(),
            other => other.clone(),
        }
    }
    values.iter().map(round).collect()
}

fn query_proof(files: BTreeMap<String, Vec<u8>>) -> Vec<serde_json::Value> {
    let temp = tempfile::tempdir().unwrap();
    for (relative, bytes) in files {
        std::fs::write(temp.path().join(relative), bytes).unwrap();
    }
    crate::export_jobs::pdf::query(
        temp.path(),
        "document.typ",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        "metadata",
        "value",
    )
    .unwrap()
}

#[test]
fn running_headers_keep_the_last_chapter_on_each_page_without_visiting_every_chapter() {
    let mut theme = theme();
    theme.page.header.left.clear();
    theme.page.header.center.clear();
    theme.page.header.right = vec![super::RunningPart::ChapterTitle];
    theme.page.header.show_on_first_page = true;
    let mut files = bundle(&theme).files;
    let runtime = String::from_utf8(files["theme-values.typ"].clone()).unwrap();
    // Compare the bounded implementation against the simple page-based
    // definition, including chapters below the header and on the same page.
    let runtime = runtime.replace("  if title != none { title }", r#"
  let expected = query(<bluink-chapter>).filter(it => it.location().page() <= current).at(-1, default: none)
  assert(title == if expected == none { none } else { expected.value })
  metadata((kind: "chapter-header-proof", page: current, title: title))
  if title != none { title }
"#);
    files.insert("theme-values.typ".into(), runtime.into_bytes());
    files.insert(
        "content.typ".into(),
        br#"
#import "theme-values.typ": *
Title page without a chapter.
#for i in range(150) { metadata((unrelated: i)) }
#pagebreak()
#publication-chapter("First", first: true)[First chapter.]
#publication-chapter("Second", first: true)[Second chapter on this page.]
#pagebreak()
Second chapter continues without a marker.
#pagebreak()
#publication-chapter("Third", first: true)[Third chapter.]
#publication-chapter("Fourth", first: true)[Fourth chapter on this page.]
#pagebreak()
Fourth chapter continues.
"#
        .to_vec(),
    );
    let values = query_proof(files);
    let headers = values
        .iter()
        .filter(|value| value["kind"] == "chapter-header-proof")
        .map(|value| (value["page"].as_u64().unwrap(), value["title"].clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        headers,
        BTreeMap::from([
            (1, serde_json::Value::Null),
            (2, json!("Second")),
            (3, json!("Second")),
            (4, json!("Fourth")),
            (5, json!("Fourth")),
        ])
    );
}

#[test]
fn native_link_styling_inherits_text_and_applies_explicit_theme_emphasis() {
    for emphasized in [false, true] {
        let mut theme = theme();
        let link = theme.styles.get_mut(&Role::Link).unwrap();
        link.font_weight = if emphasized {
            super::Weight::Bold
        } else {
            super::Weight::Regular
        };
        link.italic = emphasized;
        let mut files = bundle(&theme).files;
        files.insert("content.typ".into(), br#"
#import "theme-values.typ": *
#let probe(name) = context metadata((kind: "link-proof", name: name, size: text.size.pt(), weight: text.weight, style: text.style, font: text.font, fill: text.fill.to-hex()))
#text(size: 9pt, font: "Source Serif 4")[#link("https://example.com")[Small link#probe("plain")]]
#text(size: 17pt, weight: 700, style: "italic")[#link("https://example.com")[Emphasized link#probe("emphasized")]]
"#.to_vec());
        let values = query_proof(files);
        let plain = values
            .iter()
            .find(|value| value["name"] == "plain")
            .unwrap();
        assert_eq!(plain["size"], 9.0);
        assert_eq!(plain["weight"], if emphasized { "bold" } else { "regular" });
        assert_eq!(plain["style"], if emphasized { "italic" } else { "normal" });
        assert!(
            plain["font"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case("Source Serif 4")
        );
        assert_eq!(plain["fill"], theme.styles[&Role::Link].color);
        let inherited = values
            .iter()
            .find(|value| value["name"] == "emphasized")
            .unwrap();
        assert_eq!(inherited["size"], 17.0);
        assert_eq!(inherited["weight"], "bold");
        assert_eq!(inherited["style"], "italic");
    }
}

fn heading_proof_source(framed: bool) -> String {
    use crate::domain::publication::typst::{HeadingOptions, heading};
    let mut source = String::from("#import \"theme-values.typ\": *\n");
    for level in 1..=7 {
        let body = format!(
            r#"Heading {level}#context metadata((kind: "heading-proof", level: {level}, size: text.size.pt(), font: text.font, weight: text.weight, style: text.style, color: text.fill.to-hex(), position: here().position()))"#
        );
        let anchor = format!("heading-{level}");
        if framed {
            source.push_str(&heading(
                level,
                &body,
                HeadingOptions {
                    anchor: Some(&anchor),
                    ..HeadingOptions::default()
                },
            ));
        } else {
            writeln!(
                source,
                "#heading(level: {level})[{body}]#label(\"{anchor}\")\n"
            )
            .unwrap();
        }
        source.push_str("#publication-par[Following prose verifies spacing and keep-with-next.]\n");
    }
    source.push_str(
        "#context metadata((kind: \"levels-proof\", levels: query(heading).map(it => it.level)))\n",
    );
    source
}

#[test]
fn explicit_headings_preserve_decorated_theme_geometry_and_semantic_levels() {
    let mut theme = theme();
    for role in [
        Role::Heading1,
        Role::Heading2,
        Role::Heading3,
        Role::Heading4,
        Role::Heading5,
        Role::Heading6,
    ] {
        let style = theme.styles.get_mut(&role).unwrap();
        style.font_weight = super::Weight::Regular;
        style.italic = true;
        style.color = "#345678".into();
        style.background = Some("#f2f4f8".into());
        style.border_width_pt = 1.0;
        style.padding_pt = 4.0;
        style.indent_pt = 8.0;
        style.alignment = super::Alignment::End;
        style.keep_together = true;
        style.keep_with_next = true;
    }
    let mut current = bundle(&theme).files;
    current.insert(
        "content.typ".into(),
        heading_proof_source(true).into_bytes(),
    );
    let mut previous = current.clone();
    previous.insert(
        "content.typ".into(),
        heading_proof_source(false).into_bytes(),
    );
    // The previous transformation is an independent layout oracle for
    // typography, frames, indentation and spacing under authored themes.
    let runtime = String::from_utf8(previous["theme-values.typ"].clone()).unwrap()
        .replace("  publication-heading-styles(body)", "  body")
        .replace("  show link: set text(..link-text)", "  show link: set text(..link-text)\n  show heading: it => publication-block(\"heading\" + str(calc.min(it.level, 6)), it)");
    previous.insert("theme-values.typ".into(), runtime.into_bytes());
    let actual = query_proof(current);
    // Both layouts must agree; sizes may differ in the last bit of a float.
    assert_eq!(rounded(&actual), rounded(&query_proof(previous)));
    let headings = actual
        .iter()
        .filter(|value| value["kind"] == "heading-proof")
        .collect::<Vec<_>>();
    assert_eq!(headings.len(), 7);
    for proof in &headings {
        assert_eq!(proof["weight"], "regular");
        assert_eq!(proof["style"], "italic");
        assert_eq!(proof["color"], "#345678");
    }
    assert_eq!(headings[5]["size"], headings[6]["size"]);
    assert_eq!(
        actual
            .iter()
            .find(|value| value["kind"] == "levels-proof")
            .unwrap()["levels"],
        json!([1, 2, 3, 4, 5, 6, 7])
    );
}

#[test]
fn portable_shell_and_every_role_compile_with_only_packaged_fonts_and_real_line_height() {
    let temp = tempfile::tempdir().unwrap();
    let font_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf");
    let theme = theme();
    let bundle = bundle(&theme);
    publish_pdf(
        bundle.clone(),
        PdfProfile::Default,
        &temp.path().join("proof.pdf"),
        &font_path,
    )
    .unwrap();
    for (relative, bytes) in bundle.files {
        std::fs::write(temp.path().join(relative), bytes).unwrap();
    }
    let values = crate::export_jobs::pdf::query(
        temp.path(),
        "document.typ",
        &font_path,
        "metadata",
        "value",
    )
    .unwrap();
    let proof = values
        .iter()
        .find(|value| value["kind"] == "style-proof")
        .expect("Rendered proof metadata");
    assert!(proof["pages"].as_u64().unwrap() >= 2);
    let baseline = proof["baseline"].as_f64().unwrap();
    assert!(
        (baseline - 18.0).abs() < 0.001,
        "Expected 12pt × 1.5 = 18pt; got {baseline}"
    );
    let heading = values
        .iter()
        .find(|value| value["kind"] == "heading-style")
        .expect("Rendered heading metadata");
    assert_eq!(heading["size"], theme.styles[&Role::Heading1].font_size_pt);
    let tables = crate::export_jobs::pdf::query(
        temp.path(),
        "document.typ",
        &font_path,
        "table",
        "children",
    )
    .unwrap();
    let cells = tables[0].as_array().unwrap();
    assert_eq!(cells[0]["func"], "header");
    assert_eq!(cells[0]["children"][0]["x"], 0);
    assert_eq!(cells[1]["rowspan"], 2);
    assert_eq!(cells[2]["colspan"], 2);
    assert_eq!(cells[3]["x"], 1);
    assert_eq!(cells[3]["y"], 2);
}

#[test]
#[ignore = "Explicit developer command writes review artifacts to the ignored Rust target directory."]
fn write_style_proofs() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/publication-style-proof");
    std::fs::create_dir_all(&directory).unwrap();
    let theme = theme();
    let bundle = bundle(&theme);
    for (relative, bytes) in &bundle.files {
        std::fs::write(directory.join(relative), bytes).unwrap();
    }
    let destination = directory.join("proof.pdf");
    if destination.exists() {
        std::fs::remove_file(&destination).unwrap();
    }
    publish_pdf(
        bundle,
        PdfProfile::Default,
        &destination,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
    )
    .unwrap();
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Portable theme proof</title><style>{}</style></head><body><main class=\"publication-content\">{}</main></body></html>",
        css::stylesheet(&theme, "Portable theme proof").unwrap(),
        include_str!("../../../../test-fixtures/publication/full-export.html")
    );
    let html = crate::export_jobs::fonts::embed_html_fonts(
        &html,
        theme.fonts.iter().map(String::as_str),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web"),
    )
    .unwrap();
    std::fs::create_dir_all(directory.join("assets")).unwrap();
    std::fs::write(
        directory.join("assets/pixel.png"),
        include_bytes!("../../../../test-fixtures/one-pixel.png"),
    )
    .unwrap();
    std::fs::write(directory.join("proof.html"), html).unwrap();
}
