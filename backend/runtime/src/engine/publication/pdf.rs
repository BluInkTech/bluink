// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::plan::reading::BookEntry;
#[cfg(test)]
use super::plan::reading::collect_book_entries;
use crate::engine::project::item::read_project_item;
use crate::engine::publication::PdfProfile;
use crate::engine::publication::plan::{PublicationFormat, PublicationPlan, assert_plan_fresh};
use crate::engine::publication::source::{
    document, min_heading_level, parse_language, resolved_fields,
};
use crate::engine::publication::theme::load_pdf_theme;
use crate::engine::publication::typst::{
    TypstResources, TypstState, render_typst_document_in_scope,
};

pub(crate) struct PdfBundle {
    pub(crate) shell_path: String,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) page_lines: Vec<(usize, String)>,
}

/// Compile in a private sibling folder and publish the verified PDF in one
/// rename. A failed compiler run never leaves a partial destination file.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn publish_pdf(
    bundle: &PdfBundle,
    profile: PdfProfile,
    destination: &Path,
    typst: &Path,
    font_path: &Path,
) -> Result<(), String> {
    if !font_path.is_dir() {
        return Err(format!(
            "Packaged font folder is missing: {}",
            font_path.display()
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "PDF output already exists: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect PDF output: {error}")),
    }
    let parent = destination
        .parent()
        .filter(|path| path.is_dir())
        .ok_or("PDF output parent must be an existing folder.")?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("PDF output needs a filename.")?;
    if !name.to_ascii_lowercase().ends_with(".pdf") {
        return Err("PDF output filename must end in .pdf.".to_owned());
    }
    let stage = parent.join(format!(".{name}.bluink-{}.tmp", uuid::Uuid::now_v7()));
    fs::create_dir(&stage).map_err(|error| format!("Cannot stage PDF output: {error}"))?;
    let result = (|| {
        for (relative, bytes) in &bundle.files {
            let path = Path::new(relative);
            if relative.is_empty()
                || relative.contains('\\')
                || path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(format!("Unsafe PDF bundle path: {relative}"));
            }
            let output = stage.join(path);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Cannot create PDF stage folder: {error}"))?;
            }
            fs::write(&output, bytes)
                .map_err(|error| format!("Cannot write PDF bundle file {relative}: {error}"))?;
        }
        if !bundle.files.contains_key(&bundle.shell_path) {
            return Err("PDF bundle has no theme shell.".to_owned());
        }
        let version = Command::new(typst)
            .arg("--version")
            .output()
            .map_err(|error| format!("Cannot start packaged Typst: {error}"))?;
        if !version.status.success()
            || !String::from_utf8_lossy(&version.stdout).starts_with("typst 0.15.1 ")
        {
            return Err("Packaged Typst must be version 0.15.1.".to_owned());
        }
        let output_pdf = stage.join("result.pdf");
        let diagnostic_path = stage.join("typst-diagnostics.txt");
        let diagnostics = fs::File::create(&diagnostic_path)
            .map_err(|error| format!("Cannot stage Typst diagnostics: {error}"))?;
        let mut command = Command::new(typst);
        command.args(["compile", "--features", "a11y-extras"]);
        if matches!(profile, PdfProfile::Accessible) {
            command.args(["--pdf-standard", "ua-1"]);
        }
        command.args([
            "--root",
            stage.to_str().ok_or("PDF stage path is not UTF-8.")?,
            "--ignore-system-fonts",
            "--font-path",
            font_path
                .to_str()
                .ok_or("Packaged font path is not UTF-8.")?,
            "--diagnostic-format",
            "short",
        ]);
        command
            .arg(stage.join(&bundle.shell_path))
            .arg(&output_pdf)
            .stdout(Stdio::null())
            .stderr(Stdio::from(diagnostics));
        let mut child = command
            .spawn()
            .map_err(|error| format!("Cannot start Typst: {error}"))?;
        let deadline = Instant::now() + Duration::from_mins(30);
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("Cannot wait for Typst: {error}"))?
            {
                if !status.success() {
                    let diagnostics = fs::read_to_string(&diagnostic_path)
                        .unwrap_or_else(|_| "Typst diagnostics unavailable.".to_owned());
                    let source = source_for_diagnostic(&diagnostics, &bundle.page_lines)
                        .map(|id| format!(" in page {id}"))
                        .unwrap_or_default();
                    return Err(format!("Typst failed{source}: {}", diagnostics.trim()));
                }
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Typst exceeded the 30-minute compilation limit.".to_owned());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let bytes =
            fs::read(&output_pdf).map_err(|error| format!("Cannot read compiled PDF: {error}"))?;
        if bytes.len() < 5 || bytes.len() > 1024 * 1024 * 1024 || !bytes.starts_with(b"%PDF-") {
            return Err("Typst produced an invalid or oversized PDF.".to_owned());
        }
        fs::hard_link(&output_pdf, destination)
            .map_err(|error| format!("Cannot publish PDF output: {error}"))
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

fn source_for_diagnostic<'a>(
    diagnostics: &str,
    page_lines: &'a [(usize, String)],
) -> Option<&'a str> {
    let line = diagnostics
        .split("content.typ:")
        .nth(1)?
        .split(':')
        .next()?
        .parse::<usize>()
        .ok()?;
    page_lines
        .iter()
        .rev()
        .find(|(start, _)| *start <= line)
        .map(|(_, id)| id.as_str())
}

/// Assemble an immutable Typst source tree from a fresh Publication plan.
/// The shell and its support files are copied from one validated theme package.
#[tracing::instrument(level = "debug", skip_all, fields(page_count = plan.pages.len(), asset_count = plan.assets.len()))]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
pub(crate) fn render_publication_pdf(
    root: &Path,
    plan: &PublicationPlan,
) -> Result<PdfBundle, String> {
    if !matches!(plan.format, PublicationFormat::Pdf) {
        return Err("A PDF renderer needs a PDF Publication plan.".to_owned());
    }
    assert_plan_fresh(root, plan)?;
    let mut files = load_pdf_theme(root, &plan.theme, &plan.title, "en")?;
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let mut content = String::from("#import \"theme-values.typ\": *\n");
    writeln!(content, "#set document(title: {})", quote(&plan.title))
        .expect("writing to a String cannot fail");
    let book_entries = super::plan::reading::entries(plan)?;
    let has_contents = book_entries.len() > 1;
    let mut page_lines = Vec::new();
    let mut citation_scope = TypstState::default();
    let assets = resolved_assets(plan);
    let mut counted_bytes = 0;
    let mut line_number = 1;
    for entry in book_entries {
        line_number += content[counted_bytes..]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        counted_bytes = content.len();
        if let BookEntry::Section {
            id,
            title,
            number,
            depth,
        } = &entry
        {
            page_lines.push((line_number, id.clone()));
            write!(
                content,
                "#publication-chapter({})[#heading(level: {}, supplement: [Chapter])[#text({})]]\n\n",
                quote(&format!("{number}. {title}")),
                depth,
                quote(&format!("{number}. {title}")),
            )
            .expect("writing to a String cannot fail");
            continue;
        }
        let BookEntry::Page { id, number, intro } = entry else {
            unreachable!()
        };
        let page = plan
            .pages
            .iter()
            .find(|page| page.id == id)
            .ok_or("Book entry was not captured as a page.")?;
        let is_root = page.id == plan.root_page_id;
        page_lines.push((line_number, page.id.clone()));
        let item = read_project_item(root, &page.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?;
        let language = item["meta"]["lang"].as_str().unwrap_or("en");
        let (lang, region) = parse_language(language)
            .ok_or_else(|| format!("Page {} has invalid language tag {language}.", page.id))?;
        if is_root {
            write!(content,
                "#set text(lang: {}, region: {})\n#heading(level: 1, outlined: false)[#text({})] <note-{}>\n\n",
                quote(&lang),
                region
                    .as_deref()
                    .map_or_else(|| "none".to_owned(), quote),
                quote(&plan.title),
                page.id
            ).expect("writing to a String cannot fail");
        } else if !intro {
            let number = number.as_deref().ok_or("Chapter has no authored number.")?;
            write!(content,
                "#publication-chapter({})[]\n#set text(lang: {}, region: {})\n#heading(level: {}, supplement: [Chapter])[#text({})] <note-{}>\n\n",
                quote(&format!("{number}. {}", page.title)),
                quote(&lang),
                region.as_deref().map_or_else(|| "none".to_owned(), quote),
                page.depth,
                quote(&format!("{number}. {}", page.title)),
                page.id
            ).expect("writing to a String cannot fail");
        } else {
            write!(
                content,
                "#set text(lang: {}, region: {})\n#metadata(()) <note-{}>\n\n",
                quote(&lang),
                region.as_deref().map_or_else(|| "none".to_owned(), quote),
                page.id,
            )
            .expect("writing to a String cannot fail");
        }
        let links = super::render::native_links(&plan.links, &page.id)?;
        let fields = resolved_fields(&item, document(&item))?;
        let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
        let body = render_typst_document_in_scope(
            document(&item),
            &TypstResources {
                page_id: &page.id,
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            super::plan::reading::body_heading_level(page.depth)?
                - i16::try_from(min_heading_level(document(&item), &exclusions).unwrap_or(1))
                    .map_err(|_| "Page heading level is too large.")?,
            &mut citation_scope,
        )
        .map_err(|error| format!("Page {}: {error}", page.id))?;
        content.push_str(&body);
        if is_root && has_contents {
            content.push_str("#pagebreak()\n#outline(title: [Contents], target: heading.where(supplement: [Chapter]))\n\n");
        }
    }
    let content_path = "content.typ".to_owned();
    files.insert(content_path, content.into_bytes());
    for asset in &plan.assets {
        let bytes = super::render::read_asset(root, asset)?;
        files.insert(
            format!("assets/{}/{}", asset.owner_id, asset.filename),
            bytes,
        );
    }
    assert_plan_fresh(root, plan)?;
    Ok(PdfBundle {
        shell_path: "document.typ".to_owned(),
        files,
        page_lines,
    })
}

fn resolved_assets(plan: &PublicationPlan) -> HashMap<String, String> {
    plan.assets
        .iter()
        .map(|asset| {
            (
                asset.src.clone(),
                format!("assets/{}/{}", asset.owner_id, asset.filename),
            )
        })
        .collect()
}

pub(crate) use super::typst::quoted as quote;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        BookEntry, PdfBundle, collect_book_entries, publish_pdf, render_publication_pdf,
        source_for_diagnostic,
    };
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::plan::{
        PlannedStructureNode, PublicationFormat, plan_publication,
    };

    #[test]
    fn shared_query_pages_are_rendered_once() {
        let nodes = vec![
            PlannedStructureNode::Collection {
                id: "first".to_owned(),
                title: "First".to_owned(),
                page_ids: vec!["page".to_owned()],
                show_pages_in_navigation: true,
                listing_slug: None,
                page_size: 20,
                feed: false,
                archives: false,
            },
            PlannedStructureNode::Collection {
                id: "second".to_owned(),
                title: "Second".to_owned(),
                page_ids: vec!["page".to_owned()],
                show_pages_in_navigation: true,
                listing_slug: None,
                page_size: 20,
                feed: false,
                archives: false,
            },
        ];
        let mut entries = Vec::new();
        collect_book_entries(
            &nodes,
            1,
            &mut Vec::new(),
            &mut std::collections::HashSet::new(),
            &mut entries,
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, BookEntry::Page { .. }))
                .count(),
            1
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep each end-to-end scenario together so setup, failure checks and cleanup remain visible."
    )]
    fn renders_book_and_compiles_with_packaged_typst() {
        let binary = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "linux-x64/typst"
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "windows-x64/typst.exe"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-arm64/typst"
        } else {
            return;
        };
        let typst_binary = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../typst/assets")
            .join(binary);
        if !typst_binary.is_file() {
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]}),
            None,
        )
        .unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let asset_src = format!("/assets/{}/image.png", ids[0]);
        write_item_input(&root, json!({"id":ids[0],"meta":{"type":"page","title":"Start","lang":"en-GB"},
            "content":{"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Next chapter","marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[1])}}]}]},
                {"type":"image","attrs":{"src":asset_src,"alt":"A sample image"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Chapter one","lang":"en"},
            "content":{"type":"doc","content":[{"type":"heading","attrs":{"level":2,"id":uuid::Uuid::now_v7().to_string()},"content":[{"type":"text","text":"Details"}]}]}}),
            None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Source","lang":"en"},
            "content":{"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted source"}]}]}}),
            None, "2026-09-28").unwrap();
        write_asset(
            &root,
            &ids[0],
            "image.png",
            &std::fs::read(fixture.join("../one-pixel.png")).unwrap(),
        )
        .unwrap();
        write_item_input(&root, json!({"id":ids[3],"meta":{"type":"publication","title":"Book"},
            "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"section","id":uuid::Uuid::now_v7().to_string(),"title":"Fundamentals","introPageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        let bundle = render_publication_pdf(&root, &plan).unwrap();
        let source = String::from_utf8(bundle.files["content.typ"].clone()).unwrap();
        assert!(source.contains(&format!("<note-{}>", ids[1])));
        assert!(source.contains("Quoted source"));
        assert!(source.contains("1. Fundamentals"));
        assert!(source.contains("#heading(level: 3)[#text(\"Details\")]"));
        assert_eq!(source.matches("Details").count(), 1);
        publish_pdf(
            &bundle,
            plan.targets.pdf.profile,
            &root.join("book.pdf"),
            &typst_binary,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("book.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        assert!(
            publish_pdf(
                &bundle,
                plan.targets.pdf.profile,
                &root.join("book.pdf"),
                &typst_binary,
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf")
            )
            .unwrap_err()
            .contains("already exists")
        );
        let editorial_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":editorial_id,"meta":{"type":"publication","title":"Editorial book"},
            "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"editorial","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let editorial_plan =
            plan_publication(&root, &editorial_id, PublicationFormat::Pdf).unwrap();
        let editorial = render_publication_pdf(&root, &editorial_plan).unwrap();
        publish_pdf(
            &editorial,
            editorial_plan.targets.pdf.profile,
            &root.join("editorial.pdf"),
            &typst_binary,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("editorial.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes");
        let installed = root.join("themes/field-notes");
        for directory in ["html", "site"] {
            std::fs::create_dir_all(installed.join(directory)).unwrap();
        }
        for relative in ["manifest.json", "html/article.html", "site/page.html"] {
            std::fs::copy(example.join(relative), installed.join(relative)).unwrap();
        }
        let themed_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":themed_id,"meta":{"type":"publication","title":"Field notes book"},
            "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"field-notes","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let themed_plan = plan_publication(&root, &themed_id, PublicationFormat::Pdf).unwrap();
        assert!(!themed_plan.theme.requires_trust);
        let themed = render_publication_pdf(&root, &themed_plan).unwrap();
        let themed_values = String::from_utf8(themed.files["theme-values.typ"].clone()).unwrap();
        assert!(themed_values.contains("#f2f4f8"));
        publish_pdf(
            &themed,
            themed_plan.targets.pdf.profile,
            &root.join("field-notes.pdf"),
            &typst_binary,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("field-notes.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let accessible_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":accessible_id,"meta":{"type":"publication","title":"Accessible book"},
            "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"accessible","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let accessible_plan =
            plan_publication(&root, &accessible_id, PublicationFormat::Pdf).unwrap();
        let accessible = render_publication_pdf(&root, &accessible_plan).unwrap();
        publish_pdf(
            &accessible,
            accessible_plan.targets.pdf.profile,
            &root.join("accessible.pdf"),
            &typst_binary,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf"),
        )
        .unwrap();
        assert!(
            std::fs::read(root.join("accessible.pdf"))
                .unwrap()
                .starts_with(b"%PDF-")
        );
        let bad = PdfBundle {
            shell_path: "pdf/document.typ".to_owned(),
            files: std::collections::BTreeMap::from([(
                "pdf/document.typ".to_owned(),
                b"#undefined-publication-function()".to_vec(),
            )]),
            page_lines: Vec::new(),
        };
        assert_eq!(
            source_for_diagnostic(
                "error at pdf/content.typ:25:8",
                &[(1, "first".to_owned()), (20, "second".to_owned())]
            ),
            Some("second")
        );
        assert!(
            publish_pdf(
                &bad,
                accessible_plan.targets.pdf.profile,
                &root.join("bad.pdf"),
                &typst_binary,
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf")
            )
            .unwrap_err()
            .contains("Typst failed")
        );
        assert!(!root.join("bad.pdf").exists());
        temporary.close().unwrap();
    }
}
