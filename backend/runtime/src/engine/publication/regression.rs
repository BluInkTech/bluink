// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! The same portable corpus protects semantics while exporters move to shared traversal.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::html::{HtmlResources, render_html_document};
use super::typst::{Destination, TypstResources, render_typst_document};

fn output() -> (String, String) {
    let document: Value = serde_json::from_str(include_str!(
        "../../../../test-fixtures/publication/full-export.json"
    ))
    .unwrap();
    let links = HashMap::from([
        (
            "https://example.com/?a=1&b=2".to_owned(),
            "https://example.com/?a=1&b=2".to_owned(),
        ),
        (
            "https://example.com/source".to_owned(),
            "https://example.com/source".to_owned(),
        ),
        (
            "/?note=01a0e2a0-0000-7000-8000-000000000001".to_owned(),
            "chapter-two.html".to_owned(),
        ),
        ("#heading-one".to_owned(), "#heading-one".to_owned()),
    ]);
    let assets = HashMap::from([(
        "/assets/test/pixel.png".to_owned(),
        "assets/pixel.png".to_owned(),
    )]);
    let fields = HashMap::from([("author".to_owned(), "Writer & editor".to_owned())]);
    let snippets = HashMap::from([(
        "source_one".to_owned(),
        json!({"type":"paragraph","content":[
            {"type":"text","text":"Shared source","marks":[{"type":"strong"}]},
            {"type":"text","text":" online","marks":[{"type":"link","attrs":{"href":"https://example.com/source"}}]}
        ]}),
    )]);
    let excluded = HashSet::from(["excluded-paragraph".to_owned()]);
    let html = render_html_document(
        &document,
        &HtmlResources {
            page_id: "test-page",
            links: &links,
            assets: &assets,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &excluded,
        },
        0,
    )
    .unwrap();
    let mut typst_links = links
        .into_iter()
        .map(|(key, destination)| (key, Destination::Url(destination)))
        .collect::<HashMap<_, _>>();
    typst_links.insert(
        "/?note=01a0e2a0-0000-7000-8000-000000000001".into(),
        Destination::Anchor("chapter-two".into()),
    );
    typst_links.insert(
        "#heading-one".into(),
        Destination::Anchor("block-heading-one".into()),
    );
    let typst = render_typst_document(
        &document,
        &TypstResources {
            page_id: "test-page",
            links: &typst_links,
            assets: &assets,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &excluded,
        },
        0,
    )
    .unwrap();
    (html, typst)
}

#[test]
fn complete_export_corpus_matches_reviewed_baselines() {
    let (html, typst) = output();
    assert_eq!(
        html,
        include_str!("../../../../test-fixtures/publication/full-export.html")
    );
    assert_eq!(
        typst.trim_end(),
        include_str!("../../../../test-fixtures/publication/full-export.typ").trim_end()
    );
    assert!(!html.contains("Excluded text"));
    assert!(!typst.contains("Excluded text"));
    assert_eq!(html.matches("role=\"doc-footnote\"").count(), 3);
    assert_eq!(typst.matches("#footnote[").count(), 3);
}

#[test]
fn complete_corpus_compiles_with_native_pdf_helpers_and_packaged_fonts() {
    use super::pdf::{PdfBundle, publish_pdf};
    use super::theme_format::{self, Definition, Overrides};
    use std::collections::BTreeMap;

    let (_, source) = output();
    let definition: Definition = serde_json::from_str(include_str!(
        "../../../../vendor/themes/classic/manifest.json"
    ))
    .unwrap();
    let theme = theme_format::resolve(&definition, "pdf", &Overrides::default()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let bundle = PdfBundle {
        shell_path: "document.typ".into(), page_lines: vec![(1,"test-page".into())],
        files: BTreeMap::from([
            ("document.typ".into(), theme_format::typst::document_shell("Full export proof", "en").unwrap().into_bytes()),
            ("theme-values.typ".into(), theme_format::typst::values(&theme).unwrap().into_bytes()),
            ("content.typ".into(), format!("#import \"theme-values.typ\": *\n#publication-anchor(\"chapter-two\")\n{source}").into_bytes()),
            ("assets/pixel.png".into(), include_bytes!("../../../../test-fixtures/one-pixel.png").to_vec()),
        ]),
    };
    let platform = if cfg!(target_os = "windows") {
        "windows-x64/typst.exe"
    } else if cfg!(target_os = "linux") {
        "linux-x64/typst"
    } else {
        "macos-arm64/typst"
    };
    let binary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../typst/assets")
        .join(platform);
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf");
    publish_pdf(
        &bundle,
        super::PdfProfile::Default,
        &temp.path().join("full.pdf"),
        &binary,
        &fonts,
    )
    .unwrap();
    for (relative, bytes) in &bundle.files {
        let path = temp.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let query = std::process::Command::new(&binary)
        .args([
            "query",
            "--features",
            "a11y-extras",
            "--ignore-system-fonts",
            "--font-path",
        ])
        .arg(&fonts)
        .arg(temp.path().join("document.typ"))
        .args(["table", "--field", "children"])
        .output()
        .unwrap();
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    let table: Value = serde_json::from_slice(&query.stdout).unwrap();
    let table = table.as_array().unwrap().first().unwrap();
    let cells = table.as_array().unwrap();
    assert!(cells.iter().any(|cell| cell["rowspan"] == 2));
    assert!(cells.iter().any(|cell| cell["colspan"] == 2));
}

#[test]
#[ignore = "Explicit command updates baselines for review; never run as an automatic test fix."]
fn regenerate_export_baselines() {
    let (html, typst) = output();
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/publication");
    std::fs::write(directory.join("full-export.html"), html).unwrap();
    std::fs::write(
        directory.join("full-export.typ"),
        format!("{}\n", typst.trim_end()),
    )
    .unwrap();
}
