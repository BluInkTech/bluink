// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Component, Path},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

mod compiler;

use crate::domain::publication::{
    PdfProfile,
    plan::{PublicationFormat, PublicationPlan, reading::BookEntry},
};

#[cfg(test)]
use crate::domain::publication::plan::reading::collect_book_entries;
use crate::domain::publication::{
    source::{document, min_heading_level, parse_language, resolved_fields},
    theme_format::ReferencesAt,
    typst::{
        HeadingOptions, TypstResources, TypstState, heading as render_heading,
        render_typst_document_in_scope, render_typst_references,
    },
};
use crate::export_jobs::{plan::assert_plan_fresh, theme::load_pdf_theme};
use crate::store::snapshots::read_project_item;

#[cfg_attr(test, derive(Clone))]
pub(crate) struct PdfBundle {
    pub(crate) shell_path: String,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) page_lines: Vec<(usize, String)>,
}

/// Accepts only the pinned Typst. A version query that fails reports how it
/// failed, so a broken or blocked binary is not mistaken for a wrong version.
fn check_typst_version(version: &std::process::Output) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&version.stdout);
    if !version.status.success() {
        let stderr = String::from_utf8_lossy(&version.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!(
                "Packaged Typst could not report its version ({}).",
                version.status
            )
        } else {
            format!(
                "Packaged Typst could not report its version ({}): {detail}",
                version.status
            )
        });
    }
    if !stdout.starts_with("typst 0.15.1 ") {
        return Err(format!(
            "Packaged Typst must be version 0.15.1, but reports “{}”.",
            stdout.trim()
        ));
    }
    Ok(())
}

/// Compile without a cancellation source, as tests and fixtures do.
#[cfg(test)]
pub(crate) fn publish_pdf(
    bundle: PdfBundle,
    profile: PdfProfile,
    destination: &Path,
    typst: &Path,
    font_path: &Path,
) -> Result<(), String> {
    publish_pdf_until(bundle, profile, destination, typst, font_path, &|| false)
}

/// Compile in a private sibling folder and publish the verified PDF in one
/// hard link. A failed compiler run never leaves a partial destination file.
/// Typst stops as soon as `cancelled` returns true, so a cancelled export
/// frees the print lane and shutdown does not wait for it.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn publish_pdf_until(
    bundle: PdfBundle,
    profile: PdfProfile,
    destination: &Path,
    typst: &Path,
    font_path: &Path,
    cancelled: &dyn Fn() -> bool,
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
    let PdfBundle {
        shell_path,
        files,
        page_lines,
    } = bundle;
    let result = (|| {
        if !files.contains_key(&shell_path) {
            return Err("PDF bundle has no theme shell.".to_owned());
        }
        // Consume the source tree as it is staged. No source or compressed
        // asset buffers need to stay alive during the compiler's memory peak.
        for (relative, bytes) in files {
            let path = Path::new(&relative);
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
        compiler::verify_typst(typst)?;
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
            .arg(stage.join(&shell_path))
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
                    let source = source_for_diagnostic(&diagnostics, &page_lines)
                        .map(|id| format!(" in page {id}"))
                        .unwrap_or_default();
                    return Err(format!("Typst failed{source}: {}", diagnostics.trim()));
                }
                break;
            }
            if cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Export cancelled.".to_owned());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Typst exceeded the 30-minute compilation limit.".to_owned());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        compiler::validate_pdf(&output_pdf)?;
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
    let book_entries = crate::domain::publication::plan::reading::entries(plan)?;
    let has_contents = book_entries.len() > 1;
    let mut page_lines = Vec::new();
    let anchors = crate::domain::publication::render::linked_anchors(&plan.links);
    let links_by_page = crate::domain::publication::render::native_links_by_page(&plan.links)?;
    let empty_links = HashMap::new();
    let pages: HashMap<_, _> = plan
        .pages
        .iter()
        .map(|page| (page.id.as_str(), page))
        .collect();
    let page_style = &plan.theme.resolved.page;
    let heading = page_style.references_heading.as_str();
    // A book listing its sources at its end numbers them straight through.
    let mut book_notes = (page_style.references_at == ReferencesAt::Book).then(TypstState::default);
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
                "#publication-chapter({})[{}]\n\n",
                quote(&format!("{number}. {title}")),
                render_heading(
                    *depth,
                    &format!("#text({})", quote(&format!("{number}. {title}"))),
                    HeadingOptions {
                        chapter: true,
                        ..HeadingOptions::default()
                    }
                ),
            )
            .expect("writing to a String cannot fail");
            continue;
        }
        let BookEntry::Page { id, number, intro } = entry else {
            unreachable!()
        };
        let page = pages
            .get(id.as_str())
            .ok_or("Book entry was not captured as a page.")?;
        let is_root = page.id == plan.root_page_id;
        page_lines.push((line_number, page.id.clone()));
        let item = read_project_item(root, &page.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?;
        let language = item["meta"]["lang"].as_str().unwrap_or("en");
        let (lang, region) = parse_language(language)
            .ok_or_else(|| format!("Page {} has invalid language tag {language}.", page.id))?;
        // Each page is its own content block, so its language rule ends with
        // it. Unscoped, every page's set rule would wrap all later pages, and a
        // Publication of thousands of pages overflows Typst's stack. Its
        // footnotes number afresh, as each page's notes do on the website.
        content.push_str("#[\n#counter(footnote).update(0)\n");
        if is_root {
            write!(
                content,
                "#set text(lang: {}, region: {})\n{}",
                quote(&lang),
                region.as_deref().map_or_else(|| "none".to_owned(), quote),
                render_heading(
                    1,
                    &format!("#text({})", quote(&plan.title)),
                    HeadingOptions {
                        anchor: Some(&format!("note-{}", page.id)),
                        exclude_from_outline: true,
                        ..HeadingOptions::default()
                    }
                )
            )
            .expect("writing to a String cannot fail");
        } else if !intro {
            let number = number.as_deref().ok_or("Chapter has no authored number.")?;
            write!(
                content,
                "#publication-chapter({})[]\n#set text(lang: {}, region: {})\n{}",
                quote(&format!("{number}. {}", page.title)),
                quote(&lang),
                region.as_deref().map_or_else(|| "none".to_owned(), quote),
                render_heading(
                    page.depth,
                    &format!("#text({})", quote(&format!("{number}. {}", page.title))),
                    HeadingOptions {
                        anchor: Some(&format!("note-{}", page.id)),
                        chapter: true,
                        ..HeadingOptions::default()
                    }
                )
            )
            .expect("writing to a String cannot fail");
        } else {
            write!(
                content,
                "#set text(lang: {}, region: {})\n#metadata(())#label({})\n\n",
                quote(&lang),
                region.as_deref().map_or_else(|| "none".to_owned(), quote),
                quote(&format!("note-{}", page.id)),
            )
            .expect("writing to a String cannot fail");
        }
        let links = links_by_page.get(page.id.as_str()).unwrap_or(&empty_links);
        let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
        let fields = resolved_fields(&item, document(&item), &exclusions)?;
        let level = crate::domain::publication::plan::reading::body_heading_level(page.depth)?;
        let mut chapter_notes = TypstState::named(&page.id);
        let notes = book_notes.as_mut().unwrap_or(&mut chapter_notes);
        let body = render_typst_document_in_scope(
            document(&item),
            &TypstResources {
                page_id: &page.id,
                links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                anchors: &anchors,
                references_heading: heading,
            },
            level
                - i16::try_from(min_heading_level(document(&item), &exclusions).unwrap_or(1))
                    .map_err(|_| "Page heading level is too large.")?,
            notes,
        )
        .map_err(|error| crate::domain::publication::render::on_page(&page.id, error))?;
        content.push_str(&body);
        // Each chapter ends with the sources it cites, unless the book lists them.
        if book_notes.is_none() {
            content.push_str(
                &render_typst_references(
                    &mut chapter_notes,
                    &page.id,
                    Some((
                        heading,
                        u16::try_from(level).map_err(|_| "Heading level is invalid.")?,
                    )),
                    &anchors,
                )
                .map_err(|error| crate::domain::publication::render::on_page(&page.id, error))?,
            );
        }
        if is_root && has_contents {
            content.push_str("#pagebreak()\n");
            content.push_str(&render_heading(
                1,
                "Contents",
                HeadingOptions {
                    exclude_from_outline: true,
                    ..HeadingOptions::default()
                },
            ));
            content.push_str(
                "#outline(title: none, target: heading.where(supplement: [Chapter]))\n\n",
            );
        }
        content.push_str("]\n\n");
    }
    if let Some(notes) = &mut book_notes {
        let list = render_typst_references(notes, &plan.root_page_id, None, &anchors)?;
        if !list.is_empty() {
            // The book's sources close it as a chapter of their own in the contents.
            write!(
                content,
                "#publication-chapter({})[{}]\n\n{list}",
                quote(heading),
                render_heading(
                    1,
                    &format!("#text({})", quote(heading)),
                    HeadingOptions {
                        chapter: true,
                        ..HeadingOptions::default()
                    }
                ),
            )
            .expect("writing to a String cannot fail");
        }
    }
    let content_path = "content.typ".to_owned();
    files.insert(content_path, content.into_bytes());
    for asset in &plan.assets {
        let bytes = crate::export_jobs::resources::read_asset(root, asset)?;
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

pub(crate) use crate::domain::publication::typst::quoted as quote;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        BookEntry, PdfBundle, check_typst_version, collect_book_entries, publish_pdf,
        render_publication_pdf, source_for_diagnostic,
    };

    pub(super) fn version_output(code: u32, stdout: &str, stderr: &str) -> std::process::Output {
        #[cfg(windows)]
        let status = std::os::windows::process::ExitStatusExt::from_raw(code);
        #[cfg(unix)]
        let status =
            std::os::unix::process::ExitStatusExt::from_raw(i32::try_from(code).unwrap() << 8);
        std::process::Output {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn typst_version_check_reports_how_it_failed() {
        assert!(check_typst_version(&version_output(0, "typst 0.15.1 (9dfd3a08)\n", "")).is_ok());
        let older =
            check_typst_version(&version_output(0, "typst 0.14.0 (abc)\n", "")).unwrap_err();
        assert!(
            older.contains("must be version 0.15.1, but reports “typst 0.14.0 (abc)”"),
            "{older}"
        );
        let failed =
            check_typst_version(&version_output(3, "", "Access is denied.\n")).unwrap_err();
        assert!(
            failed.starts_with("Packaged Typst could not report its version ("),
            "{failed}"
        );
        assert!(failed.ends_with("): Access is denied."), "{failed}");
        assert!(!failed.contains("must be version"), "{failed}");
    }
    use crate::application::item_writes::write_asset;
    use crate::application::item_writes::write_item_input;
    use crate::domain::project::config::create_project_config;
    use crate::domain::publication::plan::PlannedStructureNode;
    use crate::domain::publication::plan::PublicationFormat;
    use crate::export_jobs::plan::plan_publication;

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
        crate::domain::project::properties::initialize(&root).unwrap();
        crate::application::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]}),
            None,
        )
        .unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        let asset_src = format!("/assets/{}/image.png", ids[0]);
        write_item_input(&root, json!({"id":ids[0],"meta":{"type":"page","title":"Start","lang":"en-GB"},
            "content":{"document": {"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Next chapter","marks":[{"type":"link","attrs":{"href":format!("/?note={}",ids[1])}}]}]},
                {"type":"image","attrs":{"src":asset_src,"alt":"A sample image"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Chapter one","lang":"en"},
            "content":{"document": {"type":"doc","content":[{"type":"heading","attrs":{"level":2,"id":uuid::Uuid::now_v7().to_string()},"content":[{"type":"text","text":"Details"}]}]}}}),
            None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Source","lang":"en"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted source"}]}]}}}),
            None, "2026-09-28").unwrap();
        write_asset(
            &root,
            &ids[0],
            "image.png",
            &std::fs::read(fixture.join("../one-pixel.png")).unwrap(),
        )
        .unwrap();
        write_item_input(&root, json!({"id":ids[3],"meta":{"type":"publication","title":"Book"},
            "content":{"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"section","id":uuid::Uuid::now_v7().to_string(),"title":"Fundamentals","introPageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        let bundle = render_publication_pdf(&root, &plan).unwrap();
        let source = String::from_utf8(bundle.files["content.typ"].clone()).unwrap();
        // A page's language rule stays inside its own block; unscoped rules
        // nest once per page and overflow Typst on large Publications.
        // Each page also numbers its footnotes from one.
        assert_eq!(
            source
                .matches("#[\n#counter(footnote).update(0)\n#set text(")
                .count(),
            plan.pages.len()
        );
        assert_eq!(source.matches("#set text(").count(), plan.pages.len());
        assert!(source.contains(&format!("#label(\"note-{}\")", ids[1])));
        assert!(source.contains("Quoted source"));
        assert!(source.contains("1. Fundamentals"));
        assert!(source.contains("#heading(level: 3)[#text(\"Details\")]"));
        assert_eq!(source.matches("Details").count(), 1);
        publish_pdf(
            bundle.clone(),
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
                bundle,
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
            "content":{"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"editorial","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let editorial_plan =
            plan_publication(&root, &editorial_id, PublicationFormat::Pdf).unwrap();
        let editorial = render_publication_pdf(&root, &editorial_plan).unwrap();
        publish_pdf(
            editorial,
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
        crate::store::theme_packages::install_fixture(&root, "field-notes", &example);

        let themed_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":themed_id,"meta":{"type":"publication","title":"Field notes book"},
            "content":{"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"field-notes","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let themed_plan = plan_publication(&root, &themed_id, PublicationFormat::Pdf).unwrap();
        assert!(!themed_plan.theme.requires_trust);
        let themed = render_publication_pdf(&root, &themed_plan).unwrap();
        let themed_values = String::from_utf8(themed.files["theme-values.typ"].clone()).unwrap();
        assert!(themed_values.contains("#f2f4f8"));
        publish_pdf(
            themed,
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
            "content":{"kind":"publication","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"accessible","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let accessible_plan =
            plan_publication(&root, &accessible_id, PublicationFormat::Pdf).unwrap();
        let accessible = render_publication_pdf(&root, &accessible_plan).unwrap();
        publish_pdf(
            accessible,
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
                bad,
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
    /// Source preparation only: the benchmark script owns the measured compiler.
    #[test]
    #[ignore = "Run scripts/benchmark-pdf.ps1 explicitly against the generated Fiction project."]
    fn prepare_fiction_memory_benchmark() {
        let root = std::path::PathBuf::from(std::env::var_os("BLUINK_PDF_BENCHMARK_ROOT").unwrap());
        let output =
            std::path::PathBuf::from(std::env::var_os("BLUINK_PDF_BENCHMARK_SOURCE").unwrap());
        let started = std::time::Instant::now();
        let plan = crate::export_jobs::plan::plan_publication(
            &root,
            "8f93d289-83d9-5a19-8431-29843d4698dc",
            PublicationFormat::Pdf,
        )
        .unwrap();
        let planning_seconds = started.elapsed().as_secs_f64();
        let started = std::time::Instant::now();
        let bundle = render_publication_pdf(&root, &plan).unwrap();
        let serialization_seconds = started.elapsed().as_secs_f64();
        std::fs::create_dir(&output).unwrap();
        for (relative, bytes) in &bundle.files {
            let path = output.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let manifest = serde_json::json!({
            "publicationId": plan.publication_id,
            "publicationRevision": plan.publication_revision,
            "sourcePages": plan.pages.len(),
            "planningSeconds": planning_seconds,
            "serializationSeconds": serialization_seconds,
            "assets": plan.assets,
            "sourceHashes": bundle.files.iter().map(|(name, bytes)| {
                use sha2::Digest;
                (name, crate::domain::publication::digest_hex(&sha2::Sha256::digest(bytes)))
            }).collect::<std::collections::BTreeMap<_, _>>(),
        });
        std::fs::write(
            output.join("benchmark-source.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
}
