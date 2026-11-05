// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt::Write as _;

mod metadata;
mod navigation;

use crate::domain::document::projection::{bounded_preview, project_document};
use crate::domain::publication::{
    html::{
        HtmlResources, HtmlState, escape_html, html_footnotes, prepare_html_document,
        render_prepared_html,
    },
    plan::{
        PlannedCollection, PlannedListing, PlannedStructureNode, PublicationFormat, PublicationPlan,
    },
    preflight::{LinkResolution, PlannedLink},
    source::{document, min_heading_level, resolved_fields},
    theme::{HtmlShellContext, HtmlShellKind},
    theme_format::ResolvedTheme,
};
use crate::export_jobs::{fonts::SiteFonts, plan::assert_plan_fresh};
use crate::store::snapshots::read_project_item;
use metadata::page_metadata;
use navigation::*;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

/// Render-only lookup: portable plans retain their ordered page array.
struct SitePlan<'a> {
    source: &'a PublicationPlan,
    pages_by_id: HashMap<&'a str, &'a crate::domain::publication::plan::PlannedPage>,
}
impl std::ops::Deref for SitePlan<'_> {
    type Target = PublicationPlan;
    fn deref(&self) -> &Self::Target {
        self.source
    }
}
impl SitePlan<'_> {
    fn page(&self, id: &str) -> Option<&crate::domain::publication::plan::PlannedPage> {
        self.pages_by_id.get(id).copied()
    }
}

const SEARCH_SCRIPT: &str = r#"(() => {
  const entries = __BLUINK_SEARCH_INDEX__;
  const form = document.querySelector('[data-publication-search]');
  if (!form) return;
  const input = form.querySelector('input[type="search"]');
  const results = form.querySelector('[data-publication-search-results]');
  const base = form.getAttribute('data-base') || '';
  const render = () => {
    const query = input.value.trim().replace(/\s+/g, ' ').toLowerCase();
    results.replaceChildren();
    if (!query) return;
    const matching = entries.filter(entry => entry.search.includes(query));
    const count = document.createElement('p');
    count.textContent = `${matching.length} matching ${matching.length === 1 ? 'page' : 'pages'}` +
      (matching.length > 30 ? ' (showing first 30)' : '');
    results.append(count);
    const list = document.createElement('ol');
    for (const entry of matching.slice(0, 30)) {
      const item = document.createElement('li');
      const link = document.createElement('a');
      link.href = base + entry.path;
      link.textContent = entry.title;
      item.append(link);
      list.append(item);
    }
    results.append(list);
  };
  input.addEventListener('input', render);
  form.addEventListener('submit', event => event.preventDefault());
  if (new URLSearchParams(location.search).has('q')) {
    input.value = new URLSearchParams(location.search).get('q') || '';
    render();
  }
})();"#;

/// Where a site offers the whole Publication as one PDF, when it does.
pub(crate) const PDF_DOWNLOAD: &str = "publication.pdf";

/// A website written file by file into a hidden folder beside its destination.
/// `publish` renames the complete folder into place. Dropping the stage removes
/// it, so a failed export never exposes a partial website.
pub(crate) struct SiteStage {
    directory: tempfile::TempDir,
    destination: PathBuf,
    files: BTreeSet<String>,
    fonts: Option<SiteFonts>,
}

impl SiteStage {
    /// An existing destination is never replaced implicitly.
    pub(crate) fn new(destination: &Path) -> Result<Self, String> {
        ensure_absent(destination)?;
        let parent = destination
            .parent()
            .filter(|path| path.is_dir())
            .ok_or("Site output parent must be an existing folder.")?;
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Site output needs a folder name.")?;
        let directory = tempfile::Builder::new()
            .prefix(&format!(".{name}.bluink-"))
            .suffix(".tmp")
            .tempdir_in(parent)
            .map_err(|error| format!("Cannot stage site output: {error}"))?;
        Ok(Self {
            directory,
            destination: destination.to_owned(),
            files: BTreeSet::new(),
            fonts: None,
        })
    }

    /// Write the selected fonts now and style every page written after this.
    pub(crate) fn use_fonts(&mut self, theme: &ResolvedTheme, web: &Path) -> Result<(), String> {
        let (fonts, files) = SiteFonts::prepare(theme, web)?;
        for (relative, bytes) in files {
            self.write(&relative, &bytes)?;
        }
        self.fonts = Some(fonts);
        Ok(())
    }

    /// The staged path for `relative`, with its folders created.
    pub(crate) fn path(&mut self, relative: &str) -> Result<PathBuf, String> {
        let path = Path::new(relative);
        if relative.is_empty()
            || relative.contains('\\')
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("Unsafe site output path: {relative}"));
        }
        let output = self.directory.path().join(path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Cannot create site folder: {error}"))?;
        }
        self.files.insert(relative.to_owned());
        Ok(output)
    }

    pub(crate) fn write(&mut self, relative: &str, bytes: &[u8]) -> Result<(), String> {
        let output = self.path(relative)?;
        fs::write(output, bytes)
            .map_err(|error| format!("Cannot write site file {relative}: {error}"))
    }

    fn write_page(&mut self, route: &str, html: String) -> Result<(), String> {
        let html = match &self.fonts {
            Some(fonts) => fonts.style(route, &html)?,
            None => html,
        };
        self.write(route, html.as_bytes())
    }

    pub(crate) fn publish(self) -> Result<(), String> {
        if !self.files.contains("index.html") {
            return Err("Site output has no index.html.".to_owned());
        }
        ensure_absent(&self.destination)?;
        let staging = self.directory.keep();
        fs::rename(&staging, &self.destination).map_err(|error| {
            let _ = fs::remove_dir_all(&staging);
            format!("Cannot publish site output: {error}")
        })
    }

    #[cfg(test)]
    fn contains(&self, relative: &str) -> bool {
        self.files.contains(relative)
    }

    #[cfg(test)]
    fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.directory.path().join(relative)).unwrap_or_default()
    }

    #[cfg(test)]
    fn read_text(&self, relative: &str) -> String {
        String::from_utf8(self.read(relative)).unwrap_or_default()
    }
}

fn ensure_absent(destination: &Path) -> Result<(), String> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(format!(
            "Site output already exists: {}",
            destination.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot inspect site output: {error}")),
    }
}

/// Render a complete site into its stage. Each file is written as soon as it is
/// rendered, so memory does not grow with the site; nothing is visible until
/// `SiteStage::publish`.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
pub(crate) fn render_publication_site(
    root: &Path,
    plan: &PublicationPlan,
    trust_project_theme: bool,
    stage: &mut SiteStage,
) -> Result<(), String> {
    if !matches!(plan.format, PublicationFormat::Site) {
        return Err("A site renderer needs a site Publication plan.".to_owned());
    }
    assert_plan_fresh(root, plan)?;
    let captured = SitePlan {
        source: plan,
        pages_by_id: plan
            .pages
            .iter()
            .map(|page| (page.id.as_str(), page))
            .collect(),
    };
    let plan = &captured;
    let shell =
        crate::export_jobs::theme::prepare_html_shell(root, &plan.theme, trust_project_theme)?;
    let links = links_by_page(&plan.links);
    // Asset URLs differ between pages only by their folder depth.
    let mut assets_by_prefix: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut summaries = HashMap::new();
    let mut search_entries = Vec::with_capacity(plan.pages.len());
    let snippets: HashMap<String, serde_json::Value> = plan
        .snippets
        .iter()
        .map(|snippet| (snippet.id.clone(), snippet.paragraph.clone()))
        .collect();
    for page in &plan.pages {
        let item = read_project_item(root, &page.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Publication page {} disappeared.", page.id))?;
        summaries.insert(page.id.as_str(), page_summary(&item));
        let page_document = document(&item);
        let links = resolved_links(
            plan,
            links.get(page.id.as_str()).map_or(&[][..], Vec::as_slice),
            &page.id,
            &page.id,
        )?;
        let assets = assets_by_prefix
            .entry(root_prefix(&page_file(plan, &page.id)))
            .or_insert_with_key(|prefix| site_assets(plan, prefix));
        let exclusions: HashSet<String> = page.excluded_block_ids.iter().cloned().collect();
        let fields = resolved_fields(&item, page_document, &exclusions)?;
        let heading_offset =
            2 - i16::try_from(min_heading_level(page_document, &exclusions).unwrap_or(1))
                .map_err(|_| "Page heading level is too large.")?;
        let heading = plan.theme.resolved.page.references_heading.as_str();
        let mut notes = HtmlState::default();
        let prepared = prepare_html_document(
            page_document,
            &HtmlResources {
                page_id: &page.id,
                references_heading: heading,
                links: &links,
                assets,
                fields: &fields,
                snippets: &snippets,
                excluded_block_ids: &exclusions,
            },
            heading_offset,
            &mut notes.scope,
        )
        .map_err(|error| format!("Page {}: {error}", page.id))?;
        // Each page lists the sources it cites; search finds them there too.
        let sources = notes.scope.take_list(&page.id, Some((heading, 2)));
        let metadata = page_metadata(&prepared)?;
        let mut searchable = metadata.text.clone();
        if let Some(list) = &sources {
            searchable.push(' ');
            searchable.push_str(&page_metadata(list)?.text);
        }
        search_entries.push(search_entry(plan, page, &searchable));
        let mut body = render_prepared_html(&prepared, &mut notes)?;
        body.push_str(&html_footnotes(&mut notes));
        if let Some(list) = &sources {
            body.push_str(&render_prepared_html(list, &mut notes)?);
        }
        // The home page offers the whole Publication as one PDF when asked to.
        let download = if plan.targets.site.pdf_download && page.id == plan.root_page_id {
            format!(
                "<p class=\"publication-download\"><a href=\"{}\" download=\"{}.pdf\">Download the PDF</a></p>",
                relative_href(&page_file(plan, &page.id), PDF_DOWNLOAD),
                escape_html(&plan.title)
            )
        } else {
            String::new()
        };
        let mut article = format!(
            "<article id=\"note-{}\"><header><h1>{}</h1>{download}</header>\n",
            page.id,
            escape_html(&page.title)
        );
        article.push_str(&body);
        article.push_str("</article>\n");
        let navigation = site_navigation(plan, &page.id)?;
        let title = format!("{} — {}", page.title, plan.title);
        let language = item["meta"]["lang"].as_str().unwrap_or("und");
        let html = shell.render(
            HtmlShellKind::Site,
            HtmlShellContext {
                title: &title,
                language,
                body: &article,
                navigation: &navigation,
                contents: &metadata.contents,
                role: if page.id == plan.root_page_id {
                    "home"
                } else if page.published_on.is_some() {
                    "article"
                } else {
                    "page"
                },
                page: serde_json::json!({"title":page.title,"route":page.route,"publishedOn":page.published_on,"tags":page.tags}),
            },
        )?;
        stage.write_page(&page_file(plan, &page.id), html)?;
    }
    for listing in &plan.listings {
        let body = render_listing(plan, listing, &summaries)?;
        let navigation = site_navigation_at(plan, &listing.route, None)?;
        let title = format!("{} — {}", listing.title, plan.title);
        let html = shell.render(
            HtmlShellKind::Site,
            HtmlShellContext {
                title: &title,
                language: "en",
                body: &body,
                navigation: &navigation,
                contents: "",
                role: if listing.route.contains("/archive/") || listing.route.contains("/tag/") {
                    "archive"
                } else {
                    "listing"
                },
                page: listing_context(plan, listing, &summaries)?,
            },
        )?;
        stage.write_page(&listing.route, html)?;
    }
    for collection in plan.collections.iter().filter(|collection| collection.feed) {
        let slug = collection
            .listing_slug
            .as_deref()
            .ok_or("Feed collection has no listing address.")?;
        stage.write(
            &format!("{slug}/feed.xml"),
            render_feed(plan, collection, &summaries)?.as_bytes(),
        )?;
    }
    if let Some(base_url) = &plan.targets.site.base_url {
        let sitemap = render_sitemap(base_url, &stage.files);
        stage.write("sitemap.xml", sitemap.as_bytes())?;
    }
    stage.write("search.js", search_script(&search_entries)?.as_bytes())?;
    for asset in &plan.assets {
        let path = stage.path(&format!("assets/{}/{}", asset.owner_id, asset.filename))?;
        let mut output = fs::File::create(&path)
            .map_err(|error| format!("Cannot write site asset {}: {error}", asset.src))?;
        crate::export_jobs::resources::copy_asset(root, asset, &mut output)?;
    }
    assert_plan_fresh(root, plan)?;
    Ok(())
}

fn listing_page_route(base: &str, number: usize) -> String {
    if number == 1 {
        format!("{base}/index.html")
    } else {
        format!("{base}/page/{number}/index.html")
    }
}

fn page_summary(source: &Value) -> String {
    if let Some(summary) = source["meta"]["summary"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return summary.to_owned();
    }
    bounded_preview(&project_document(document(source)).text)
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn render_listing(
    plan: &SitePlan<'_>,
    listing: &PlannedListing,
    summaries: &HashMap<&str, String>,
) -> Result<String, String> {
    let collection = plan
        .collections
        .iter()
        .find(|collection| collection.id == listing.collection_id)
        .ok_or("Listing has no planned collection.")?;
    let mut body = format!(
        "<section class=\"publication-listing\"><h1>{}</h1>",
        escape_html(&listing.title)
    );
    if listing.page_ids.is_empty() {
        body.push_str("<p>No pages match this collection yet.</p>");
    } else {
        body.push_str("<ol class=\"publication-card-list\">");
        for id in &listing.page_ids {
            let page = plan.page(id).ok_or("Listing page was not captured.")?;
            let summary = summaries
                .get(id.as_str())
                .ok_or("Listing source was not captured.")?;
            write!(
                body,
                "<li><article><h2><a href=\"{}\">{}</a></h2>",
                relative_href(&listing.route, &page.route),
                escape_html(&page.title)
            )
            .expect("writing to a String cannot fail");
            if let Some(date) = &page.published_on {
                write!(
                    body,
                    "<time datetime=\"{}\">{}</time>",
                    escape_html(date),
                    escape_html(date)
                )
                .expect("writing to a String cannot fail");
            }
            if !summary.is_empty() {
                write!(body, "<p>{}</p>", escape_html(summary))
                    .expect("writing to a String cannot fail");
            }
            if collection.archives && !page.tags.is_empty() {
                let slug = collection
                    .listing_slug
                    .as_deref()
                    .ok_or("Archive collection has no listing address.")?;
                body.push_str("<ul class=\"publication-card-tags\">");
                for tag in &page.tags {
                    write!(
                        body,
                        "<li><a href=\"{}\">{}</a></li>",
                        relative_href(&listing.route, &format!("{slug}/tag/{tag}/index.html")),
                        escape_html(tag)
                    )
                    .expect("writing to a String cannot fail");
                }
                body.push_str("</ul>");
            }
            body.push_str("</article></li>");
        }
        body.push_str("</ol>");
    }
    if listing.total_pages > 1 {
        body.push_str("<nav class=\"publication-pagination\" aria-label=\"Listing pages\">");
        for number in 1..=listing.total_pages {
            write!(
                body,
                "<a href=\"{}\"{}>{number}</a>",
                relative_href(
                    &listing.route,
                    &listing_page_route(&listing.base_path, number)
                ),
                if number == listing.page_number {
                    " aria-current=\"page\""
                } else {
                    ""
                }
            )
            .expect("writing to a String cannot fail");
        }
        body.push_str("</nav>");
    }
    if listing.base_path == collection.listing_slug.as_deref().unwrap_or("")
        && listing.page_number == 1
        && collection.archives
    {
        let base = collection
            .listing_slug
            .as_deref()
            .ok_or("Archive collection has no listing address.")?;
        let archives = plan
            .listings
            .iter()
            .filter(|entry| {
                entry.page_number == 1 && entry.base_path.starts_with(&format!("{base}/archive/"))
            })
            .collect::<Vec<_>>();
        if !archives.is_empty() {
            body.push_str("<aside class=\"publication-archive-links\"><h2>Archives</h2><ul>");
            for archive in archives {
                write!(
                    body,
                    "<li><a href=\"{}\">{}</a></li>",
                    relative_href(&listing.route, &archive.route),
                    escape_html(&archive.title)
                )
                .expect("writing to a String cannot fail");
            }
            body.push_str("</ul></aside>");
        }
    }
    body.push_str("</section>");
    Ok(body)
}

fn listing_context(
    plan: &SitePlan<'_>,
    listing: &PlannedListing,
    summaries: &HashMap<&str, String>,
) -> Result<Value, String> {
    let collection = plan
        .collections
        .iter()
        .find(|collection| collection.id == listing.collection_id)
        .ok_or("Listing has no planned collection.")?;
    let mut entries = Vec::new();
    for id in &listing.page_ids {
        let page = plan.page(id).ok_or("Listing page was not captured.")?;
        let summary = summaries
            .get(id.as_str())
            .ok_or("Listing source was not captured.")?;
        let tags = if collection.archives {
            page.tags.iter().map(|tag| serde_json::json!({"name":tag,"href":relative_href(&listing.route, &format!("{}/tag/{tag}/index.html", collection.listing_slug.as_deref().unwrap_or("")))})).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        entries.push(serde_json::json!({"title":page.title,"href":relative_href(&listing.route, &page.route),"summary":summary,"publishedOn":page.published_on,"tags":tags}));
    }
    let pagination = (1..=listing.total_pages).map(|number| serde_json::json!({"number":number,"href":relative_href(&listing.route, &listing_page_route(&listing.base_path, number)),"current":number == listing.page_number})).collect::<Vec<_>>();
    let archives = if listing.base_path == collection.listing_slug.as_deref().unwrap_or("")
        && listing.page_number == 1
        && collection.archives
    {
        let base = collection.listing_slug.as_deref().unwrap_or("");
        plan.listings.iter().filter(|entry| entry.page_number == 1 && entry.base_path.starts_with(&format!("{base}/archive/")))
            .map(|entry| serde_json::json!({"title":entry.title,"href":relative_href(&listing.route, &entry.route)})).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(
        serde_json::json!({"title":listing.title,"entries":entries,"pagination":pagination,"archives":archives,"collectionId":listing.collection_id}),
    )
}

fn canonical_url(base: &str, route: &str) -> String {
    let path = route.strip_suffix("index.html").unwrap_or(route);
    format!("{base}{path}")
}

fn escape_xml(source: &str) -> String {
    source
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn render_feed(
    plan: &SitePlan<'_>,
    collection: &PlannedCollection,
    summaries: &HashMap<&str, String>,
) -> Result<String, String> {
    let base = plan
        .targets
        .site
        .base_url
        .as_deref()
        .ok_or("Blog feed needs a site address.")?;
    let slug = collection
        .listing_slug
        .as_deref()
        .ok_or("Blog feed has no listing address.")?;
    let mut ids = collection.page_ids.clone();
    ids.sort_by(|a, b| {
        let left = plan
            .page(a)
            .and_then(|page| page.published_on.as_deref())
            .unwrap_or("");
        let right = plan
            .page(b)
            .and_then(|page| page.published_on.as_deref())
            .unwrap_or("");
        right.cmp(left).then_with(|| a.cmp(b))
    });
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss version=\"2.0\"><channel><title>{}</title><link>{}</link><description>{}</description>",
        escape_xml(&collection.title),
        escape_xml(&canonical_url(base, &format!("{slug}/index.html"))),
        escape_xml(&plan.title)
    );
    for id in ids.iter().take(20) {
        let page = plan.page(id).ok_or("Feed page was not planned.")?;
        let summary = summaries
            .get(id.as_str())
            .ok_or("Feed source was not captured.")?;
        let date = page
            .published_on
            .as_deref()
            .ok_or("Feed page needs a publication date.")?;
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|error| error.to_string())?;
        let timestamp = date
            .and_hms_opt(0, 0, 0)
            .ok_or("Invalid feed publication date.")?;
        let pub_date =
            chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(timestamp, chrono::Utc)
                .to_rfc2822();
        let url = canonical_url(base, &page.route);
        write!(xml, "<item><title>{}</title><link>{}</link><guid isPermaLink=\"true\">{}</guid><pubDate>{}</pubDate><description>{}</description></item>",
            escape_xml(&page.title), escape_xml(&url), escape_xml(&url), escape_xml(&pub_date), escape_xml(summary)).expect("writing to a String cannot fail");
    }
    xml.push_str("</channel></rss>");
    Ok(xml)
}
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
)]
#[expect(
    clippy::expect_used,
    reason = "Formatting only strings and primitive values into String is infallible."
)]
fn render_sitemap(base: &str, files: &BTreeSet<String>) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
    );
    for route in files.iter().filter(|route| route.ends_with(".html")) {
        write!(
            xml,
            "<url><loc>{}</loc></url>",
            escape_xml(&canonical_url(base, route))
        )
        .expect("writing to a String cannot fail");
    }
    xml.push_str("</urlset>");
    xml
}

#[expect(
    clippy::expect_used,
    reason = "Site planning captures a route for every included page before rendering references."
)]
fn page_file(plan: &SitePlan<'_>, id: &str) -> String {
    plan.page(id)
        .map(|page| page.route.clone())
        .expect("every included page has a planned route")
}

fn page_href(plan: &SitePlan<'_>, from_id: &str, to_id: &str) -> String {
    relative_href(&page_file(plan, from_id), &page_file(plan, to_id))
}

fn root_prefix(from: &str) -> String {
    "../".repeat(from.split('/').count().saturating_sub(1))
}

fn relative_href(from: &str, to: &str) -> String {
    let from_parts = from.split('/').collect::<Vec<_>>();
    let to_parts = to.split('/').collect::<Vec<_>>();
    let dirs = &from_parts[..from_parts.len().saturating_sub(1)];
    let mut common = 0;
    while common < dirs.len() && common < to_parts.len() && dirs[common] == to_parts[common] {
        common += 1;
    }
    format!(
        "{}{}",
        "../".repeat(dirs.len() - common),
        to_parts[common..].join("/")
    )
}

fn links_by_page(links: &[PlannedLink]) -> HashMap<&str, Vec<&PlannedLink>> {
    let mut grouped: HashMap<&str, Vec<&PlannedLink>> = HashMap::new();
    for link in links {
        grouped.entry(link.page_id.as_str()).or_default().push(link);
    }
    grouped
}

fn resolved_links(
    plan: &SitePlan<'_>,
    source_links: &[&PlannedLink],
    source_id: &str,
    host_id: &str,
) -> Result<HashMap<String, String>, String> {
    let mut links = HashMap::new();
    for link in source_links {
        let target = match link.resolution {
            LinkResolution::Included => page_href(
                plan,
                host_id,
                link.target_id
                    .as_deref()
                    .ok_or("Included link has no target ID.")?,
            ),
            LinkResolution::TextOnly => String::new(),
            LinkResolution::Anchor if link.target_id.as_deref() != Some(host_id) => {
                let owner = link
                    .target_id
                    .as_deref()
                    .ok_or("Anchor has no owner page.")?;
                if plan.page(owner).is_none() {
                    return Err(format!("Anchor page {owner} is outside this Publication."));
                }
                format!(
                    "{}{}",
                    page_href(plan, host_id, owner),
                    link.destination
                        .as_deref()
                        .ok_or("Anchor has no destination.")?
                )
            }
            _ => link
                .destination
                .as_deref()
                .ok_or("Planned link has no destination.")?
                .to_owned(),
        };
        if links
            .insert(link.href.clone(), target.clone())
            .is_some_and(|prior| prior != target)
        {
            return Err(format!(
                "Source page {source_id} has conflicting link targets."
            ));
        }
    }
    Ok(links)
}

fn site_assets(plan: &SitePlan<'_>, prefix: &str) -> HashMap<String, String> {
    plan.assets
        .iter()
        .map(|asset| {
            (
                asset.src.clone(),
                format!("{prefix}assets/{}/{}", asset.owner_id, asset.filename),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use serde_json::{Value, json};

    use super::{SiteStage, page_summary, render_publication_site};
    use crate::domain::publication::plan::PublicationPlan;

    /// Render into a stage beside the project, for reading the staged files.
    fn render(root: &Path, plan: &PublicationPlan, trusted: bool) -> Result<SiteStage, String> {
        let mut stage = SiteStage::new(&root.join("site"))?;
        render_publication_site(root, plan, trusted, &mut stage)?;
        Ok(stage)
    }
    use crate::application::item_writes::write_asset;
    use crate::application::item_writes::write_item_input;
    use crate::domain::project::config::create_project_config;
    use crate::domain::publication::plan::PublicationFormat;
    use crate::export_jobs::one_html::render_publication_html;
    use crate::export_jobs::pdf::render_publication_pdf;
    use crate::export_jobs::plan::plan_publication;
    use crate::export_jobs::plan::plan_publication_resolved;

    #[test]
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "Canonical resource filenames and generated routes are deliberately case sensitive."
    )]
    fn every_site_layout_can_render_every_output_format() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let home = uuid::Uuid::now_v7().to_string();
        let chapter = uuid::Uuid::now_v7().to_string();
        for (id, title, body) in [
            (&home, "Home", "Home content"),
            (&chapter, "Chapter", "Chapter content"),
        ] {
            write_item_input(&root, json!({"id":id,"meta":{"type":"page","title":title,"lang":"en"},
                "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":body}]}]}}}),
                None, "2026-09-28").unwrap();
        }
        for layout in ["documentation", "website"] {
            let id = uuid::Uuid::now_v7().to_string();
            write_item_input(&root, json!({"id":id,"meta":{"type":"publication","title":format!("{layout} sample")},
                "content":{"kind":"publication","selectedOutput":"site","rootPageId":home,
                    "outline":[{"kind":"page","pageId":chapter,"children":[]}],
                    "selectionRules":[],"themeId":"classic",
                    "targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                        "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                        "site":{"layout":layout,"outsideLinks":"error","pdfDownload":layout == "website"}}}}),
                None, "2026-09-28").unwrap();
            for format in [
                PublicationFormat::Pdf,
                PublicationFormat::Docx,
                PublicationFormat::Epub,
                PublicationFormat::Html,
                PublicationFormat::Site,
            ] {
                let plan = plan_publication(&root, &id, format).unwrap();
                assert_eq!(plan.pages.len(), 2, "{layout} {format:?}");
                match format {
                    PublicationFormat::Epub => {
                        let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets");
                        crate::export_jobs::epub::render_publication(
                            &root,
                            &plan,
                            &fonts.join("web"),
                            Some(&fonts.join("pdf")),
                        )
                        .unwrap();
                    }
                    PublicationFormat::Docx => {
                        let fonts =
                            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/docx");
                        let word = crate::export_jobs::word::render_publication(
                            &root, &plan, &fonts, None,
                        )
                        .unwrap();
                        assert!(word.bytes.starts_with(b"PK"), "{layout} DOCX");
                    }
                    PublicationFormat::Pdf => {
                        let pdf = render_publication_pdf(&root, &plan).unwrap();
                        let source = String::from_utf8(pdf.files["content.typ"].clone()).unwrap();
                        assert!(source.contains("Chapter content"), "{layout} PDF");
                    }
                    PublicationFormat::Html => {
                        let html = render_publication_html(&root, &plan, false).unwrap();
                        assert_eq!(
                            html.matches("<article id=\"note-").count(),
                            2,
                            "{layout} HTML"
                        );
                    }
                    PublicationFormat::Site => {
                        let site = render(&root, &plan, false).unwrap();
                        assert!(site.contains("index.html"), "{layout} site");
                        let home = site.read_text("index.html");
                        assert_eq!(
                            home.contains(
                                "href=\"publication.pdf\" download=\"website sample.pdf\""
                            ),
                            layout == "website",
                            "{layout} PDF download"
                        );
                        assert_eq!(
                            site.files
                                .iter()
                                .filter(|route| route.ends_with(".html"))
                                .count(),
                            2,
                            "{layout} site"
                        );
                    }
                }
            }
        }
        temporary.close().unwrap();
    }

    #[test]
    fn renders_ordered_site_with_links_passages_and_assets() {
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
                {"type":"paragraph","content":[{"type":"text","text":"Jump to details","marks":[{"type":"link","attrs":{"href":format!("#block-{}",ids[1])}}]}]},
                {"type":"image","attrs":{"src":asset_src,"alt":"A sample image"}},
                {"type":"embed","attrs":{"snippetId":"excerpt"}}
            ]}}}), None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[1],"meta":{"type":"page","title":"Chapter <one>"},
            "content":{"document": {"type":"doc","content":[{"type":"heading","attrs":{"id":ids[1],"level":2},"content":[{"type":"text","text":"Details"}]}]}}}),
            None, "2026-09-28").unwrap();
        write_item_input(&root, json!({"id":ids[2],"meta":{"type":"page","title":"Source"},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","attrs":{"labels":["excerpt"]},"content":[{"type":"text","text":"Quoted source"}]}]}}}),
            None, "2026-09-28").unwrap();
        write_asset(&root, &ids[0], "image.png", b"image bytes").unwrap();
        write_item_input(&root, json!({"id":ids[3],"meta":{"type":"publication","title":"Book"},
            "content":{"kind":"publication","selectedOutput":"site","rootPageId":ids[0],"outline":[{"kind":"section","id":uuid::Uuid::now_v7().to_string(),"title":"Fundamentals","children":[{"kind":"page","pageId":ids[1],"children":[]}]}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let plan = plan_publication(&root, &ids[3], PublicationFormat::Site).unwrap();
        let site = render(&root, &plan, false).unwrap();
        let index = site.read_text("index.html");
        let chapter = site.read_text(&format!("pages/{}.html", ids[1]));
        assert!(index.contains(&format!("href=\"pages/{}.html\"", ids[1])));
        assert!(
            index.contains(&format!("href=\"pages/{}.html#block-{}\"", ids[1], ids[1])),
            "{index}"
        );
        assert!(index.contains("Quoted source"));
        assert!(index.contains("Fundamentals"));
        assert!(index.contains("src=\"assets/"));
        assert!(index.contains("--publication-ink: #222222;"));
        assert!(chapter.contains("Chapter &lt;one&gt;"));
        assert!(chapter.contains("<h2"));
        assert!(chapter.contains("aria-label=\"On this page\""));
        assert!(chapter.contains(">Details</a>"));
        assert!(chapter.contains("src=\"../search.js\""));
        assert!(index.contains("src=\"search.js\""));
        let search = site.read_text("search.js");
        assert!(search.contains("next chapter"));
        assert!(
            search.contains("quoted source"),
            "Expanded snippets must be searchable"
        );
        assert!(search.contains("Chapter <one>"));
        assert!(chapter.contains("href=\"../index.html\""));
        assert_eq!(
            site.read(&format!("assets/{}/image.png", ids[0])),
            b"image bytes"
        );
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/themes/field-notes");
        crate::store::theme_packages::install_fixture(&root, "field-notes", &example);

        let themed_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":themed_id,"meta":{"type":"publication","title":"Field notes"},
            "content":{"kind":"publication","selectedOutput":"site","rootPageId":ids[0],"outline":[{"kind":"page","pageId":ids[1],"children":[]}],
                "selectionRules":[],"themeId":"field-notes","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"documentation","outsideLinks":"error"}}}}), None, "2026-09-28").unwrap();
        let themed_plan = plan_publication(&root, &themed_id, PublicationFormat::Site).unwrap();
        assert!(
            render(&root, &themed_plan, false)
                .err()
                .unwrap()
                .contains("requires explicit trust")
        );
        let themed = render(&root, &themed_plan, true).unwrap();
        let themed_index = themed.read_text("index.html");
        assert!(themed_index.contains("publication--field-notes"));
        assert!(themed_index.contains("padding: 0.500000rem"));
        assert!(themed_index.contains("Quoted source</p>"));
        assert!(themed_index.contains("aria-label=\"Chapters\""));
        temporary.close().unwrap();
    }

    #[test]
    fn renders_a_website_with_paginated_blog_archives_and_feed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
        let ids: Vec<_> = (0..4).map(|_| uuid::Uuid::now_v7().to_string()).collect();
        for (index, id) in ids.iter().take(3).enumerate() {
            let meta = if index == 0 {
                json!({"type":"page","title":"Home","lang":"en"})
            } else {
                json!({"type":"page","title":format!("Post {index}"),"lang":"en",
                    "summary":format!("Summary {index}"),"published_on":format!("2026-09-0{index}"),
                    "web_slug":format!("post-{index}")})
            };
            write_item_input(&root, json!({"id":id,"meta":meta,"tags":if index == 0 {json!([])} else {json!(["astronomy"])},
                "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":format!("Body {index}")}]}]}}}), None, "2026-09-28").unwrap();
        }
        let collection_id = uuid::Uuid::now_v7().to_string();
        write_item_input(&root, json!({"id":ids[3],"meta":{"type":"publication","title":"Astronomy site"},
            "content":{"kind":"publication","selectedOutput":"site","rootPageId":ids[0],
                "outline":[{"kind":"collection","id":collection_id,"title":"Articles","query":{"version":1,"clauses":[{"polarity":"required","predicate":{"kind":"tag","key":"astronomy"}}],"sort":[{"field":"published_on","direction":"desc"}]},
                    "listingSlug":"blog","pageSize":1,"feed":true,"archives":true,"showPagesInNavigation":false}],
                "selectionRules":[],"themeId":"classic","targets":{"pdf":{"profile":"default","outsideLinks":"error"},
                "html":{"outsideLinks":"error"},"docx":{"outsideLinks":"error"},"epub":{"outsideLinks":"error"},
                "site":{"layout":"website","outsideLinks":"error","baseUrl":"https://example.test/"}}}}), None, "2026-09-28").unwrap();
        let resolved = HashMap::from([(collection_id, vec![ids[2].clone(), ids[1].clone()])]);
        let plan =
            plan_publication_resolved(&root, &ids[3], PublicationFormat::Site, &resolved).unwrap();
        assert_eq!(plan.listings.len(), 8);
        let site = render(&root, &plan, false).unwrap();
        for route in [
            "index.html",
            "pages/post-1/index.html",
            "pages/post-2/index.html",
            "blog/index.html",
            "blog/page/2/index.html",
            "blog/archive/2026/index.html",
            "blog/archive/2026/09/index.html",
            "blog/tag/astronomy/index.html",
            "blog/feed.xml",
            "sitemap.xml",
        ] {
            assert!(site.contains(route), "Missing {route}");
        }
        let listing = site.read_text("blog/index.html");
        assert!(listing.contains("Post 2"));
        assert!(listing.contains("post-2&#x2f;index.html"));
        assert!(listing.contains("Summary 2"));
        let feed = site.read_text("blog/feed.xml");
        assert!(feed.contains("https://example.test/pages/post-2/"));
        assert!(feed.contains("<description>Summary 2</description>"));
        assert!(feed.find("Post 2").unwrap() < feed.find("Post 1").unwrap());
        let sitemap = site.read_text("sitemap.xml");
        assert!(sitemap.contains("https://example.test/blog/"));
        let pdf_plan =
            plan_publication_resolved(&root, &ids[3], PublicationFormat::Pdf, &resolved).unwrap();
        assert_eq!(pdf_plan.pages.len(), 3);
        assert!(render_publication_pdf(&root, &pdf_plan).is_ok());
        let html_plan =
            plan_publication_resolved(&root, &ids[3], PublicationFormat::Html, &resolved).unwrap();
        let html = render_publication_html(&root, &html_plan, false).unwrap();
        assert!(html.find("Body 2").unwrap() < html.find("Body 1").unwrap());
        assert_eq!(html.matches("<article id=\"note-").count(), 3);
        temporary.close().unwrap();
    }

    #[test]
    fn page_summary_prefers_authored_text_and_falls_back_to_document_preview() {
        let authored = serde_json::json!({
            "meta":{"summary":"  Authored summary  "},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Document text"}]}]}}
        });
        assert_eq!(page_summary(&authored), "Authored summary");

        let generated = serde_json::json!({
            "meta":{},
            "content":{"document": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Document text"}]}]}}
        });
        assert_eq!(page_summary(&generated), "Document text");
    }

    #[test]
    fn publishes_atomically_and_never_replaces_an_existing_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("book");
        let mut stage = SiteStage::new(&destination).unwrap();
        stage.write("index.html", b"<h1>Book</h1>").unwrap();
        stage.write("pages/one.html", b"<h1>One</h1>").unwrap();
        // Staged files stay hidden until the whole site is published.
        assert!(!destination.exists());
        stage.publish().unwrap();
        assert_eq!(
            std::fs::read(destination.join("pages/one.html")).unwrap(),
            b"<h1>One</h1>"
        );
        let mut invalid = SiteStage::new(&root.join("invalid")).unwrap();
        assert!(
            invalid
                .write("../escape", b"bad")
                .unwrap_err()
                .contains("Unsafe")
        );
        drop(invalid);
        assert!(!root.join("escape").exists());
        assert!(!root.join("invalid").exists());
        assert!(
            SiteStage::new(&root.join("empty"))
                .unwrap()
                .publish()
                .unwrap_err()
                .contains("no index.html")
        );
        assert!(
            SiteStage::new(&destination)
                .err()
                .unwrap()
                .contains("already exists")
        );
        // A destination created while staging is not replaced either.
        let racing = root.join("racing");
        let mut stage = SiteStage::new(&racing).unwrap();
        stage.write("index.html", b"new").unwrap();
        std::fs::create_dir(&racing).unwrap();
        assert!(stage.publish().unwrap_err().contains("already exists"));
        assert_eq!(
            std::fs::read(destination.join("index.html")).unwrap(),
            b"<h1>Book</h1>"
        );
        // Every stage was removed: only the published and racing folders remain.
        let mut left = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(left, ["book", "racing"]);
        temporary.close().unwrap();
    }
}
