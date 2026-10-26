// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::{Overrides, ResolvedTheme, Role, css, resolve, typst};
use crate::engine::publication::PdfProfile;
use crate::engine::publication::pdf::{PdfBundle, publish_pdf};

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
    assert!(
        typst::document_shell(title, "en")
            .unwrap()
            .contains("\\n#panic(\\\"bad\\\")")
    );
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
    let html = crate::engine::publication::fonts::embed_html_fonts(
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
        Role::BoxInfo,
        Role::BoxAbstract,
        Role::BoxNote,
        Role::BoxTip,
        Role::BoxSuccess,
        Role::BoxImportant,
        Role::BoxWarning,
        Role::BoxError,
        Role::BoxQuestion,
        Role::BoxExample,
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
  #publication-par[The final chapter uses the same theme.]
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
                typst::document_shell("Portable <theme> proof", "en")
                    .unwrap()
                    .into_bytes(),
            ),
            (
                "theme-values.typ".into(),
                typst::values(theme).unwrap().into_bytes(),
            ),
            ("content.typ".into(), sample().into_bytes()),
            (
                "pixel.png".into(),
                include_bytes!("../../../../../test-fixtures/one-pixel.png").to_vec(),
            ),
        ]),
    }
}

fn binary() -> PathBuf {
    let platform = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64/typst.exe"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64/typst"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64/typst"
    } else {
        panic!("No packaged Typst platform for this test host")
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../typst/assets")
        .join(platform)
}

#[test]
fn portable_shell_and_every_role_compile_with_only_packaged_fonts_and_real_line_height() {
    let temp = tempfile::tempdir().unwrap();
    let font_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf");
    let theme = theme();
    let bundle = bundle(&theme);
    publish_pdf(
        &bundle,
        PdfProfile::Default,
        &temp.path().join("proof.pdf"),
        &binary(),
        &font_path,
    )
    .unwrap();
    for (relative, bytes) in bundle.files {
        std::fs::write(temp.path().join(relative), bytes).unwrap();
    }
    let query = std::process::Command::new(binary())
        .args([
            "query",
            "--features",
            "a11y-extras",
            "--ignore-system-fonts",
            "--font-path",
        ])
        .arg(&font_path)
        .arg(temp.path().join("document.typ"))
        .args(["metadata", "--field", "value"])
        .output()
        .unwrap();
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    let values: Vec<serde_json::Value> = serde_json::from_slice(&query.stdout).unwrap();
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
    let query = std::process::Command::new(binary())
        .args([
            "query",
            "--features",
            "a11y-extras",
            "--ignore-system-fonts",
            "--font-path",
        ])
        .arg(&font_path)
        .arg(temp.path().join("document.typ"))
        .args(["table", "--field", "children"])
        .output()
        .unwrap();
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    let tables: Vec<serde_json::Value> = serde_json::from_slice(&query.stdout).unwrap();
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
        &bundle,
        PdfProfile::Default,
        &destination,
        &binary(),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
    )
    .unwrap();
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Portable theme proof</title><style>{}</style></head><body><main class=\"publication-content\">{}</main></body></html>",
        css::stylesheet(&theme, "Portable theme proof").unwrap(),
        include_str!("../../../../../test-fixtures/publication/full-export.html")
    );
    let html = crate::engine::publication::fonts::embed_html_fonts(
        &html,
        theme.fonts.iter().map(String::as_str),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/web"),
    )
    .unwrap();
    std::fs::create_dir_all(directory.join("assets")).unwrap();
    std::fs::write(
        directory.join("assets/pixel.png"),
        include_bytes!("../../../../../test-fixtures/one-pixel.png"),
    )
    .unwrap();
    std::fs::write(directory.join("proof.html"), html).unwrap();
}
