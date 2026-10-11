// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::engine::project::item::read_project_item;
use crate::engine::publication::PdfProfile;
#[cfg(test)]
use crate::engine::publication::plan::PlannedPage;
use crate::engine::publication::plan::{
    PlannedStructureNode, PublicationFormat, PublicationPlan, assert_plan_fresh,
};
use crate::engine::publication::preflight::LinkResolution;
use crate::engine::publication::source::{
    document, min_heading_level, parse_language, resolved_fields,
};
use crate::engine::publication::style::typst_values;
use crate::engine::publication::theme::load_pdf_theme;
use crate::engine::publication::typst::{
    TypstLink, TypstResources, TypstState, render_typst_document_in_scope,
};

pub(crate) struct PdfBundle {
    pub(crate) shell_path: String,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    pub(crate) page_lines: Vec<(usize, String)>,
}

/// Bundle keys and Typst imports use portable slash paths on every host.
pub(super) fn bundle_sibling(shell: &str, relative: &str) -> String {
    match shell.rsplit_once('/') {
        Some((folder, _)) if !folder.is_empty() => format!("{folder}/{relative}"),
        _ => relative.to_owned(),
    }
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
        command.arg("compile");
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
pub(crate) fn render_publication_pdf(
    root: &Path,
    plan: &PublicationPlan,
    trust_project_theme: bool,
) -> Result<PdfBundle, String> {
    if !matches!(plan.format, PublicationFormat::Pdf) {
        return Err("A PDF renderer needs a PDF Publication plan.".to_owned());
    }
    assert_plan_fresh(root, plan)?;
    let theme = load_pdf_theme(root, &plan.theme, trust_project_theme)?;
    let mut files = theme.files;
    let mut page_items = HashMap::new();
    for page in &plan.pages {
        page_items.insert(
            page.id.as_str(),
            read_project_item(root, &page.id)?
                .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?,
        );
    }
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    let mut content = String::from("#import \"theme-values.typ\": *\n");
    writeln!(content, "#set document(title: {})", quote(&plan.title)?)
        .expect("writing to a String cannot fail");
    let mut book_entries = vec![BookEntry::Page {
        id: plan.root_page_id.clone(),
        number: None,
        intro: false,
    }];
    let mut rendered_page_ids = HashSet::from([plan.root_page_id.clone()]);
    collect_book_entries(
        &plan.structure,
        1,
        &mut Vec::new(),
        &mut rendered_page_ids,
        &mut book_entries,
    );
    let mut page_lines = Vec::new();
    let mut citation_scope = TypstState::default();
    for entry in book_entries {
        if let BookEntry::Section {
            id,
            title,
            number,
            depth,
        } = &entry
        {
            page_lines.push((
                content.bytes().filter(|byte| *byte == b'\n').count() + 1,
                id.clone(),
            ));
            write!(
                content,
                "#pagebreak()\n#heading(level: {}, supplement: [Chapter])[#text({})]\n\n",
                depth,
                quote(&format!("{number}. {title}"))?,
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
        page_lines.push((
            content.bytes().filter(|byte| *byte == b'\n').count() + 1,
            page.id.clone(),
        ));
        let item = page_items
            .get(page.id.as_str())
            .ok_or("Publication page was not captured.")?;
        let language = item["meta"]["lang"].as_str().unwrap_or("en");
        let (lang, region) = parse_language(language)
            .ok_or_else(|| format!("Page {} has invalid language tag {language}.", page.id))?;
        if is_root {
            write!(content,
                "#set text(lang: {}, region: {})\n#v(3cm * spacing)\n#align(center)[#title[#text({})]] <note-{}>\n#v(1cm * spacing)\n\n",
                quote(&lang)?,
                region
                    .as_deref()
                    .map(quote)
                    .transpose()?
                    .unwrap_or_else(|| "none".to_owned()),
                quote(&plan.title)?,
                page.id
            ).expect("writing to a String cannot fail");
        } else if !intro {
            let number = number.as_deref().ok_or("Chapter has no authored number.")?;
            write!(content,
                "#pagebreak()\n#set text(lang: {}, region: {})\n#heading(level: {}, supplement: [Chapter])[#text({})] <note-{}>\n\n",
                quote(&lang)?,
                region.as_deref().map(quote).transpose()?.unwrap_or_else(|| "none".to_owned()),
                page.depth,
                quote(&format!("{number}. {}", page.title))?,
                page.id
            ).expect("writing to a String cannot fail");
        } else {
            write!(
                content,
                "#set text(lang: {}, region: {})\n#metadata(()) <note-{}>\n\n",
                quote(&lang)?,
                region
                    .as_deref()
                    .map(quote)
                    .transpose()?
                    .unwrap_or_else(|| "none".to_owned()),
                page.id,
            )
            .expect("writing to a String cannot fail");
        }
        let links = resolved_links(plan, &page.id)?;
        let assets = resolved_assets(plan);
        let fields = resolved_fields(item, document(item))?;
        let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
        let body = render_typst_document_in_scope(
            document(item),
            &TypstResources {
                links: &links,
                assets: &assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
                macros: Some(&theme.macros),
            },
            i16::try_from(page.depth).map_err(|_| "Publication outline is too deep.")? + 1
                - i16::try_from(min_heading_level(document(item), &exclusions).unwrap_or(1))
                    .map_err(|_| "Page heading level is too large.")?,
            &mut citation_scope,
        )
        .map_err(|error| format!("Page {}: {error}", page.id))?;
        content.push_str(&body);
        if is_root {
            content.push_str("#pagebreak()\n#outline(title: [Contents], target: heading.where(supplement: [Chapter]))\n\n");
        }
    }
    let values_path = bundle_sibling(&theme.shell_path, "theme-values.typ");
    let content_path = bundle_sibling(&theme.shell_path, "content.typ");
    if files.contains_key(&values_path) || files.contains_key(&content_path) {
        return Err("Theme cannot replace generated Publication files.".to_owned());
    }
    files.insert(
        values_path,
        typst_values(&plan.theme.settings)?.into_bytes(),
    );
    files.insert(content_path, content.into_bytes());
    for asset in &plan.assets {
        let path = root
            .join("items")
            .join(&asset.owner_id[..2])
            .join(&asset.owner_id)
            .join("assets")
            .join(&asset.filename);
        let bytes =
            fs::read(&path).map_err(|error| format!("Cannot read asset {}: {error}", asset.src))?;
        let digest = Sha256::digest(&bytes);
        let hash: String = super::digest_hex(&digest);
        if bytes.len() as u64 != asset.bytes || hash != asset.sha256 {
            return Err(format!("Asset {} changed during PDF rendering.", asset.src));
        }
        let relative = bundle_sibling(
            &theme.shell_path,
            &format!("assets/{}/{}", asset.owner_id, asset.filename),
        );
        files.insert(relative, bytes);
    }
    assert_plan_fresh(root, plan)?;
    Ok(PdfBundle {
        shell_path: theme.shell_path,
        files,
        page_lines,
    })
}

fn resolved_links(
    plan: &PublicationPlan,
    source_id: &str,
) -> Result<HashMap<String, TypstLink>, String> {
    let mut links = HashMap::new();
    for link in plan.links.iter().filter(|link| link.page_id == source_id) {
        let destination = match link.resolution {
            LinkResolution::Included => TypstLink::Label(format!(
                "note-{}",
                link.target_id
                    .as_deref()
                    .ok_or("Included link has no target ID.")?
            )),
            LinkResolution::Anchor => {
                let anchor = link
                    .destination
                    .as_deref()
                    .ok_or("Anchor has no destination.")?
                    .strip_prefix('#')
                    .ok_or("Anchor has no fragment.")?;
                if source_id != plan.root_page_id
                    && !plan.pages.iter().any(|page| page.id == source_id)
                {
                    return Err(format!(
                        "Passage page {source_id} has an anchor outside this Publication."
                    ));
                }
                TypstLink::Label(anchor.to_owned())
            }
            LinkResolution::External => TypstLink::Url(
                link.destination
                    .as_deref()
                    .ok_or("Link has no destination.")?
                    .to_owned(),
            ),
            LinkResolution::TextOnly => TypstLink::PlainText,
        };
        if links
            .insert(link.href.clone(), destination.clone())
            .is_some_and(|prior| prior != destination)
        {
            return Err(format!(
                "Source page {source_id} has conflicting link targets."
            ));
        }
    }
    Ok(links)
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

enum BookEntry {
    Section {
        id: String,
        title: String,
        number: String,
        depth: usize,
    },
    Page {
        id: String,
        number: Option<String>,
        intro: bool,
    },
}

fn collect_book_entries(
    nodes: &[PlannedStructureNode],
    depth: usize,
    counts: &mut Vec<usize>,
    rendered_page_ids: &mut HashSet<String>,
    output: &mut Vec<BookEntry>,
) {
    for node in nodes {
        counts.truncate(depth);
        if counts.len() < depth {
            counts.resize(depth, 0);
        }
        counts[depth - 1] += 1;
        let number = counts
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(".");
        match node {
            PlannedStructureNode::Page {
                page_id, children, ..
            } => {
                if rendered_page_ids.insert(page_id.clone()) {
                    output.push(BookEntry::Page {
                        id: page_id.clone(),
                        number: Some(number),
                        intro: false,
                    });
                }
                collect_book_entries(children, depth + 1, counts, rendered_page_ids, output);
            }
            PlannedStructureNode::Section {
                id,
                title,
                intro_page_id,
                children,
            } => {
                output.push(BookEntry::Section {
                    id: id.clone(),
                    title: title.clone(),
                    number,
                    depth,
                });
                if let Some(page_id) = intro_page_id
                    && rendered_page_ids.insert(page_id.clone())
                {
                    output.push(BookEntry::Page {
                        id: page_id.clone(),
                        number: None,
                        intro: true,
                    });
                }
                collect_book_entries(children, depth + 1, counts, rendered_page_ids, output);
            }
            PlannedStructureNode::Collection {
                id,
                title,
                page_ids,
                ..
            } => {
                output.push(BookEntry::Section {
                    id: id.clone(),
                    title: title.clone(),
                    number,
                    depth,
                });
                for (index, page_id) in page_ids.iter().enumerate() {
                    if rendered_page_ids.insert(page_id.clone()) {
                        output.push(BookEntry::Page {
                            id: page_id.clone(),
                            number: Some(format!(
                                "{}.{}",
                                counts
                                    .iter()
                                    .map(usize::to_string)
                                    .collect::<Vec<_>>()
                                    .join("."),
                                index + 1
                            )),
                            intro: false,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn chapter_numbers(pages: &[PlannedPage]) -> Result<HashMap<String, String>, String> {
    let root = pages.first().ok_or("Publication has no root page.")?;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut numbers: HashMap<String, String> = HashMap::new();
    for page in pages.iter().skip(1) {
        let parent = page.parent_id.as_deref().ok_or("Chapter has no parent.")?;
        let next = counts.entry(parent).or_default();
        *next += 1;
        let number = if parent == root.id {
            next.to_string()
        } else {
            format!(
                "{}.{}",
                numbers
                    .get(parent)
                    .ok_or("Chapter parent is out of order.")?,
                next
            )
        };
        numbers.insert(page.id.clone(), number);
    }
    Ok(numbers)
}

pub(crate) fn quote(value: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{
        BookEntry, PdfBundle, chapter_numbers, collect_book_entries, publish_pdf,
        render_publication_pdf, source_for_diagnostic,
    };
    use crate::engine::project::asset::write_asset;
    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_item_input;
    use crate::engine::publication::plan::{
        PlannedPage, PlannedStructureNode, PublicationFormat, plan_publication,
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
    fn numbers_nested_chapters_from_the_authored_outline() {
        let page = |id: &str, parent: Option<&str>, depth| PlannedPage {
            id: id.to_owned(),
            revision: "rev".to_owned(),
            title: id.to_owned(),
            route: format!("pages/{id}.html"),
            published_on: None,
            tags: Vec::new(),
            depth,
            excluded_block_ids: Vec::new(),
            parent_id: parent.map(str::to_owned),
        };
        let pages = [
            page("root", None, 0),
            page("a", Some("root"), 1),
            page("b", Some("a"), 2),
            page("c", Some("a"), 2),
            page("d", Some("root"), 1),
        ];
        let numbers = chapter_numbers(&pages).unwrap();
        assert_eq!(numbers["a"], "1");
        assert_eq!(numbers["b"], "1.1");
        assert_eq!(numbers["c"], "1.2");
        assert_eq!(numbers["d"], "2");
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
        let root = std::env::temp_dir().join(format!("bluink-pdf-{}", uuid::Uuid::now_v7()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::snippets::write_snippet(
            &root,
            "excerpt",
            json!({"type":"paragraph","content":[{"type":"text","text":"Quoted source"}]}),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("properties")).unwrap();
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
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Pdf).unwrap();
        let bundle = render_publication_pdf(&root, &plan, false).unwrap();
        let source = String::from_utf8(bundle.files["pdf/content.typ"].clone()).unwrap();
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
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let editorial_plan =
            plan_publication(&root, &editorial_id, PublicationFormat::Pdf).unwrap();
        let editorial = render_publication_pdf(&root, &editorial_plan, false).unwrap();
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
        for directory in ["html", "site", "pdf"] {
            std::fs::create_dir_all(installed.join(directory)).unwrap();
        }
        for relative in [
            "manifest.json",
            "html/article.html",
            "html/macros.html",
            "site/page.html",
            "pdf/document.typ",
            "pdf/macros.j2",
        ] {
            std::fs::copy(example.join(relative), installed.join(relative)).unwrap();
        }
        let themed_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":themed_id,"meta":{"type":"publication","title":"Field notes book"},
            "content":{"kind":"publication","preset":"book","selectedOutput":"pdf","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"field-notes","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let themed_plan = plan_publication(&root, &themed_id, PublicationFormat::Pdf).unwrap();
        assert!(
            render_publication_pdf(&root, &themed_plan, false)
                .err()
                .unwrap()
                .contains("requires explicit trust")
        );
        let themed = render_publication_pdf(&root, &themed_plan, true).unwrap();
        let themed_source = String::from_utf8(themed.files["pdf/content.typ"].clone()).unwrap();
        assert!(themed_source.contains("#block(inset: 6pt, fill: surface)["));
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
                "html":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let accessible_plan =
            plan_publication(&root, &accessible_id, PublicationFormat::Pdf).unwrap();
        let accessible = render_publication_pdf(&root, &accessible_plan, false).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
    }
}
