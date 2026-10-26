// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use quick_xml::{Reader, events::Event};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::PathBuf,
};

fn fonts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets")
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut result = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if index == 0 {
            assert_eq!(file.name(), "mimetype");
            assert_eq!(file.compression(), zip::CompressionMethod::Stored);
            assert!(file.extra_data().is_none_or(<[u8]>::is_empty));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        result.insert(file.name().into(), bytes);
    }
    assert_eq!(result["mimetype"], b"application/epub+zip");
    for (path, bytes) in &result {
        if ["xml", "xhtml", "opf"].iter().any(|ext| {
            Path::new(path)
                .extension()
                .is_some_and(|value| value == *ext)
        }) {
            let mut reader = Reader::from_reader(bytes.as_slice());
            let mut depth = 0;
            loop {
                match reader
                    .read_event()
                    .unwrap_or_else(|error| panic!("{path}: {error}"))
                {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => {
                        assert!(depth > 0);
                        depth -= 1;
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
            assert_eq!(depth, 0, "{path}");
        }
    }
    result
}

fn corpus() -> EpubBundle {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let theme =
        super::super::theme::capture_theme(root, "classic", PublicationFormat::Epub).unwrap();
    let snippets = [PlannedSnippet {
        id: "source_one".into(),
        revision: "test".into(),
        paragraph: json!({"type":"paragraph","content":[{"type":"text","text":"Shared source"}]}),
    }];
    let mut book = Book::new(
        root,
        &theme.resolved,
        &[],
        &snippets,
        &fonts().join("web"),
        None,
    )
    .unwrap();
    let doc: Value = serde_json::from_str(include_str!(
        "../../../../../test-fixtures/publication/full-export.json"
    ))
    .unwrap();
    let links = HashMap::from([
        (
            "https://example.com/?a=1&b=2".into(),
            "https://example.com/?a=1&b=2".into(),
        ),
        (
            "https://example.com/source".into(),
            "https://example.com/source".into(),
        ),
        (
            "/?note=01a0e2a0-0000-7000-8000-000000000001".into(),
            "second.xhtml".into(),
        ),
        ("#heading-one".into(), "#block-heading-one".into()),
    ]);
    let assets = HashMap::from([("/assets/test/pixel.png".into(), "../media/pixel.png".into())]);
    book.package
        .add(
            "media/pixel.png",
            "image/png",
            include_bytes!("../../../../../test-fixtures/one-pixel.png").to_vec(),
        )
        .unwrap();
    let fields = HashMap::from([("author".into(), "Writer & editor".into())]);
    let exclusions = HashSet::from(["excluded-paragraph".into()]);
    let body = html::render_xhtml_document(
        &doc,
        &HtmlResources {
            page_id: "corpus",
            links: &links,
            assets: &assets,
            fields: &fields,
            snippets: &book.snippets,
            excluded_block_ids: &exclusions,
        },
        1,
    )
    .unwrap();
    for (path, title, language, depth) in [
        ("first.xhtml", "First & <chapter>", "en-GB", 1),
        ("second.xhtml", "Deuxième", "fr", 2),
    ] {
        book.chapter(
            Chapter {
                path: format!("chapters/{path}"),
                title: title.into(),
                depth,
            },
            language,
            &body,
        )
        .unwrap();
    }
    book.finish(
        "00000000-0000-7000-8000-000000000001",
        "Corpus & proof",
        "en-GB",
    )
    .unwrap()
}

#[test]
fn complete_corpus_preserves_content_and_produces_well_formed_reflowable_package() {
    let bundle = corpus();
    let files = parts(&bundle.bytes);
    let first = String::from_utf8(files["EPUB/chapters/first.xhtml"].clone()).unwrap();
    for value in [
        "epub:type=\"noteref\"",
        "epub:type=\"footnote\"",
        "second.xhtml",
        "Writer &amp; editor",
        "<br />",
        "<col ",
        "Shared source",
    ] {
        assert!(first.contains(value), "{value}");
    }
    assert!(!first.contains("Excluded text"));
    assert_eq!(first.matches("role=\"doc-footnote\"").count(), 3);
    assert!(files.keys().any(|path| {
        path.starts_with("EPUB/fonts/")
            && Path::new(path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("woff2"))
    }));
    assert!(
        files
            .keys()
            .any(|path| path.starts_with("EPUB/fonts/licenses/"))
    );
    let nav = String::from_utf8(files["EPUB/nav.xhtml"].clone()).unwrap();
    assert!(nav.contains("chapters/first.xhtml\">First &amp; &lt;chapter&gt;</a><ol><li>"));
    let css = String::from_utf8(files["EPUB/styles.css"].clone()).unwrap();
    assert!(!css.contains(":root") && !css.contains("@page") && !css.contains("var(--"));
    assert!(css.contains("rem") && css.contains("pre-wrap"));
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("proof.epub");
    publish(&bundle, &output).unwrap();
    assert!(publish(&bundle, &output).is_err());
    assert!(publish(&bundle, &dir.path().join("proof.zip")).is_err());
    assert_eq!(std::fs::read(output).unwrap(), bundle.bytes);
}

#[test]
fn publication_reading_order_resources_and_cross_chapter_links_share_the_plan() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let plan = super::super::plan::plan_publication(
        &root,
        "01a0e7f5-d342-7f23-bd3d-6e27064a16cd",
        PublicationFormat::Epub,
    )
    .unwrap();
    let bundle = render_publication(
        &root,
        &plan,
        &fonts().join("web"),
        Some(&fonts().join("pdf")),
    )
    .unwrap();
    let files = parts(&bundle.bytes);
    for page in &plan.pages {
        assert!(files.contains_key(&format!("EPUB/chapters/{}.xhtml", page.id)));
    }
    for page in &plan.pages {
        for target in chapter_links(&plan.links, &page.id).unwrap().values() {
            if !target.is_empty() && !target.contains(':') {
                let path = target.split('#').next().unwrap();
                assert!(
                    files.contains_key(&format!("EPUB/chapters/{path}")),
                    "{target}"
                );
            }
        }
    }
    let opf = String::from_utf8(files["EPUB/book.opf"].clone()).unwrap();
    assert_eq!(
        opf.matches("<itemref ").count(),
        entries(&plan).unwrap().len()
    );
    assert!(opf.contains("properties=\"nav\"") && opf.contains("dc:language"));
    let mut bad = plan;
    bad.format = PublicationFormat::Html;
    assert!(render_publication(&root, &bad, &fonts().join("web"), None).is_err());
}

#[test]
fn package_and_xhtml_reject_unsafe_inputs() {
    let mut package = Package::default();
    assert!(package.add("../escape", "text/plain", vec![]).is_err());
    assert!(package.add("chapters/a#b.xhtml", XHTML, vec![]).is_err());
    package.add("safe.txt", "text/plain", vec![1]).unwrap();
    assert!(package.add("safe.txt", "text/plain", vec![2]).is_err());
    assert!(package.finish("id", "Title", "en", &[]).is_err());
    let empty = HashMap::new();
    let snippets = HashMap::new();
    let excluded = HashSet::new();
    let doc = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"bad\u{0}text"}]}]});
    assert!(
        html::render_xhtml_document(
            &doc,
            &HtmlResources {
                page_id: "test",
                links: &empty,
                assets: &empty,
                fields: &empty,
                snippets: &snippets,
                excluded_block_ids: &excluded
            },
            0
        )
        .is_err()
    );
}

#[test]
#[ignore = "Writes an EPUB proof for EPUBCheck and reader inspection when explicitly requested."]
fn write_epub_proof() {
    let output =
        std::env::var_os("BLUINK_EPUB_PROOF").expect("Set BLUINK_EPUB_PROOF to a new .epub path");
    publish(&corpus(), Path::new(&output)).unwrap();
}
