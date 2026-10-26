// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::tests::{parts, tags, theme};
use super::*;
use crate::engine::publication::render::{self, ReferenceScope, Resources};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../test-fixtures/publication/full-export.json"
    ))
    .unwrap()
}

fn prepare(source: &Value, scope: &mut ReferenceScope) -> render::PreparedPage {
    let links = HashMap::from([
        (
            "https://example.com/?a=1&b=2".into(),
            Destination::Url("https://example.com/?a=1&b=2".into()),
        ),
        (
            "https://example.com/source".into(),
            Destination::Url("https://example.com/source".into()),
        ),
        (
            "/?note=01a0e2a0-0000-7000-8000-000000000001".into(),
            Destination::Anchor("chapter-two".into()),
        ),
        (
            "#heading-one".into(),
            Destination::Anchor("block-heading-one".into()),
        ),
    ]);
    let assets = HashMap::from([
        ("/assets/test/pixel.png".into(), "assets/pixel.png".into()),
        (
            "/assets/test/diagram.svg".into(),
            "assets/diagram.svg".into(),
        ),
    ]);
    let fields = HashMap::from([("author".into(), "Writer & editor".into())]);
    let snippets = HashMap::from([(
        "source_one".into(),
        json!({"type":"paragraph","content":[{"type":"text","text":"Shared source","marks":[{"type":"strong"}]},{"type":"text","text":" online","marks":[{"type":"link","attrs":{"href":"https://example.com/source"}}]}]}),
    )]);
    render::prepare_page(
        "full-corpus",
        source,
        &Resources {
            links: &links,
            assets: &assets,
            fields: &fields,
            snippets: &snippets,
            excluded_block_ids: &HashSet::from(["excluded-paragraph".into()]),
        },
        0,
        scope,
    )
    .unwrap()
}

fn full(theme: &ResolvedTheme) -> DocxBundle {
    let mut writer = DocxDocument::new(
        theme,
        "Complete DOCX export proof",
        "en-GB",
        &super::tests::font_folder(),
    )
    .unwrap();
    writer
        .add_image_resource(
            "assets/pixel.png",
            include_bytes!("../../../../../test-fixtures/one-pixel.png").to_vec(),
            None,
        )
        .unwrap();
    writer.add_image_resource("assets/diagram.svg",include_bytes!("../../../../../test-fixtures/pdf-project/items/01/01a0e2a0-0000-7000-8000-000000000002/assets/diagram.svg").to_vec(),None).unwrap();
    let mut scope = ReferenceScope::default();
    writer
        .chapter("Full export corpus", 1, "corpus", true)
        .unwrap();
    render::walk(&prepare(&corpus(), &mut scope), &mut writer).unwrap();
    writer.contents_page().unwrap();
    writer
        .chapter("Second chapter", 1, "chapter-two", false)
        .unwrap();
    let second = json!({"type":"doc","content":[
        {"type":"paragraph","content":[{"type":"text","text":"The same citation is reused across chapters "},{"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}}]},
        {"type":"figure","content":[{"type":"blockTitle","content":[{"type":"text","text":"Offline vector diagram"}]},{"type":"image","attrs":{"src":"/assets/test/diagram.svg","alt":"A blue circle on a pale rectangle","width":128,"height":64}}]}
    ]});
    render::walk(&prepare(&second, &mut scope), &mut writer).unwrap();
    writer.finish().unwrap()
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "Keep native full-corpus parity assertions together for review."
)]
fn full_corpus_preserves_native_spans_boxes_lists_images_and_deduplicated_rich_notes() {
    let files = parts(&full(&theme("classic")).bytes);
    let document = tags(&files[MAIN]);
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "w:footnoteReference")
            .count(),
        3
    );
    assert_eq!(
        document
            .iter()
            .filter(|(name, attrs)| name == "w:fldSimple" && attrs["w:instr"].contains("NOTEREF"))
            .count(),
        2
    );
    assert_eq!(
        document
            .iter()
            .filter(|(name, attrs)| name == "w:vMerge" && attrs["w:val"] == "restart")
            .count(),
        1
    );
    assert_eq!(
        document
            .iter()
            .filter(|(name, attrs)| name == "w:vMerge" && attrs["w:val"] == "continue")
            .count(),
        1
    );
    assert!(
        document
            .iter()
            .any(|(name, attrs)| name == "w:gridSpan" && attrs["w:val"] == "2")
    );
    for role in [
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
        Role::Quote,
        Role::Embed,
    ] {
        assert!(
            document.iter().any(|(name, attrs)| name == "w:tblCaption"
                && attrs["w:val"] == format!("Bluink {}", role.name())),
            "{}",
            role.name()
        );
    }
    assert!(document.iter().any(|(name, attrs)| name == "wp:extent"
        && attrs["cx"] == "609600"
        && attrs["cy"] == "457200"));
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "w:drawing")
            .count(),
        3
    );
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "asvg:svgBlip")
            .count(),
        1
    );
    let notes = tags(&files[notes::PART]);
    assert_eq!(
        notes
            .iter()
            .filter(|(name, attrs)| name == "w:footnote" && !attrs.contains_key("w:type"))
            .count(),
        3
    );
    assert_eq!(
        notes
            .iter()
            .filter(|(name, _)| name == "w:footnoteRef")
            .count(),
        3
    );
    assert!(files.contains_key("word/_rels/footnotes.xml.rels"));
    let text = String::from_utf8_lossy(&files[MAIN]);
    assert!(!text.contains("Excluded text"));
    assert!(text.contains("Writer &amp; editor"));
    assert!(
        text.contains("Table caption")
            && text.contains("Code caption")
            && text.contains("Image caption")
    );
    assert!(String::from_utf8_lossy(&files[notes::PART]).contains("Rich footnote"));
    assert!(
        tags(&files["word/numbering.xml"])
            .iter()
            .any(|(name, attrs)| name == "w:startOverride" && attrs["w:val"] == "7")
    );
}

#[test]
fn print_parts_have_native_fields_first_page_suppression_and_an_editable_clickable_toc() {
    let files = parts(&full(&theme("classic")).bytes);
    let header = String::from_utf8_lossy(&files["word/header.xml"]);
    assert!(header.contains("Complete DOCX export proof") && header.contains("STYLEREF"));
    assert!(!String::from_utf8_lossy(&files["word/header-first.xml"]).contains("STYLEREF"));
    let footer = String::from_utf8_lossy(&files["word/footer.xml"]);
    assert!(footer.contains(" PAGE ") && footer.contains(" NUMPAGES "));
    let document = tags(&files[MAIN]);
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "w:titlePg")
            .count(),
        1
    );
    assert!(
        document
            .iter()
            .any(|(name, attrs)| name == "w:headerReference" && attrs["w:type"] == "first")
    );
    assert!(
        document
            .iter()
            .any(|(name, attrs)| name == "w:pageBreakBefore" && attrs["w:val"] == "1")
    );
    let text = String::from_utf8_lossy(&files[MAIN]);
    assert!(text.contains(" TOC ") && text.contains(" PAGEREF ") && text.contains("Contents"));
    let text = quick_xml::escape::unescape(&text).unwrap();
    assert!(text.contains(" TOC \\f B \\h \\z "));
    assert!(text.contains("\" \\f B \\l 1 "));
    assert!(text.contains(" REF b_"));
    assert_eq!(text.matches(" TC ").count(), 1);
    let cached_targets: Vec<_> = document
        .iter()
        .filter_map(|(name, attrs)| {
            (name == "w:fldSimple")
                .then(|| attrs.get("w:instr"))
                .flatten()
                .and_then(|instruction| instruction.strip_prefix(" PAGEREF "))
                .and_then(|instruction| instruction.split_whitespace().next())
        })
        .collect();
    assert_ne!(cached_targets, [] as [&str; 0]);
    assert_eq!(cached_targets.len(), 1);
    let contents = text
        .split("<w:sdtContent>")
        .nth(1)
        .unwrap()
        .split("</w:sdtContent>")
        .next()
        .unwrap();
    assert_eq!(contents.matches("<w:p>").count(), 2); // Heading plus entry; no field-only spacer paragraphs.
    assert!(
        cached_targets
            .iter()
            .all(|target| target.starts_with("_Toc"))
    );
    assert!(document.iter().any(|(name, attrs)| {
        name == "w:hyperlink"
            && attrs
                .get("w:anchor")
                .is_some_and(|anchor| anchor.starts_with("b_"))
    }));
    assert!(
        document
            .iter()
            .any(|(name, attrs)| name == "w:hyperlink" && attrs.contains_key("w:anchor"))
    );
}

#[test]
fn contents_reference_title_bookmarks_without_interpolating_authored_field_arguments() {
    let theme = theme("classic");
    let mut writer =
        DocxDocument::new(&theme, "Cover", "en", &super::tests::font_folder()).unwrap();
    writer.chapter("Cover", 1, "cover", true).unwrap();
    writer.contents_page().unwrap();
    writer
        .chapter("2. A \"quoted\" \\ path — 日本語", 3, "chapter", false)
        .unwrap();
    let files = parts(&writer.finish().unwrap().bytes);
    let xml = String::from_utf8_lossy(&files[MAIN]);
    let xml = quick_xml::escape::unescape(&xml).unwrap();
    assert!(xml.contains("2. A \"quoted\" \\ path — 日本語"));
    assert!(xml.contains(" REF b_"));
    assert!(xml.contains("\" \\f B \\l 3 "));
    assert!(!xml.contains(" TC \"2. A"));
    assert_eq!(xml.matches(" TC ").count(), 1);
    assert!(!xml.contains("\\o \"1-9\""));
    assert!(!xml.contains("w:vanish"));
}

#[test]
fn invalid_or_missing_image_resources_fail_without_external_resolution() {
    let theme = theme("classic");
    let mut writer =
        DocxDocument::new(&theme, "Images", "en", &super::tests::font_folder()).unwrap();
    let external =
        br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///private.png"/></svg>"#;
    assert!(
        writer
            .add_image_resource("bad.svg", external.to_vec(), None)
            .unwrap_err()
            .message
            .contains("embedded")
    );
    assert!(
        writer
            .add_image_resource("bad.png", b"not an image".to_vec(), None)
            .is_err()
    );
    let source = json!({"type":"doc","content":[{"type":"image","attrs":{"id":"missing-image","src":"/assets/test/pixel.png","alt":"Missing"}}]});
    let error = render::render(&prepare(&source, &mut ReferenceScope::default()), writer)
        .err()
        .unwrap();
    assert_eq!(error.block_id.as_deref(), Some("missing-image"));
    assert!(error.message.contains("not loaded"));
}

fn paragraph<'a>(xml: &'a str, text: &str) -> &'a str {
    let needle = format!("<w:t xml:space=\"preserve\">{text}</w:t>");
    let start = xml[..xml.find(&needle).unwrap()].rfind("<w:p>").unwrap();
    &xml[start..start + xml[start..].find("</w:p>").unwrap() + 6]
}

#[test]
fn caption_pairing_container_decoration_and_continuous_chapters_are_native() {
    let after = parts(&full(&theme("classic")).bytes);
    let xml = String::from_utf8_lossy(&after[MAIN]);
    assert!(paragraph(&xml, "B").contains("<w:keepNext w:val=\"1\"/>"));
    assert!(paragraph(&xml, "Two rows").contains("<w:keepNext w:val=\"1\"/>"));
    assert!(paragraph(&xml, "Two columns").contains("<w:keepNext w:val=\"1\"/>"));
    assert!(paragraph(&xml, "A").contains("<w:keepNext w:val=\"1\"/>"));
    assert!(!paragraph(&xml, "Table caption").contains("<w:keepNext w:val=\"1\"/>"));
    let quote = paragraph(&xml, "A quoted paragraph.");
    assert!(quote.contains("<w:ind w:left=\"0\" w:right=\"0\"/>"));
    assert!(xml.contains("<w:tcPr><w:tcW") && xml.contains("w:fill=\"f2f4f8\""));

    let mut before = theme("classic");
    before
        .styles
        .get_mut(&Role::Caption)
        .unwrap()
        .caption_position = Some(super::super::theme_format::CaptionPosition::Before);
    before.page.chapter_start = super::super::theme_format::ChapterStart::Continuous;
    before.page.header.show_on_first_page = true;
    let files = parts(&full(&before).bytes);
    let xml = String::from_utf8_lossy(&files[MAIN]);
    assert!(paragraph(&xml, "Table caption").contains("<w:keepNext w:val=\"1\"/>"));
    assert!(xml.find("Table caption").unwrap() < xml.find("Two rows").unwrap());
    assert_eq!(xml.matches("w:pageBreakBefore").count(), 1); // Contents always starts its own page.
    assert!(paragraph(&xml, "Contents").contains("w:pageBreakBefore"));
    assert!(String::from_utf8_lossy(&files["word/header-first.xml"]).contains("STYLEREF"));
}

#[test]
fn excluded_cells_and_vertical_continuations_retain_fixed_grid_and_required_paragraphs() {
    let source = json!({"type":"doc","content":[{"type":"table","attrs":{"columnWidthsPercent":[20,30,50]},"content":[
        {"type":"table_row","content":[{"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"Header one"}]}]},{"type":"table_header","attrs":{"colspan":2},"content":[{"type":"paragraph","content":[{"type":"text","text":"Header two"}]}]}]},
        {"type":"table_row","content":[{"type":"table_cell","attrs":{"id":"excluded-paragraph","rowspan":2},"content":[{"type":"paragraph","content":[{"type":"text","text":"Hidden cell"}]}]},{"type":"table_cell","attrs":{"colspan":2},"content":[{"type":"paragraph","content":[{"type":"text","text":"Visible cell"}]}]}]},
        {"type":"table_row","content":[{"type":"table_cell","content":[{"type":"paragraph"}]},{"type":"table_cell","content":[{"type":"paragraph"}]}]}
    ]}]});
    let theme = theme("classic");
    let writer =
        DocxDocument::new(&theme, "Excluded cells", "en", &super::tests::font_folder()).unwrap();
    let bundle = render::render(&prepare(&source, &mut ReferenceScope::default()), writer).unwrap();
    let files = parts(&bundle.bytes);
    let xml = String::from_utf8_lossy(&files[MAIN]);
    assert!(!xml.contains("Hidden cell") && xml.contains("Visible cell"));
    let document = tags(&files[MAIN]);
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "w:tblHeader")
            .count(),
        1
    );
    assert_eq!(
        document.iter().filter(|(name, _)| name == "w:tc").count(),
        7
    );
    assert_eq!(
        document
            .iter()
            .filter(|(name, attrs)| name == "w:vMerge" && attrs["w:val"] == "continue")
            .count(),
        1
    );
    for cell in xml.split("<w:tc>").skip(1) {
        assert!(cell.split("</w:tc>").next().unwrap().ends_with("</w:p>"));
    }
    let tracks: Vec<f64> = document
        .iter()
        .filter(|(name, _)| name == "w:gridCol")
        .map(|(_, attrs)| attrs["w:w"].parse().unwrap())
        .collect();
    assert_eq!(tracks.len(), 3);
    let total: f64 = tracks.iter().sum();
    assert!((tracks[0] / total - 0.2).abs() < 0.001 && (tracks[2] / total - 0.5).abs() < 0.001);
}

#[test]
fn fully_excluded_container_children_leave_a_valid_editable_empty_cell() {
    let theme = theme("classic");
    let source = json!({"type":"doc","content":[{"type":"blockquote","content":[{"type":"paragraph","attrs":{"id":"excluded-paragraph"},"content":[{"type":"text","text":"Hidden paragraph"}]}]}]});
    let bundle = render::render(
        &prepare(&source, &mut ReferenceScope::default()),
        DocxDocument::new(&theme, "Empty quote", "en", &super::tests::font_folder()).unwrap(),
    )
    .unwrap();
    let files = parts(&bundle.bytes);
    let text = String::from_utf8_lossy(&files[MAIN]);
    assert!(!text.contains("Hidden paragraph"));
    assert!(text.contains("</w:p></w:tc>"));
}

#[test]
fn empty_list_and_figure_wrappers_retain_their_bookmarks_before_the_next_chapter() {
    let theme = theme("classic");
    let source = json!({"type":"doc","content":[
        {"type":"list","attrs":{"id":"empty-list"},"content":[{"type":"listItem","content":[{"type":"paragraph","attrs":{"id":"excluded-paragraph"}}]}]},
        {"type":"figure","attrs":{"id":"empty-figure"},"content":[{"type":"blockTitle","content":[]},{"type":"image","attrs":{"id":"excluded-paragraph","src":"/assets/test/pixel.png","alt":"Excluded image"}}]}
    ]});
    let mut writer =
        DocxDocument::new(&theme, "Empty wrappers", "en", &super::tests::font_folder()).unwrap();
    for id in ["empty-list", "empty-figure"] {
        writer.bookmarks.link(&format!("block-{id}"));
    }
    render::walk(
        &prepare(&source, &mut ReferenceScope::default()),
        &mut writer,
    )
    .unwrap();
    writer.chapter("Next chapter", 1, "next", false).unwrap();
    let files = parts(&writer.finish().unwrap().bytes);
    let xml = String::from_utf8_lossy(&files[MAIN]);
    assert_eq!(
        tags(&files[MAIN])
            .iter()
            .filter(|(name, attrs)| name == "w:bookmarkStart" && attrs["w:name"].starts_with("b_"))
            .count(),
        3
    );
    let chapter = xml[..xml.find("BluinkChapter").unwrap()]
        .rfind("<w:p>")
        .unwrap();
    assert_eq!(xml[..chapter].matches("<w:p>").count(), 2);
    assert!(!xml.contains("w:drawing"));
}

#[test]
#[ignore = "Explicit developer command writes the complete DOCX corpus for Microsoft schema and Word verification."]
fn write_complete_word_proofs() {
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/complete-word-proof");
    std::fs::create_dir_all(&output).unwrap();
    for name in ["classic", "editorial"] {
        std::fs::write(
            output.join(format!("{name}.docx")),
            full(&theme(name)).bytes,
        )
        .unwrap();
    }
}
