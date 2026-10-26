// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed planning and rendering shared by the application, CLI and HTTP jobs.
mod artifacts;
pub(crate) mod jobs;
pub(crate) mod model;
mod preview;
mod scheduler;
use std::collections::HashMap;
use std::path::Path;

use crate::engine::publication::{self, OutlineNode, OutsideLinks, PdfProfile};
use publication::page::PagePlan;
use publication::plan::{PublicationFormat, PublicationPlan};
use publication::theme::ThemeSnapshot;
use serde::Serialize;

use crate::application::error::{Result, internal_error, invalid_request};
use crate::indexing::service;
use crate::{query, search};

pub(crate) struct ResolvedInputs {
    pub(crate) publication_revision: String,
    pub(crate) collections: HashMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Generation {
    pub(crate) output: String,
    pub(crate) pages: usize,
    pub(crate) assets: usize,
    pub(crate) theme_sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArticleGeneration {
    pub(crate) output: Option<String>,
    pub(crate) html: Option<String>,
    pub(crate) page_id: String,
    pub(crate) assets: usize,
    pub(crate) theme_sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteGeneration {
    pub(crate) output: String,
    pub(crate) page_id: String,
    pub(crate) assets: usize,
    pub(crate) theme_sha256: String,
}

pub(crate) struct Resources<'a> {
    pub(crate) web_fonts: Option<&'a str>,
    pub(crate) pdf_fonts: Option<&'a str>,
    pub(crate) docx_fonts: Option<&'a str>,
    pub(crate) typst: Option<&'a str>,
}

fn resource<'a>(value: Option<&'a str>, name: &str) -> Result<&'a Path> {
    value
        .map(Path::new)
        .ok_or_else(|| internal_error(format!("The installed {name} resource is unavailable.")))
}

fn collection_queries<'a>(
    nodes: &'a [OutlineNode],
    output: &mut Vec<(&'a str, &'a query::document::QueryDocument)>,
) {
    for node in nodes {
        match node {
            OutlineNode::Page { children, .. } | OutlineNode::Section { children, .. } => {
                collection_queries(children, output);
            }
            OutlineNode::Collection { id, query, .. } => output.push((id, query)),
        }
    }
}

fn publication_source(root: &Path, id: &str) -> Result<(String, publication::PublicationContent)> {
    if !crate::engine::project::identity::is_item_id(id) {
        return Err(invalid_request(format!("Invalid Bluink item id: {id}")));
    }
    let item = crate::engine::project::item::read_project_item(root, id)?
        .filter(|item| item["meta"]["type"] == "publication")
        .ok_or_else(|| invalid_request(format!("Publication {id} does not exist.")))?;
    let revision = item["revision"]
        .as_str()
        .ok_or_else(|| internal_error("Publication has no revision."))?
        .to_owned();
    let content = publication::validate_publication(&item["content"]).map_err(internal_error)?;
    Ok((revision, content))
}

pub(crate) fn needs_index(root: &Path, id: &str) -> Result<bool> {
    let (_, content) = publication_source(root, id)?;
    let mut queries = Vec::new();
    collection_queries(&content.outline, &mut queries);
    Ok(!queries.is_empty())
}

pub(crate) fn resolve_inputs(root: &Path, id: &str, index: Option<&str>) -> Result<ResolvedInputs> {
    let (publication_revision, content) = publication_source(root, id)?;
    let mut queries = Vec::new();
    collection_queries(&content.outline, &mut queries);
    let mut collections = HashMap::new();
    if !queries.is_empty() {
        let index = index
            .ok_or_else(|| invalid_request("Publication collections need a ready search index."))?;
        let (snapshot, cache) = service::planning_index(index)?;
        let limit = u32::try_from(snapshot.reader.searcher().num_docs())
            .map_err(|_| invalid_request("Publication query has too many indexed items."))?;
        for (id, source) in queries {
            let mut ordered = source.clone();
            if ordered.sort.is_empty() {
                ordered.sort.push(query::document::QuerySort {
                    field: "title".into(),
                    direction: query::document::SortDirection::Asc,
                });
            }
            let scoped = query::document::QueryDocument::active_pages()
                .compose(&ordered)
                .map_err(|errors| invalid_request(&errors[0].message))?;
            let response =
                search::search_snapshot(&snapshot, &cache, &scoped, limit, 0, None, true)?;
            if response.hits.len() != response.count {
                return Err(invalid_request(
                    "Publication query results changed while planning.",
                ));
            }
            collections.insert(
                id.to_owned(),
                response.hits.into_iter().map(|hit| hit.id).collect(),
            );
        }
    }
    Ok(ResolvedInputs {
        publication_revision,
        collections,
    })
}

pub(crate) fn plan_resolved(
    root: &Path,
    id: &str,
    format: PublicationFormat,
    inputs: &ResolvedInputs,
) -> Result<PublicationPlan> {
    let plan = publication::plan::plan_publication_resolved(root, id, format, &inputs.collections)
        .map_err(internal_error)?;
    if plan.publication_revision != inputs.publication_revision {
        return Err(invalid_request(
            "Publication changed while resolving its collections. Generate again.",
        ));
    }
    Ok(plan)
}

pub(crate) fn plan_publication(
    root: &Path,
    id: &str,
    format: PublicationFormat,
    index: Option<&str>,
) -> Result<PublicationPlan> {
    plan_resolved(root, id, format, &resolve_inputs(root, id, index)?)
}

pub(crate) fn plan_linked_pages(
    root: &Path,
    page: &str,
    format: PublicationFormat,
    theme: &str,
    outside: OutsideLinks,
    profile: PdfProfile,
) -> Result<PublicationPlan> {
    publication::plan::plan_linked_pages(root, page, format, theme, outside, profile)
        .map_err(internal_error)
}

pub(crate) fn trusted_theme(theme: &ThemeSnapshot, hash: Option<&str>) -> Result<bool> {
    if theme.requires_trust {
        if hash != Some(&theme.execution_sha256) {
            return Err(invalid_request(format!(
                "Project theme {} requires approval for web code hash {}.",
                theme.id, theme.execution_sha256
            )));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(crate) fn render_publication(
    root: &Path,
    plan: &PublicationPlan,
    output: &Path,
    hash: Option<&str>,
    index: Option<&str>,
    resources: &Resources<'_>,
) -> Result<Generation> {
    if !plan.collections.is_empty() {
        let current = resolve_inputs(root, &plan.publication_id, index)?;
        for collection in &plan.collections {
            if current.collections.get(&collection.id) != Some(&collection.resolved_ids) {
                return Err(invalid_request(format!(
                    "Collection {} changed since generation started. Generate again to include its current pages.",
                    collection.title
                )));
            }
        }
    }
    let trusted = trusted_theme(&plan.theme, hash)?;
    match plan.format {
        PublicationFormat::Epub => {
            let bundle = publication::epub::render_publication(
                root,
                plan,
                resource(resources.web_fonts, "web fonts")?,
                resources.pdf_fonts.map(Path::new),
            )
            .map_err(internal_error)?;
            publication::epub::publish(&bundle, output).map_err(internal_error)?;
        }
        PublicationFormat::Docx => {
            let bundle = publication::word::render_publication(
                root,
                plan,
                resource(resources.docx_fonts, "Word fonts")?,
                resources.pdf_fonts.map(Path::new),
            )
            .map_err(internal_error)?;
            publication::docx::publish_docx(&bundle, output).map_err(internal_error)?;
        }
        PublicationFormat::Pdf => {
            let bundle =
                publication::pdf::render_publication_pdf(root, plan).map_err(internal_error)?;
            publication::pdf::publish_pdf(
                &bundle,
                plan.targets.pdf.profile,
                output,
                resource(resources.typst, "Typst")?,
                resource(resources.pdf_fonts, "PDF fonts")?,
            )
            .map_err(internal_error)?;
        }
        PublicationFormat::Html => {
            let html = publication::one_html::render_publication_html(root, plan, trusted)
                .map_err(internal_error)?;
            let html = publication::fonts::embed_html(
                &html,
                &plan.theme.resolved,
                resource(resources.web_fonts, "web fonts")?,
            )
            .map_err(internal_error)?;
            publication::one_html::publish_publication_html(&html, output)
                .map_err(internal_error)?;
        }
        PublicationFormat::Site => {
            let mut site = publication::site::render_publication_site(root, plan, trusted)
                .map_err(internal_error)?;
            publication::fonts::add_site(
                &mut site.files,
                &plan.theme.resolved,
                resource(resources.web_fonts, "web fonts")?,
            )
            .map_err(internal_error)?;
            publication::site::publish_site(&site, output).map_err(internal_error)?;
        }
    }
    Ok(Generation {
        output: output.to_string_lossy().into_owned(),
        pages: plan.pages.len(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

pub(crate) fn render_article(
    root: &Path,
    plan: &PagePlan,
    output: Option<&Path>,
    hash: Option<&str>,
    resources: &Resources<'_>,
) -> Result<ArticleGeneration> {
    let trusted = trusted_theme(&plan.theme, hash)?;
    let html = publication::article::render_article(root, plan, trusted).map_err(internal_error)?;
    let html = publication::fonts::embed_html(
        &html,
        &plan.theme.resolved,
        resource(resources.web_fonts, "web fonts")?,
    )
    .map_err(internal_error)?;
    if let Some(output) = output {
        publication::article::publish_article(&html, output).map_err(internal_error)?;
    }
    Ok(ArticleGeneration {
        output: output.map(|path| path.to_string_lossy().into_owned()),
        html: output.is_none().then_some(html),
        page_id: plan.page_id.clone(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

pub(crate) fn render_note_pdf(
    root: &Path,
    plan: &PagePlan,
    output: &Path,
    hash: Option<&str>,
    resources: &Resources<'_>,
) -> Result<NoteGeneration> {
    trusted_theme(&plan.theme, hash)?;
    let bundle = publication::note_pdf::render_note_pdf(root, plan).map_err(internal_error)?;
    publication::pdf::publish_pdf(
        &bundle,
        plan.profile,
        output,
        resource(resources.typst, "Typst")?,
        resource(resources.pdf_fonts, "PDF fonts")?,
    )
    .map_err(internal_error)?;
    Ok(NoteGeneration {
        output: output.to_string_lossy().into_owned(),
        page_id: plan.page_id.clone(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

pub(crate) fn render_note_docx(
    root: &Path,
    plan: &PagePlan,
    output: &Path,
    hash: Option<&str>,
    resources: &Resources<'_>,
) -> Result<NoteGeneration> {
    trusted_theme(&plan.theme, hash)?;
    let bundle = publication::word::render_page(
        root,
        plan,
        resource(resources.docx_fonts, "Word fonts")?,
        resources.pdf_fonts.map(Path::new),
    )
    .map_err(internal_error)?;
    publication::docx::publish_docx(&bundle, output).map_err(internal_error)?;
    Ok(NoteGeneration {
        output: output.to_string_lossy().into_owned(),
        page_id: plan.page_id.clone(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

pub(crate) fn render_note_epub(
    root: &Path,
    plan: &PagePlan,
    output: &Path,
    resources: &Resources<'_>,
) -> Result<NoteGeneration> {
    let bundle = publication::epub::render_page(
        root,
        plan,
        resource(resources.web_fonts, "web fonts")?,
        resources.pdf_fonts.map(Path::new),
    )
    .map_err(internal_error)?;
    publication::epub::publish(&bundle, output).map_err(internal_error)?;
    Ok(NoteGeneration {
        output: output.to_string_lossy().into_owned(),
        page_id: plan.page_id.clone(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

pub(crate) fn pdf_profile(value: &str) -> Result<PdfProfile> {
    match value {
        "default" => Ok(PdfProfile::Default),
        "accessible" => Ok(PdfProfile::Accessible),
        _ => Err(invalid_request("--profile must be default or accessible.")),
    }
}

pub(crate) fn outside_links(value: &str) -> Result<OutsideLinks> {
    match value {
        "error" => Ok(OutsideLinks::Error),
        "text-only" => Ok(OutsideLinks::TextOnly),
        _ => Err(invalid_request(
            "--outside-links must be error or text-only.",
        )),
    }
}
