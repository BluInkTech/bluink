// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! PDF and Word place notes the same way: footnotes per chapter, citations
//! as numbered markers, and each source listed once where the theme says.

use super::plan::plan_publication;
use crate::application::item_writes::write_item_input;
use crate::domain::project::config::create_project_config;
use crate::domain::publication::plan::PublicationFormat;
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;

/// One chapter's notes: its footnotes, its citation markers, and the numbers
/// of the sources listed in it.
#[derive(Debug, PartialEq, Eq)]
struct Chapter {
    footnotes: usize,
    markers: Vec<Vec<usize>>,
    listed: Vec<usize>,
}

fn numbers(text: &str) -> Vec<usize> {
    text.split(',')
        .map(|number| number.trim().parse().unwrap())
        .collect()
}

/// Chapters of the Typst source: each page is its own block.
fn typst_chapters(source: &str) -> Vec<Chapter> {
    let marker = Regex::new(r#"\\\[((?:#link\(label\("[^"]+"\)\)\[\d+\](?:, )?)+)\\\]"#).unwrap();
    let number = Regex::new(r#"#link\(label\("[^"]+"\)\)\[(\d+)\]"#).unwrap();
    let item = Regex::new(r#"#publication-reference-item\("(\d+)""#).unwrap();
    source
        .split("#[\n#counter(footnote).update(0)")
        .skip(1)
        .map(|chapter| Chapter {
            footnotes: chapter.matches("#footnote[").count(),
            markers: marker
                .captures_iter(chapter)
                .map(|group| {
                    number
                        .captures_iter(&group[1])
                        .map(|found| found[1].parse().unwrap())
                        .collect()
                })
                .collect(),
            listed: item
                .captures_iter(chapter)
                .map(|found| found[1].parse().unwrap())
                .collect(),
        })
        .collect()
}

/// Chapters of the Word document: each page is its own section.
fn word_chapters(document: &str) -> Vec<Chapter> {
    let text = Regex::new(r"<w:t(?: [^>]*)?>([^<]*)</w:t>").unwrap();
    let entry = Regex::new(r"^\[(\d+)\] ").unwrap();
    let marker = Regex::new(r"\[(\d+(?:, \d+)*)\]").unwrap();
    let sections: Vec<&str> = document.split("<w:sectPr>").collect();
    sections[..sections.len() - 1]
        .iter()
        .map(|section| {
            let mut chapter = Chapter {
                footnotes: section.matches("<w:footnoteReference").count(),
                markers: Vec::new(),
                listed: Vec::new(),
            };
            for paragraph in section.split("</w:p>") {
                let words: String = text
                    .captures_iter(paragraph)
                    .map(|found| found[1].to_owned())
                    .collect();
                if let Some(found) = entry.captures(&words) {
                    chapter.listed.push(found[1].parse().unwrap());
                } else {
                    chapter
                        .markers
                        .extend(marker.captures_iter(&words).map(|found| numbers(&found[1])));
                }
            }
            chapter
        })
        .collect()
}

fn book(overrides: Option<Value>) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
    let config: Value =
        serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
    create_project_config(&root, config).unwrap();
    crate::domain::project::properties::initialize(&root).unwrap();
    crate::application::snippets::write_snippet(
        &root,
        "shared",
        json!({"type":"paragraph","content":[{"type":"text","text":"A shared source"}]}),
        None,
    )
    .unwrap();
    let text = |text: &str| json!({"type":"text","text":text});
    let footnote = |text: &str| json!({"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":text}]}}});
    let inline = |text: &str| json!({"type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{"type":"text","text":text}]}}});
    let shared = json!({"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}});
    let pages = [
        vec![
            text("Start."),
            footnote("An aside"),
            text(" Claim"),
            inline("Alpha"),
            shared.clone(),
        ],
        vec![
            text("Middle"),
            shared,
            text(" with notes."),
            footnote("One"),
            footnote("Two"),
        ],
        vec![text("End"), inline("Gamma")],
    ];
    let ids: Vec<String> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
    for (index, content) in pages.into_iter().enumerate() {
        write_item_input(
            &root,
            json!({"id":ids[index],"meta":{"type":"page","title":format!("Page {index}"),"lang":"en"},
                "content":{"document":{"type":"doc","content":[{"type":"paragraph","content":content}]}}}),
            None,
            "2026-10-28",
        )
        .unwrap();
    }
    let mut content = json!({"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],
        "outline":[{"kind":"page","pageId":ids[1],"children":[]},{"kind":"page","pageId":ids[2],"children":[]}],
        "selectionRules":[],"themeId":"classic",
        "targets":{"pdf":{"profile":"default","outsideLinks":"text-only"},
            "html":{"outsideLinks":"text-only"},"docx":{"outsideLinks":"text-only"},"epub":{"outsideLinks":"text-only"},
            "site":{"layout":"documentation","outsideLinks":"text-only"}}});
    if let Some(overrides) = overrides {
        content["themeOverrides"] = overrides;
    }
    write_item_input(
        &root,
        json!({"id":ids[3],"meta":{"type":"publication","title":"Notes"},"content":content}),
        None,
        "2026-10-28",
    )
    .unwrap();
    (temporary, root, ids[3].clone())
}

fn both(root: &Path, publication: &str) -> (Vec<Chapter>, Vec<Chapter>) {
    let plan = plan_publication(root, publication, PublicationFormat::Pdf).unwrap();
    let bundle = super::pdf::render_publication_pdf(root, &plan).unwrap();
    let typst = typst_chapters(std::str::from_utf8(&bundle.files["content.typ"]).unwrap());
    let plan = plan_publication(root, publication, PublicationFormat::Docx).unwrap();
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/docx");
    let bundle = super::word::render_publication(root, &plan, &fonts, None).unwrap();
    let parts = super::docx::tests::parts(&bundle.bytes);
    let word = word_chapters(std::str::from_utf8(&parts["word/document.xml"]).unwrap());
    (typst, word)
}

fn chapter(footnotes: usize, markers: &[&[usize]], listed: &[usize]) -> Chapter {
    Chapter {
        footnotes,
        markers: markers.iter().map(|group| group.to_vec()).collect(),
        listed: listed.to_vec(),
    }
}

#[test]
fn pdf_and_word_list_each_chapters_sources_at_its_end() {
    let (_temporary, root, publication) = book(None);
    let (typst, word) = both(&root, &publication);
    let expected = vec![
        // Adjacent citations share one marker; footnotes keep their own count.
        chapter(1, &[&[1, 2]], &[1, 2]),
        // Each chapter numbers its sources from one, the shared one included.
        chapter(2, &[&[1]], &[1]),
        chapter(0, &[&[1]], &[1]),
    ];
    assert_eq!(typst, expected);
    assert_eq!(word, expected);
}

#[test]
fn pdf_and_word_list_a_books_sources_once_at_its_end() {
    let (_temporary, root, publication) = book(Some(
        json!({"shared":{"page":{"referencesAt":"book"}},"targets":{}}),
    ));
    let (typst, word) = both(&root, &publication);
    let expected = vec![
        chapter(1, &[&[1, 2]], &[]),
        // The shared source keeps the number it was first given.
        chapter(2, &[&[2]], &[]),
        // The book's list closes it, after the last chapter.
        chapter(0, &[&[3]], &[1, 2, 3]),
    ];
    assert_eq!(typst, expected);
    assert_eq!(word, expected);
}
