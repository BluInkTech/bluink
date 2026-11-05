// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed planning and rendering shared by the application, CLI and HTTP jobs.
pub(crate) mod article;
mod artifacts;
pub(crate) mod docx;
pub(crate) mod epub;
pub(crate) mod error;
pub(crate) mod fonts;
pub(crate) mod media;
pub(crate) mod note_pdf;
pub(crate) mod one_html;
pub(crate) mod output;
pub(crate) mod page;
pub(crate) mod parallel;
pub(crate) mod pdf;
pub(crate) mod plan;
pub(crate) mod preflight;
pub(crate) mod resources;
pub(crate) mod site;
pub(crate) mod theme;
pub(crate) mod theme_preview;
pub(crate) mod theme_store;
pub(crate) mod word;

mod decisions;
pub(crate) mod jobs;
pub(crate) mod model;
mod preview;
mod scheduler;
use self::error::{Result, internal_message, invalid_request};
use crate::domain::publication::{self, LinkDepth, OutlineNode, OutsideLinks, PdfProfile};
use crate::structured_query;
use publication::{
    page::PagePlan,
    plan::{PublicationFormat, PublicationPlan},
    theme::ThemeSnapshot,
};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

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
    /// A queued export's cancellation, checked while Typst runs.
    pub(crate) cancelled: Option<&'a dyn Fn() -> bool>,
}

const fn never() -> bool {
    false
}

impl Resources<'_> {
    fn cancelled(&self) -> &dyn Fn() -> bool {
        self.cancelled.unwrap_or(&never)
    }
}

fn resource<'a>(value: Option<&'a str>, name: &str) -> Result<&'a Path> {
    value
        .map(Path::new)
        .ok_or_else(|| internal_message(format!("The installed {name} resource is unavailable.")))
}

fn collection_queries<'a>(
    nodes: &'a [OutlineNode],
    output: &mut Vec<(&'a str, &'a structured_query::document::QueryDocument)>,
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

pub(crate) fn resolve_inputs(root: &Path, id: &str) -> Result<ResolvedInputs> {
    let store = crate::store::Store::shared(root)?;
    store
        .read(|tx| {
            let item = crate::store::item::read(tx, id, true)?
                .filter(|item| item["meta"]["type"] == "publication")
                .ok_or(crate::store::Error::NotFound)?;
            let publication_revision = item["revision"]
                .as_str()
                .ok_or_else(|| {
                    crate::store::Error::Invalid("Publication has no revision".to_owned())
                })?
                .to_owned();
            let content = publication::validate_publication(&item["content"])
                .map_err(crate::store::Error::Invalid)?;
            let config = crate::store::project::read_config(tx)?;
            let timezone = config["analytics"]["timezone"]
                .as_str()
                .ok_or_else(|| crate::store::Error::Invalid("Project has no timezone".to_owned()))?
                .parse::<chrono_tz::Tz>()
                .map_err(|error| crate::store::Error::Invalid(error.to_string()))?;
            let mut queries = Vec::new();
            collection_queries(&content.outline, &mut queries);
            let mut collections = HashMap::new();
            for (id, source) in queries {
                let mut ordered = source.clone();
                if ordered.sort.is_empty() {
                    ordered.sort.push(structured_query::document::QuerySort {
                        field: "title".into(),
                        direction: structured_query::document::SortDirection::Asc,
                    });
                }
                let scoped = structured_query::document::QueryDocument::active_pages()
                    .compose(&ordered)
                    .map_err(crate::store::Error::InvalidQuery)?;
                let ids = crate::store::query::ordered_ids(tx, &scoped, None, timezone)?;
                collections.insert(id.to_owned(), ids);
            }
            Ok(ResolvedInputs {
                publication_revision,
                collections,
            })
        })
        .map_err(Into::into)
}

pub(crate) fn plan_resolved(
    root: &Path,
    id: &str,
    format: PublicationFormat,
    inputs: &ResolvedInputs,
) -> Result<PublicationPlan> {
    let mut plan =
        crate::export_jobs::plan::plan_publication_resolved(root, id, format, &inputs.collections)
            .map_err(internal_message)?;
    if plan.publication_revision != inputs.publication_revision {
        return Err(invalid_request(
            "Publication changed while resolving its collections. Generate again.",
        ));
    }
    decisions::annotate(root, &mut plan)?;
    Ok(plan)
}

pub(crate) fn plan_publication(
    root: &Path,
    id: &str,
    format: PublicationFormat,
) -> Result<PublicationPlan> {
    plan_resolved(root, id, format, &resolve_inputs(root, id)?)
}

pub(crate) fn plan_page(
    root: &Path,
    page: &str,
    theme: &str,
    outside: OutsideLinks,
    format: PublicationFormat,
    profile: PdfProfile,
) -> Result<PagePlan> {
    let mut plan = crate::export_jobs::page::plan_page(root, page, theme, outside, format, profile)
        .map_err(internal_message)?;
    decisions::annotate_page(root, &mut plan)?;
    Ok(plan)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Linked-page planning takes the export settings, its depth and a progress callback."
)]
pub(crate) fn plan_linked_pages(
    root: &Path,
    page: &str,
    format: PublicationFormat,
    theme: &str,
    outside: OutsideLinks,
    profile: PdfProfile,
    depth: LinkDepth,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<PublicationPlan> {
    let mut plan = crate::export_jobs::plan::plan_linked_pages(
        root, page, format, theme, outside, profile, depth, progress,
    )
    .map_err(internal_message)?;
    decisions::annotate(root, &mut plan)?;
    Ok(plan)
}

pub(crate) fn link_depth(value: &str) -> Result<LinkDepth> {
    match value {
        "direct" => Ok(LinkDepth::Direct),
        "two-levels" => Ok(LinkDepth::TwoLevels),
        "reachable" => Ok(LinkDepth::Reachable),
        _ => Err(invalid_request(
            "--link-depth must be direct, two-levels or reachable.",
        )),
    }
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
    resources: &Resources<'_>,
) -> Result<Generation> {
    if !plan.collections.is_empty() {
        let current = resolve_inputs(root, &plan.publication_id)?;
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
    // A linked-pages plan resolves its links, images and snippets now, as
    // generation reads its pages, unless its caller already did.
    let completed;
    let plan = if plan.unresolved {
        completed =
            crate::export_jobs::plan::complete_linked_plan(root, plan).map_err(internal_message)?;
        &completed
    } else {
        plan
    };
    match plan.format {
        PublicationFormat::Epub => {
            let bundle = crate::export_jobs::epub::render_publication(
                root,
                plan,
                resource(resources.web_fonts, "web fonts")?,
                resources.pdf_fonts.map(Path::new),
            )
            .map_err(internal_message)?;
            crate::export_jobs::epub::publish(&bundle, output).map_err(internal_message)?;
        }
        PublicationFormat::Docx => {
            let bundle = crate::export_jobs::word::render_publication(
                root,
                plan,
                resource(resources.docx_fonts, "Word fonts")?,
                resources.pdf_fonts.map(Path::new),
            )
            .map_err(internal_message)?;
            crate::export_jobs::docx::publish_docx(&bundle, output).map_err(internal_message)?;
        }
        PublicationFormat::Pdf => {
            let bundle = crate::export_jobs::pdf::render_publication_pdf(root, plan)
                .map_err(internal_message)?;
            crate::export_jobs::pdf::publish_pdf_until(
                bundle,
                plan.targets.pdf.profile,
                output,
                resource(resources.typst, "Typst")?,
                resource(resources.pdf_fonts, "PDF fonts")?,
                resources.cancelled(),
            )
            .map_err(internal_message)?;
        }
        PublicationFormat::Html => {
            let html = crate::export_jobs::one_html::render_publication_html(root, plan, trusted)
                .map_err(internal_message)?;
            let html = crate::export_jobs::fonts::embed_html(
                &html,
                &plan.theme.resolved,
                resource(resources.web_fonts, "web fonts")?,
            )
            .map_err(internal_message)?;
            crate::export_jobs::one_html::publish_publication_html(&html, output)
                .map_err(internal_message)?;
        }
        PublicationFormat::Site => {
            let mut stage =
                crate::export_jobs::site::SiteStage::new(output).map_err(internal_message)?;
            stage
                .use_fonts(
                    &plan.theme.resolved,
                    resource(resources.web_fonts, "web fonts")?,
                )
                .map_err(internal_message)?;
            crate::export_jobs::site::render_publication_site(root, plan, trusted, &mut stage)
                .map_err(internal_message)?;
            if plan.targets.site.pdf_download {
                let pdf = stage
                    .path(crate::export_jobs::site::PDF_DOWNLOAD)
                    .map_err(internal_message)?;
                render_site_pdf(root, plan, &pdf, resources)?;
            }
            stage.publish().map_err(internal_message)?;
        }
    }
    Ok(Generation {
        output: output.to_string_lossy().into_owned(),
        pages: plan.pages.len(),
        assets: plan.assets.len(),
        theme_sha256: plan.theme.sha256.clone(),
    })
}

/// Typesets the whole Publication once for its website, into the site's stage.
fn render_site_pdf(
    root: &Path,
    site: &PublicationPlan,
    pdf: &Path,
    resources: &Resources<'_>,
) -> Result<()> {
    let inputs = resolve_inputs(root, &site.publication_id)?;
    let plan =
        crate::export_jobs::plan::plan_site_pdf(root, &site.publication_id, &inputs.collections)
            .map_err(internal_message)?;
    if plan.publication_revision != site.publication_revision {
        return Err(invalid_request(
            "Publication changed while generating its PDF. Generate again.",
        ));
    }
    let bundle =
        crate::export_jobs::pdf::render_publication_pdf(root, &plan).map_err(internal_message)?;
    crate::export_jobs::pdf::publish_pdf_until(
        bundle,
        plan.targets.pdf.profile,
        pdf,
        resource(resources.typst, "Typst")?,
        resource(resources.pdf_fonts, "PDF fonts")?,
        resources.cancelled(),
    )
    .map_err(internal_message)
}

pub(crate) fn render_article(
    root: &Path,
    plan: &PagePlan,
    output: Option<&Path>,
    hash: Option<&str>,
    resources: &Resources<'_>,
) -> Result<ArticleGeneration> {
    let trusted = trusted_theme(&plan.theme, hash)?;
    let html = crate::export_jobs::article::render_article(root, plan, trusted)
        .map_err(internal_message)?;
    let html = crate::export_jobs::fonts::embed_html(
        &html,
        &plan.theme.resolved,
        resource(resources.web_fonts, "web fonts")?,
    )
    .map_err(internal_message)?;
    if let Some(output) = output {
        crate::export_jobs::article::publish_article(&html, output).map_err(internal_message)?;
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
    let bundle =
        crate::export_jobs::note_pdf::render_note_pdf(root, plan).map_err(internal_message)?;
    crate::export_jobs::pdf::publish_pdf_until(
        bundle,
        plan.profile,
        output,
        resource(resources.typst, "Typst")?,
        resource(resources.pdf_fonts, "PDF fonts")?,
        resources.cancelled(),
    )
    .map_err(internal_message)?;
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
    let bundle = crate::export_jobs::word::render_page(
        root,
        plan,
        resource(resources.docx_fonts, "Word fonts")?,
        resources.pdf_fonts.map(Path::new),
    )
    .map_err(internal_message)?;
    crate::export_jobs::docx::publish_docx(&bundle, output).map_err(internal_message)?;
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
    let bundle = crate::export_jobs::epub::render_page(
        root,
        plan,
        resource(resources.web_fonts, "web fonts")?,
        resources.pdf_fonts.map(Path::new),
    )
    .map_err(internal_message)?;
    crate::export_jobs::epub::publish(&bundle, output).map_err(internal_message)?;
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

#[cfg(test)]
mod note_parity_tests;

#[cfg(test)]
mod regression;
