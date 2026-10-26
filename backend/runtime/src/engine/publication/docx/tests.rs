// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::*;
use crate::engine::publication::{
    render::{self, ReferenceScope, Resources},
    theme_format::{self, Definition, Overrides},
};
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Read},
};

pub(super) fn theme(id: &str) -> ResolvedTheme {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("vendor/themes/{id}/manifest.json"));
    let definition: Definition = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    theme_format::resolve(&definition, "docx", &Overrides::default()).unwrap()
}

pub(super) fn font_folder() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/docx")
}

fn source() -> Value {
    json!({"type":"doc","content":[
        {"type":"heading","attrs":{"id":"heading","level":1},"content":[{"type":"text","text":"Native DOCX theme proof"}]},
        {"type":"paragraph","attrs":{"id":"paragraph"},"content":[
            {"type":"text","text":"Café & <test> 😀 "},
            {"type":"text","text":"bold","marks":[{"type":"strong"}]},
            {"type":"text","text":" italic","marks":[{"type":"emphasis"}]},
            {"type":"text","text":" raised bold","marks":[{"type":"strong"},{"type":"superscript"}]},
            {"type":"text","text":" underlined","marks":[{"type":"underline"}]},
            {"type":"text","text":" struck","marks":[{"type":"strike"}]},
            {"type":"text","text":" code","marks":[{"type":"code"}]},
            {"type":"text","text":"2","marks":[{"type":"superscript"}]},
            {"type":"text","text":"O","marks":[{"type":"subscript"}]},
            {"type":"hard_break"},
            {"type":"text","text":"External link","marks":[{"type":"link","attrs":{"href":"https://example.com/?a=1&b=2"}},{"type":"strong"}]},
            {"type":"text","text":" Heading bookmark","marks":[{"type":"link","attrs":{"href":"#heading"}}]},
            {"type":"text","text":" plain words","marks":[{"type":"link","attrs":{"href":"text-only"}}]}
        ]},
        {"type":"paragraph"},
        {"type":"code_block","attrs":{"language":"rust"},"content":[{"type":"text","text":"fn main() {\n\tprintln!(\"<safe> & intact\");\n}"}]},
        {"type":"horizontal_rule"}
    ]})
}

pub(super) fn prepared(source: &Value, page: &str) -> render::PreparedPage {
    render::prepare_page(
        page,
        source,
        &Resources {
            links: &HashMap::from([
                (
                    "https://example.com/?a=1&b=2".into(),
                    Destination::Url("https://example.com/?a=1&b=2".into()),
                ),
                (
                    "#heading".into(),
                    Destination::Anchor("block-heading".into()),
                ),
                ("text-only".into(), Destination::PlainText),
            ]),
            assets: &HashMap::new(),
            fields: &HashMap::new(),
            snippets: &HashMap::new(),
            excluded_block_ids: &HashSet::new(),
        },
        0,
        &mut ReferenceScope::default(),
    )
    .unwrap()
}

fn bundle(source: &Value, theme: &ResolvedTheme) -> DocxBundle {
    render::render(
        &prepared(source, "proof-page"),
        DocxDocument::new(
            theme,
            "Proof & <title>",
            "en-GB",
            &super::tests::font_folder(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn drawing_in_a_numbered_container_has_one_spacing_and_ordered_properties() {
    let theme = theme("classic");
    let mut writer = DocxDocument::new(&theme, "Drawing", "en", &font_folder()).unwrap();
    writer.story.drawing = true;
    writer.story.list_item = true;
    writer.story.lists.push(1);
    writer.begin_paragraph(Role::Box, None, None).unwrap();
    writer.end_paragraph().unwrap();
    writer.story.xml.end().unwrap(); // w:body
    writer.story.xml.end().unwrap(); // w:document
    let xml = String::from_utf8(writer.story.xml.finish().unwrap()).unwrap();
    assert_eq!(xml.matches("<w:spacing ").count(), 1);
    let order =
        ["<w:numPr>", "<w:spacing ", "<w:ind ", "<w:jc "].map(|element| xml.find(element).unwrap());
    assert!(
        order
            .windows(2)
            .all(|positions| positions[0] < positions[1])
    );
    assert!(xml.contains("w:lineRule=\"auto\""));
}

pub(crate) fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut parts = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        assert_eq!(file.last_modified().unwrap().year(), 1980);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert!(parts.insert(file.name().to_owned(), bytes).is_none());
    }
    parts
}

pub(super) fn tags(bytes: &[u8]) -> Vec<(String, BTreeMap<String, String>)> {
    let mut reader = NsReader::from_reader(bytes);
    let mut tags = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event().unwrap();
        assert!(
            !matches!(namespace, ResolveResult::Unknown(_)),
            "Unbound XML namespace"
        );
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let attributes = tag
                    .attributes()
                    .map(|attribute| {
                        let attribute = attribute.unwrap();
                        (
                            attribute.key.as_ref().to_owned(),
                            attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned(),
                        )
                    })
                    .collect();
                tags.push((tag.name().as_ref().to_owned(), attributes));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    tags
}

#[test]
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical OPC names are deliberately case-sensitive."
)]
fn native_package_is_deterministic_namespace_valid_and_has_a_closed_relationship_graph() {
    let theme = theme("classic");
    let first = bundle(&source(), &theme);
    let second = bundle(&source(), &theme);
    assert_eq!(first.bytes, second.bytes);
    let files = parts(&first.bytes);
    assert_eq!(
        files
            .keys()
            .filter(|name| name.ends_with(".xml") || name.ends_with(".rels"))
            .cloned()
            .collect::<Vec<_>>(),
        [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/app.xml",
            "docProps/core.xml",
            "word/_rels/document.xml.rels",
            "word/_rels/fontTable.xml.rels",
            "word/document.xml",
            "word/fontTable.xml",
            "word/footer-first.xml",
            "word/footer.xml",
            "word/header-first.xml",
            "word/header.xml",
            "word/numbering.xml",
            "word/settings.xml",
            "word/styles.xml"
        ]
    );
    for (_, bytes) in files
        .iter()
        .filter(|(name, _)| name.ends_with(".xml") || name.ends_with(".rels"))
    {
        assert_ne!(tags(bytes), []);
    }
    let types = tags(&files["[Content_Types].xml"]);
    assert!(types.iter().any(|(_,attrs)|attrs.get("ContentType").is_some_and(|kind|kind=="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")));
    let roots = tags(&files["_rels/.rels"]);
    assert!(roots.iter().any(|(_, attrs)| {
        attrs
            .get("Type")
            .is_some_and(|kind| kind.ends_with("/metadata/core-properties"))
    }));
    let core = String::from_utf8_lossy(&files["docProps/core.xml"]);
    assert!(core.contains("Proof &amp; &lt;title&gt;"));
    assert!(!core.contains("created"));
}

#[test]
fn styles_map_every_role_and_preserve_relative_type_and_physical_page_units() {
    let mut theme = theme("classic");
    theme
        .styles
        .get_mut(&Role::BoxWarning)
        .unwrap()
        .border_style = theme_format::Stroke::Dashed;
    let files = parts(&bundle(&source(), &theme).bytes);
    let styles = tags(&files["word/styles.xml"]);
    let ids = styles
        .iter()
        .filter(|(name, _)| name == "w:style")
        .map(|(_, attributes)| attributes["w:styleId"].clone())
        .collect::<HashSet<_>>();
    for role in Role::ALL {
        assert!(ids.contains(styles::id(role)), "{}", role.name());
    }
    assert!(styles.iter().any(|(name, attrs)| name == "w:style"
        && attrs.get("w:styleId").is_some_and(|id| id == "BluinkTable")
        && attrs["w:type"] == "table"));
    assert!(styles.iter().any(|(name, attrs)| name == "w:spacing"
        && attrs.get("w:line").is_some_and(|value| value == "384")
        && attrs["w:lineRule"] == "exact"));
    assert!(styles.iter().any(|(name, attrs)| {
        name == "w:rFonts"
            && attrs["w:ascii"]
                == super::fonts::Fonts::load(&font_folder(), &theme)
                    .unwrap()
                    .family("source-serif-4")
                    .unwrap()
    }));
    let document = tags(&files[MAIN]);
    let size = &document
        .iter()
        .find(|(name, _)| name == "w:pgSz")
        .unwrap()
        .1;
    assert_eq!(size["w:w"], "11906");
    assert_eq!(size["w:h"], "16838");
    assert_eq!(
        document
            .iter()
            .find(|(name, _)| name == "w:pgMar")
            .unwrap()
            .1["w:left"],
        "1417"
    );
    for value in [f64::NAN, -1.0, f64::INFINITY] {
        assert!(styles::twips(value).is_err());
    }
}

#[test]
fn target_overrides_compile_to_native_landscape_geometry_and_font_relative_leading() {
    let definition: Definition = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/themes/classic/manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let overrides: Overrides=serde_json::from_value(json!({"shared":{"root":{"fontSize":"15pt"}},"targets":{"docx":{"page":{"width":"8.5in","height":"11in","orientation":"landscape","marginLeft":"1in"}}}})).unwrap();
    let theme = theme_format::resolve(&definition, "docx", &overrides).unwrap();
    let files = parts(&bundle(&source(), &theme).bytes);
    let document = tags(&files[MAIN]);
    let page = &document
        .iter()
        .find(|(name, _)| name == "w:pgSz")
        .unwrap()
        .1;
    assert_eq!(page["w:w"], "15840");
    assert_eq!(page["w:h"], "12240");
    assert_eq!(page["w:orient"], "landscape");
    assert_eq!(
        document
            .iter()
            .find(|(name, _)| name == "w:pgMar")
            .unwrap()
            .1["w:left"],
        "1440"
    );
    let styles = tags(&files["word/styles.xml"]);
    assert!(
        styles
            .iter()
            .any(|(name, attrs)| name == "w:sz" && attrs["w:val"] == "30")
    );
    assert!(styles.iter().any(|(name, attrs)| name == "w:spacing"
        && attrs["w:line"] == "480"
        && attrs["w:lineRule"] == "exact"));
}

#[test]
fn text_marks_links_and_safe_bookmarks_remain_native_editable_word_content() {
    let files = parts(&bundle(&source(), &theme("classic")).bytes);
    let text = String::from_utf8_lossy(&files[MAIN]);
    assert!(text.contains("Café &amp; &lt;test&gt; 😀 "));
    assert!(text.contains("xml:space=\"preserve\""));
    assert!(text.contains("<w:b w:val=\"1\"/>") && text.contains("<w:i w:val=\"1\"/>"));
    assert!(text.contains("<w:tab/>") && text.contains("<w:br/>"));
    assert!(
        text.contains("<w:vertAlign w:val=\"superscript\"/>")
            && text.contains("<w:vertAlign w:val=\"subscript\"/>")
    );
    let document = tags(&files[MAIN]);
    let starts = document
        .iter()
        .filter(|(name, _)| name == "w:bookmarkStart")
        .map(|(_, attrs)| attrs)
        .collect::<Vec<_>>();
    assert_eq!(
        starts
            .iter()
            .filter(|bookmark| bookmark["w:name"].starts_with("b_"))
            .count(),
        2
    );
    assert_eq!(
        starts
            .iter()
            .filter(|bookmark| bookmark["w:name"].starts_with("_Toc"))
            .count(),
        0
    );
    for bookmark in &starts {
        assert!(bookmark["w:name"].starts_with("b_") || bookmark["w:name"].starts_with("_Toc"));
        assert!(bookmark["w:name"].len() <= 40);
        assert!(
            document
                .iter()
                .any(|(name, attrs)| name == "w:bookmarkEnd" && attrs["w:id"] == bookmark["w:id"])
        );
    }
    assert_eq!(
        document
            .iter()
            .filter(|(name, _)| name == "w:hyperlink")
            .count(),
        2
    );
    assert!(
        document.iter().any(|(name, attrs)| name == "w:hyperlink"
            && attrs.get("w:anchor") == starts[0].get("w:name"))
    );
    let relationships = tags(&files["word/_rels/document.xml.rels"]);
    assert!(relationships.iter().any(|(_, attrs)| {
        attrs
            .get("Target")
            .is_some_and(|target| target == "https://example.com/?a=1&b=2")
            && attrs["TargetMode"] == "External"
    }));
    assert!(!text.contains("altChunk"));
}

#[test]
fn package_registry_rejects_missing_parts_duplicates_and_unsafe_names() {
    let mut package = Package::default();
    assert!(
        package
            .add("../document.xml", "application/xml", vec![])
            .is_err()
    );
    assert!(
        package
            .add("word\\document.xml", "application/xml", vec![])
            .is_err()
    );
    package.add(MAIN, "application/xml", vec![]).unwrap();
    assert!(package.add(MAIN, "application/xml", vec![]).is_err());
    package.relate("", "officeDocument", MAIN).unwrap();
    package.relate(MAIN, "styles", "word/missing.xml").unwrap();
    assert!(package.finish().unwrap_err().contains("target is missing"));
    assert!(Package::default().finish().is_err());
    let mut package = Package::default();
    for target in ["javascript:bad()", "file:///secret", "../relative"] {
        assert!(package.hyperlink(MAIN, target).is_err());
    }
}

#[test]
fn numbering_owns_native_levels_and_independent_start_overrides() {
    let mut numbering = Numbering::default();
    assert_eq!(numbering.list(true, 7, 0).unwrap(), 1);
    assert_eq!(numbering.list(false, 1, 1).unwrap(), 2);
    assert!(numbering.list(true, u64::MAX, 0).is_err());
    assert!(numbering.list(true, 1, 9).is_err());
    let theme = theme("classic");
    let fonts = super::fonts::Fonts::load(&font_folder(), &theme).unwrap();
    let xml = tags(&numbering.compile(&theme, &fonts, "en").unwrap());
    assert_eq!(xml.iter().filter(|(name, _)| name == "w:lvl").count(), 18);
    assert!(
        xml.iter()
            .any(|(name, attrs)| name == "w:startOverride" && attrs["w:val"] == "7")
    );
}

#[test]
fn multiple_prepared_pages_share_bookmarks_and_failures_retain_source_context() {
    let theme = theme("classic");
    let mut writer =
        DocxDocument::new(&theme, "Linked proof", "en", &super::tests::font_folder()).unwrap();
    let first = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Later heading","marks":[{"type":"link","attrs":{"href":"#heading"}}]}]}]});
    render::walk(&prepared(&first, "first"), &mut writer).unwrap();
    render::walk(&prepared(&source(), "second"), &mut writer).unwrap();
    assert!(writer.finish().is_ok());
    let bad = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"bad-block"},"content":[{"type":"text","text":"invalid\u{000b}text"}]}]});
    let error = render::render(
        &prepared(&bad, "bad-page"),
        DocxDocument::new(&theme, "Proof", "en", &super::tests::font_folder()).unwrap(),
    )
    .err()
    .unwrap();
    assert_eq!(error.page_id.as_deref(), Some("bad-page"));
    assert_eq!(error.block_id.as_deref(), Some("bad-block"));
    assert!(error.message.contains("XML 1.0"));
    let missing = render::render(
        &prepared(&first, "first"),
        DocxDocument::new(&theme, "Proof", "en", &super::tests::font_folder()).unwrap(),
    )
    .err()
    .unwrap();
    assert!(missing.message.contains("target was not rendered"));
    let unsupported = json!({"type":"doc","content":[{"type":"blockquote","attrs":{"id":"quote"},"content":[{"type":"paragraph"}]}]});
    let quote = render::render(
        &prepared(&unsupported, "quote-page"),
        DocxDocument::new(&theme, "Proof", "en", &super::tests::font_folder()).unwrap(),
    )
    .unwrap();
    let files = parts(&quote.bytes);
    assert!(String::from_utf8_lossy(&files[MAIN]).contains("BluinkQuote"));
}

#[test]
fn publication_is_atomic_and_never_replaces_an_existing_destination() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("proof.docx");
    let bundle = bundle(&source(), &theme("classic"));
    publish_docx(&bundle, &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bundle.bytes);
    assert!(publish_docx(&bundle, &path).is_err());
    assert!(publish_docx(&bundle, &temp.path().join("proof.pdf")).is_err());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
#[ignore = "Explicit developer command writes native Word proof artifacts for real application verification."]
fn write_native_word_proofs() {
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/native-word-proof");
    std::fs::create_dir_all(&output).unwrap();
    let mut source = source();
    source["content"].as_array_mut().unwrap().insert(2,json!({"type":"paragraph","content":[{"type":"text","text":"Baseline first"},{"type":"hard_break"},{"type":"text","text":"Baseline second"}]}));
    for level in 2..=6 {
        source["content"].as_array_mut().unwrap().push(json!({"type":"heading","attrs":{"level":level},"content":[{"type":"text","text":format!("Heading level {level}")}]}));
    }
    for name in ["classic", "editorial"] {
        let bundle = bundle(&source, &theme(name));
        std::fs::write(output.join(format!("{name}.docx")), bundle.bytes).unwrap();
    }
}
