// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Application-owned export state. Workers never retain the application or lease.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::engine::publication::{
    page::plan_page,
    plan::PublicationFormat,
    request::{ExportRequest, SourceKind},
    theme::ThemeSnapshot,
    theme_preview,
};
use tempfile::TempDir;

use super::{Resources, artifacts, model::*, preview, scheduler::Scheduler};
use crate::application::error::{Result, internal_error};

const LIFETIME: Duration = Duration::from_mins(30);
const INDEX_WAIT: Duration = Duration::from_mins(5);
static SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct Phase {
    started: Instant,
    span: tracing::Span,
}

impl Drop for Phase {
    fn drop(&mut self) {
        self.span.in_scope(|| {
            tracing::debug!(target: "bluink.export",
            event = "export_phase", duration_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            "Export phase completed");
        });
    }
}

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) root: PathBuf,
    pub(crate) index: Option<String>,
    pub(crate) web_fonts: Option<String>,
    pub(crate) pdf_fonts: Option<String>,
    pub(crate) docx_fonts: Option<String>,
    pub(crate) typst: Option<String>,
}

impl Context {
    fn resources(&self) -> Resources<'_> {
        Resources {
            web_fonts: self.web_fonts.as_deref(),
            pdf_fonts: self.pdf_fonts.as_deref(),
            docx_fonts: self.docx_fonts.as_deref(),
            typst: self.typst.as_deref(),
        }
    }
}

struct JobState {
    state: State,
    phase: &'static str,
    plan: Option<Arc<Plan>>,
    folder: Option<TempDir>,
    source_folder: Option<TempDir>,
    output: Option<PathBuf>,
    preview: Option<preview::Preview>,
    bundle: Option<tempfile::NamedTempFile>,
    error: Option<String>,
    busy: usize,
}

struct Job {
    id: String,
    span: tracing::Span,
    created: Instant,
    request: ExportRequest,
    context: Context,
    theme_preview: Option<theme_preview::Guard>,
    state: Mutex<JobState>,
    idle: Condvar,
    bundling: Mutex<()>,
    planning: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(crate::application::error::poison_error)
}

fn recover_for_cleanup<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Recovery is restricted to cancellation, failure reporting and cleanup.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct Work(Arc<Job>);

pub(crate) struct OpenedArtifact {
    pub(crate) file: std::fs::File,
    pub(crate) path: PathBuf,
    pub(crate) content_type: &'static str,
    pub(crate) site: bool,
    pub(crate) lease: Work,
}

impl Work {
    fn begin(job: &Arc<Job>, state: &mut JobState) -> Self {
        state.busy += 1;
        Self(Arc::clone(job))
    }
}

impl Drop for Work {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    fn drop(&mut self) {
        let mut state = recover_for_cleanup(&self.0.state);
        if state.busy == 1 && state.state == State::Queued {
            state.state = State::Failed;
            state.phase = "Generation failed";
            state.error = Some("Export queue stopped before generation began.".into());
        }
        if state.busy == 1 && matches!(state.state, State::Failed | State::Cancelled) {
            // Cleanup belongs to the last tracked task; stop waits for it too.
            drop(state);
            self.0.dispose();
            state = recover_for_cleanup(&self.0.state);
        }
        state.busy -= 1;
        self.0.idle.notify_all();
    }
}

impl Job {
    fn review_theme<'a>(&'a self, plan: &'a Plan) -> &'a ThemeSnapshot {
        self.theme_preview
            .as_ref()
            .map_or_else(|| plan.theme(), |preview| &preview.theme)
    }

    fn assert_preview_fresh(&self) -> Result<()> {
        if let Some(preview) = &self.theme_preview {
            preview.assert_fresh().map_err(internal_error)?;
        }
        Ok(())
    }
    fn measuring(&self, phase: &'static str) -> Phase {
        Phase {
            started: Instant::now(),
            span: tracing::info_span!(parent: &self.span,
            "export.phase", phase),
        }
    }

    fn cancelled(&self) -> bool {
        self.state
            .lock()
            .map_or(true, |state| state.state == State::Cancelled)
    }

    fn phase(&self, phase: &'static str) {
        if let Ok(mut state) = self.state.lock()
            && state.state != State::Cancelled
        {
            state.phase = phase;
        }
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    fn attempt(&self, action: impl FnOnce() -> Result<()>) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
            .unwrap_or_else(|_| Err(internal_error("Export generation panicked.")));
        if let Err(error) = outcome {
            let mut state = recover_for_cleanup(&self.state);
            if state.state != State::Cancelled {
                let phase = state.phase;
                state.state = State::Failed;
                state.phase = "Generation failed";
                state.error = Some(error.to_string());
                tracing::error!(target: "bluink.export", parent: &self.span,
                    event = "export_failed", phase, "Export generation failed");
            }
        }
    }

    fn cancel(&self) {
        {
            let mut state = recover_for_cleanup(&self.state);
            state.state = State::Cancelled;
            state.phase = "Cancelled";
        }
        self.idle.notify_all();
        if let Ok(scheduler) = Scheduler::global() {
            scheduler.cancel(&self.id);
        }
    }

    fn dispose(&self) {
        let _phase = self.measuring("cleanup");
        let (preview, folder, source_folder) = {
            let mut state = recover_for_cleanup(&self.state);
            state.output = None;
            state.bundle = None;
            (
                state.preview.take(),
                state.folder.take(),
                state.source_folder.take(),
            )
        };
        if let Some(preview) = preview
            && let Err(error) = preview.stop()
        {
            tracing::error!(target: "bluink.export", error = %error, "Website preview cleanup failed");
        }
        if let Some(folder) = folder
            && let Err(error) = folder.close()
        {
            tracing::error!(target: "bluink.export", error = %error, "Export artifact cleanup failed");
        }
        if let Some(folder) = source_folder
            && let Err(error) = folder.close()
        {
            tracing::error!(target: "bluink.export", error = %error, "Theme preview source cleanup failed");
        }
    }

    fn finish(&self) {
        let planning = recover_for_cleanup(&self.planning).take();
        if let Some(planning) = planning
            && planning.join().is_err()
        {
            tracing::error!(target: "bluink.export", "Export planning thread failed");
        }
        let mut state = recover_for_cleanup(&self.state);
        while state.busy != 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(state);
        self.dispose();
    }

    fn plan(&self) -> Result<Plan> {
        let _phase = self.measuring("planning");
        self.assert_preview_fresh()?;
        let request = &self.request;
        let context = &self.context;
        match request.source {
            SourceKind::Publication => {
                let index = context.index.as_deref();
                if super::needs_index(&context.root, &request.id)? {
                    self.phase("Waiting for the project index");
                    let index = index.ok_or_else(|| {
                        internal_error(
                            "Initialize the project index before preparing a Publication.",
                        )
                    })?;
                    let deadline = Instant::now() + INDEX_WAIT;
                    let readiness = self.measuring("readiness");
                    loop {
                        if self.cancelled() {
                            return Err(internal_error("Export cancelled."));
                        }
                        let status = crate::indexing::service::index_service_status_typed(index)?;
                        if status.state == "ready" && status.pending == 0 {
                            break;
                        }
                        if status.state == "error" {
                            return Err(internal_error(
                                status
                                    .error
                                    .unwrap_or_else(|| "Index reconciliation failed.".to_owned()),
                            ));
                        }
                        if Instant::now() >= deadline {
                            return Err(internal_error(
                                "Project indexing exceeded five minutes. Retry generation.",
                            ));
                        }
                        let state = lock(&self.state)?;
                        let _wait = self
                            .idle
                            .wait_timeout(state, Duration::from_millis(100))
                            .map_err(crate::application::error::poison_error)?;
                    }
                    drop(readiness);
                }
                self.phase("Resolving page selections");
                let resolution = self.measuring("resolve");
                let inputs = super::resolve_inputs(&context.root, &request.id, index)?;
                drop(resolution);
                self.phase("Validating pages, links, and assets");
                let _validation = self.measuring("validate");
                Ok(Plan::Publication(Box::new(super::plan_resolved(
                    &context.root,
                    &request.id,
                    request.format,
                    &inputs,
                )?)))
            }
            SourceKind::Note | SourceKind::LinkedPages => {
                let page = request
                    .page
                    .as_ref()
                    .ok_or_else(|| internal_error("Page export settings are unavailable."))?;
                if matches!(request.source, SourceKind::LinkedPages) {
                    self.phase("Following linked pages and checking content");
                    return Ok(Plan::Publication(Box::new(super::plan_linked_pages(
                        &context.root,
                        &request.id,
                        request.format,
                        &page.theme_id,
                        page.outside_links.clone(),
                        page.profile,
                    )?)));
                }
                self.phase("Checking page content");
                Ok(Plan::Page(Box::new(
                    plan_page(
                        &context.root,
                        &request.id,
                        &page.theme_id,
                        page.outside_links.clone(),
                        request.format,
                        page.profile,
                    )
                    .map_err(internal_error)?,
                )))
            }
        }
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    fn render(&self) -> Result<()> {
        let _phase = self.measuring("rendering");
        self.assert_preview_fresh()?;
        let (plan, output) = {
            let mut state = lock(&self.state)?;
            if state.state == State::Cancelled {
                return Ok(());
            }
            let plan = state
                .plan
                .clone()
                .ok_or_else(|| internal_error("Export content was not prepared."))?;
            state.state = State::Rendering;
            state.phase = match self.request.format {
                PublicationFormat::Pdf => "Generating PDF",
                PublicationFormat::Docx => "Generating Word document",
                PublicationFormat::Epub => "Building EPUB book",
                PublicationFormat::Html => "Building HTML document",
                PublicationFormat::Site => "Building website",
            };
            let folder = tempfile::Builder::new()
                .prefix("bluink-export-")
                .tempdir()?;
            let output = folder.path().join(match self.request.format {
                PublicationFormat::Pdf => "document.pdf",
                PublicationFormat::Docx => "document.docx",
                PublicationFormat::Epub => "document.epub",
                PublicationFormat::Html => "document.html",
                PublicationFormat::Site => "site",
            });
            state.output = Some(output.clone());
            state.folder = Some(folder);
            (plan, output)
        };
        let hash = plan
            .theme()
            .requires_trust
            .then_some(plan.theme().execution_sha256.as_str());
        let context = &self.context;
        match plan.as_ref() {
            Plan::Page(plan) => match plan.format {
                PublicationFormat::Epub => {
                    super::render_note_epub(&context.root, plan, &output, &context.resources())?;
                }
                PublicationFormat::Docx => {
                    super::render_note_docx(
                        &context.root,
                        plan,
                        &output,
                        hash,
                        &context.resources(),
                    )?;
                }
                PublicationFormat::Html => {
                    super::render_article(
                        &context.root,
                        plan,
                        Some(&output),
                        hash,
                        &context.resources(),
                    )?;
                }
                PublicationFormat::Pdf => {
                    super::render_note_pdf(
                        &context.root,
                        plan,
                        &output,
                        hash,
                        &context.resources(),
                    )?;
                }
                PublicationFormat::Site => {
                    return Err(internal_error(
                        "A standalone plan cannot generate a website.",
                    ));
                }
            },
            Plan::Publication(plan) => {
                super::render_publication(
                    &context.root,
                    plan,
                    &output,
                    hash,
                    context.index.as_deref(),
                    &context.resources(),
                )?;
            }
        }
        self.assert_preview_fresh()?;
        let mut state = lock(&self.state)?;
        if state.state != State::Cancelled {
            state.state = State::Ready;
            state.phase = "Ready";
        }
        Ok(())
    }
}

#[derive(Default)]
struct Registry {
    jobs: HashMap<String, Arc<Job>>,
    approved: HashSet<String>,
    stopped: bool,
}

#[derive(Default)]
struct Lifecycle {
    sweeper: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct Manager {
    registry: Mutex<Registry>,
    changed: Condvar,
    // Serializes creation with stop, including thread ownership publication.
    // Workers never acquire this gate; registry users need not wait for spawning.
    lifecycle: Mutex<Lifecycle>,
}

impl Manager {
    pub(crate) fn create(
        self: &Arc<Self>,
        context: Context,
        request: ExportRequest,
    ) -> Result<JobId> {
        self.create_owned(context, request, None)
    }

    pub(crate) fn create_theme_preview(
        self: &Arc<Self>,
        mut context: Context,
        request: ExportRequest,
        source: theme_preview::Source,
    ) -> Result<JobId> {
        source.folder.path().clone_into(&mut context.root);
        context.index = None;
        self.create_owned(context, request, Some(source))
    }

    fn create_owned(
        self: &Arc<Self>,
        context: Context,
        request: ExportRequest,
        source: Option<theme_preview::Source>,
    ) -> Result<JobId> {
        // Fail resource allocation before registering work, and serialize create
        // with stop so shutdown cannot miss an accepted planning thread.
        Scheduler::global()?;
        let mut lifecycle = lock(&self.lifecycle)?;
        if lock(&self.registry)?.stopped {
            return Err(internal_error("The application is shutting down."));
        }
        if lifecycle.sweeper.is_none() {
            let manager = Arc::downgrade(self);
            lifecycle.sweeper = Some(
                thread::Builder::new()
                    .name("bluink-export-expiry".to_owned())
                    .spawn(move || Self::sweep(manager))?,
            );
        }
        let id = uuid::Uuid::now_v7().to_string();
        let (theme_preview, source_folder) = source.map_or((None, None), |source| {
            (Some(source.guard), Some(source.folder))
        });
        let job = Arc::new(Job {
            id: id.clone(),
            span: tracing::info_span!(target: "bluink.export", "export.job",
                export_seq = SEQUENCE.fetch_add(1, Ordering::Relaxed)),
            created: Instant::now(),
            request,
            context,
            theme_preview,
            state: Mutex::new(JobState {
                state: State::Planning,
                phase: "Preparing export",
                plan: None,
                folder: None,
                source_folder,
                output: None,
                preview: None,
                bundle: None,
                error: None,
                busy: 1,
            }),
            idle: Condvar::new(),
            bundling: Mutex::new(()),
            planning: Mutex::new(None),
        });
        let work = Work(Arc::clone(&job));
        let planning = Arc::clone(&job);
        let manager = Arc::clone(self);
        let thread = thread::Builder::new()
            .name("bluink-export-plan".to_owned())
            .spawn(move || {
                let _span = planning.span.enter();
                planning.attempt(|| manager.prepare(&planning));
                drop(work);
            })?;
        *lock(&job.planning)? = Some(thread);
        match lock(&self.registry) {
            Ok(mut registry) => {
                registry.jobs.insert(id.clone(), job);
            }
            Err(error) => {
                job.cancel();
                job.finish();
                return Err(error);
            }
        }
        drop(lifecycle);
        self.changed.notify_all();
        Ok(JobId { job_id: id })
    }

    fn prepare(&self, job: &Arc<Job>) -> Result<()> {
        let plan = Arc::new(job.plan()?);
        let theme = job.review_theme(&plan);
        let approved = lock(&self.registry)?
            .approved
            .contains(&theme.execution_sha256);
        let mut state = lock(&job.state)?;
        if state.state == State::Cancelled {
            return Ok(());
        }
        let needs_approval = theme.requires_trust && !approved;
        state.plan = Some(plan);
        state.state = if needs_approval {
            State::AwaitingThemeApproval
        } else {
            State::Prepared
        };
        state.phase = if needs_approval {
            "Review project theme"
        } else {
            "Ready to generate"
        };
        if job.request.start && !needs_approval {
            Self::enqueue(job, &mut state)?;
        }
        Ok(())
    }

    fn enqueue(job: &Arc<Job>, state: &mut JobState) -> Result<()> {
        let scheduler = Scheduler::global()?;
        let rendering = Arc::clone(job);
        let queued = Instant::now();
        let _span = job.span.enter();
        scheduler.enqueue(
            matches!(
                job.request.format,
                PublicationFormat::Pdf | PublicationFormat::Docx | PublicationFormat::Epub
            ),
            job.id.clone(),
            || {
                state.state = State::Queued;
                state.phase = "Waiting to generate";
                let work = Work::begin(job, state);
                move || {
                    let _span = rendering.span.enter();
                    tracing::debug!(target: "bluink.export", event = "export_dequeued",
                        queue_ms = queued.elapsed().as_secs_f64() * 1000.0);
                    rendering.attempt(|| rendering.render());
                    drop(work);
                }
            },
        )
    }
    fn find(&self, id: &str) -> Result<Option<Arc<Job>>> {
        Ok(lock(&self.registry)?.jobs.get(id).cloned())
    }

    fn artifact_job(&self, id: &str) -> Result<Option<Arc<Job>>> {
        Ok(self.find(id)?.filter(|job| {
            let expired = job.created.elapsed() >= LIFETIME;
            if expired {
                self.changed.notify_all();
            }
            !expired
        }))
    }

    fn fresh(&self, id: &str) -> Result<Arc<Job>> {
        let job = self
            .find(id)?
            .ok_or_else(|| internal_error("Export was not found."))?;
        if job.created.elapsed() >= LIFETIME {
            self.changed.notify_all();
            return Err(internal_error("Export expired. Prepare it again."));
        }
        Ok(job)
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn read(&self, request: ReadRequest) -> Result<Status> {
        let job = self.fresh(&request.job_id)?;
        let approved = lock(&self.registry)?.approved.clone();
        let state = lock(&job.state)?;
        let theme_trust = state
            .plan
            .as_ref()
            .filter(|plan| job.review_theme(plan).requires_trust)
            .map(|plan| ThemeTrust {
                hash: job.review_theme(plan).execution_sha256.clone(),
                trusted: approved.contains(&job.review_theme(plan).execution_sha256),
                root: job
                    .theme_preview
                    .as_ref()
                    .map_or(&job.context.root, |preview| &preview.project)
                    .join("themes")
                    .join(&job.review_theme(plan).id)
                    .to_string_lossy()
                    .into_owned(),
            });
        let ready = state.state == State::Ready;
        let site = job.request.format == PublicationFormat::Site;
        Ok(Status {
            state: state.state,
            phase: state.phase,
            format: job.request.format,
            plan: request.include_plan.then(|| state.plan.clone()).flatten(),
            summary: state.plan.as_ref().map(|plan| plan.summary()),
            theme_trust,
            theme_preview: job
                .theme_preview
                .as_ref()
                .map(|preview| ThemePreviewSummary {
                    theme_id: preview.theme.id.clone(),
                    revision: preview.revision.clone(),
                    draft_sha256: preview.theme.sha256.clone(),
                }),
            artifact_url: ready.then(|| {
                format!(
                    "/api/export_artifact/{}{}",
                    job.id,
                    if site { "/index.html" } else { "" }
                )
            }),
            bundle_url: (ready && site)
                .then(|| format!("/api/export_artifact/{}/site.zip", job.id)),
            preview_url: ready
                .then(|| state.preview.as_ref().map(|preview| preview.url.clone()))
                .flatten(),
            error: state.error.clone(),
        })
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn start(&self, request: JobRequest) -> Result<JobId> {
        let job = self.fresh(&request.job_id)?;
        let mut state = lock(&job.state)?;
        if state.state != State::Prepared {
            return Err(internal_error(
                "Prepare and approve the export before generating it.",
            ));
        }
        Self::enqueue(&job, &mut state)?;
        Ok(JobId {
            job_id: job.id.clone(),
        })
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    pub(crate) fn approve(&self, request: ApproveRequest) -> Result<Approved> {
        let job = self.fresh(&request.job_id)?;
        job.assert_preview_fresh()?;
        let mut registry = lock(&self.registry)?;
        let mut state = lock(&job.state)?;
        if state.state != State::AwaitingThemeApproval
            || state
                .plan
                .as_ref()
                .is_none_or(|plan| job.review_theme(plan).execution_sha256 != request.hash)
        {
            return Err(internal_error(
                "The theme changed. Prepare again to review its current version.",
            ));
        }
        registry.approved.insert(request.hash);
        if job.request.start {
            Self::enqueue(&job, &mut state)?;
        } else {
            state.state = State::Prepared;
            state.phase = "Ready to generate";
        }
        Ok(Approved { trusted: true })
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn cancel(&self, request: JobRequest) -> Cancelled {
        let job = recover_for_cleanup(&self.registry)
            .jobs
            .get(&request.job_id)
            .cloned();
        let Some(job) = job else {
            return Cancelled { cancelled: false };
        };
        {
            let mut state = recover_for_cleanup(&job.state);
            if matches!(state.state, State::Failed | State::Cancelled)
                || (state.state == State::Ready && job.theme_preview.is_none())
            {
                return Cancelled { cancelled: false };
            }
            state.state = State::Cancelled;
            state.phase = "Cancelled";
        }
        job.idle.notify_all();
        if let Ok(scheduler) = Scheduler::global() {
            scheduler.cancel(&job.id);
        }
        if recover_for_cleanup(&job.state).busy == 0 {
            job.dispose();
        }
        Cancelled { cancelled: true }
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn preview(&self, request: JobRequest) -> Result<PreviewUrl> {
        let job = self.fresh(&request.job_id)?;
        let mut state = lock(&job.state)?;
        if state.state != State::Ready || job.request.format != PublicationFormat::Site {
            return Err(internal_error("Generate the website before viewing it."));
        }
        if state.preview.is_none() {
            let root = state
                .output
                .clone()
                .ok_or_else(|| internal_error("Generated site is unavailable."))?;
            state.preview = Some(preview::Preview::start(root)?);
        }
        Ok(PreviewUrl {
            url: state
                .preview
                .as_ref()
                .ok_or_else(|| internal_error("Could not open website preview."))?
                .url
                .clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn artifact(&self, id: &str, path: &str) -> Result<Option<Artifact>> {
        self.open_artifact(id, path)?
            .map(|opened| {
                Ok(Artifact {
                    size: opened.file.metadata()?.len(),
                    content_type: opened.content_type,
                    path: opened.path.to_string_lossy().into_owned(),
                    site: opened.site,
                })
            })
            .transpose()
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    pub(crate) fn open_artifact(&self, id: &str, path: &str) -> Result<Option<OpenedArtifact>> {
        let Some(job) = self.artifact_job(id)? else {
            return Ok(None);
        };
        let (root, lease) = {
            let mut state = lock(&job.state)?;
            if state.state != State::Ready {
                return Ok(None);
            }
            let Some(root) = state.output.clone() else {
                return Ok(None);
            };
            (root, Work::begin(&job, &mut state))
        };
        let site = job.request.format == PublicationFormat::Site;
        let path = if site {
            preview::file(&root, path)
        } else {
            path.is_empty().then(|| root.clone())
        };
        let Some(path) = path else {
            return Ok(None);
        };
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(OpenedArtifact {
            file,
            content_type: preview::content_type(&path),
            path,
            site,
            lease,
        }))
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    pub(crate) fn site_zip(&self, id: &str) -> Result<Option<OpenedArtifact>> {
        let Some(job) = self.artifact_job(id)? else {
            return Ok(None);
        };
        let (root, work) = {
            let mut state = lock(&job.state)?;
            if state.state != State::Ready || job.request.format != PublicationFormat::Site {
                return Ok(None);
            }
            let root = state
                .output
                .clone()
                .ok_or_else(|| internal_error("Generated site is unavailable."))?;
            (root, Work::begin(&job, &mut state))
        };
        // A separate lock serializes bundle preparation without blocking job
        // status or cancellation. The lease protects the source throughout.
        let _bundling = lock(&job.bundling)?;
        {
            let state = lock(&job.state)?;
            if state.state != State::Ready {
                return Ok(None);
            }
            if let Some(bundle) = &state.bundle {
                return Ok(Some(OpenedArtifact {
                    file: bundle.reopen()?,
                    path: PathBuf::from("bluink-site.zip"),
                    content_type: "application/zip",
                    site: false,
                    lease: work,
                }));
            }
        }
        let mut bundle = tempfile::NamedTempFile::new_in(
            root.parent()
                .ok_or_else(|| internal_error("Site has no output directory"))?,
        )?;
        artifacts::site_zip(&root, bundle.as_file_mut())?;
        let file = bundle.reopen()?;
        let mut state = lock(&job.state)?;
        if state.state != State::Ready {
            return Ok(None);
        }
        state.bundle = Some(bundle);
        Ok(Some(OpenedArtifact {
            file,
            path: PathBuf::from("bluink-site.zip"),
            content_type: "application/zip",
            site: false,
            lease: work,
        }))
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    #[expect(
        clippy::needless_collect,
        reason = "Collect expired ids before mutating the registry."
    )]
    fn expired(&self, now: Instant) -> Vec<Arc<Job>> {
        let mut registry = recover_for_cleanup(&self.registry);
        let ids = registry
            .jobs
            .iter()
            .filter(|(_, job)| now.saturating_duration_since(job.created) >= LIFETIME)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| registry.jobs.remove(&id))
            .collect()
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    fn sweep(manager: Weak<Self>) {
        while let Some(manager) = manager.upgrade() {
            for job in manager.expired(Instant::now()) {
                job.cancel();
                job.finish();
            }
            let registry = recover_for_cleanup(&manager.registry);
            if registry.stopped {
                return;
            }
            let wait = registry
                .jobs
                .values()
                .map(|job| LIFETIME.saturating_sub(job.created.elapsed()))
                .min()
                .unwrap_or(LIFETIME);
            let _wait = manager
                .changed
                .wait_timeout(registry, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(crate) fn stop(&self) {
        let mut lifecycle = recover_for_cleanup(&self.lifecycle);
        let jobs = {
            let mut registry = recover_for_cleanup(&self.registry);
            registry.stopped = true;
            std::mem::take(&mut registry.jobs)
        };
        self.changed.notify_all();
        for job in jobs.values() {
            job.cancel();
        }
        if let Some(sweeper) = lifecycle.sweeper.take()
            && sweeper.join().is_err()
        {
            tracing::error!(target: "bluink.export", "Export expiry thread failed");
        }
        for job in jobs.into_values() {
            job.finish();
        }
        recover_for_cleanup(&self.registry).approved.clear();
        drop(lifecycle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn an_open_download_keeps_expired_output_until_its_stream_closes() {
        use std::io::Read;
        let manager = Manager::default();
        let job = job(&manager, State::Ready);
        let path = recover_for_cleanup(&job.state).output.clone().unwrap();
        let mut artifact = manager.open_artifact(&job.id, "").unwrap().unwrap();
        assert_eq!(recover_for_cleanup(&job.state).busy, 1);
        let expired = manager.expired(Instant::now() + LIFETIME);
        assert_eq!(expired.len(), 1);
        job.cancel();
        let (done, finished) = mpsc::channel();
        let waiting = Arc::clone(&job);
        let cleanup = thread::spawn(move || {
            waiting.finish();
            done.send(()).unwrap();
        });
        assert!(finished.try_recv().is_err());
        assert!(path.exists());
        let mut contents = String::new();
        artifact.file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "temporary output");
        drop(artifact);
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        cleanup.join().unwrap();
        assert!(!path.exists());
        manager.stop();
    }

    fn job(manager: &Manager, state: State) -> Arc<Job> {
        job_with_source(manager, state, None)
    }

    #[test]
    fn concurrent_creation_and_stop_leave_no_registered_work_or_expiry_thread() {
        let manager = Arc::new(Manager::default());
        let fixture = job(&manager, State::Prepared);
        recover_for_cleanup(&manager.registry)
            .jobs
            .remove(&fixture.id);
        let accepted = manager.create(fixture.context.clone(), request()).unwrap();
        let accepted = manager.find(&accepted.job_id).unwrap().unwrap();
        let start = Arc::new(std::sync::Barrier::new(9));
        let mut creators = Vec::new();
        for _ in 0..8 {
            let manager = Arc::clone(&manager);
            let start = Arc::clone(&start);
            let context = fixture.context.clone();
            let request = request();
            creators.push(thread::spawn(move || {
                start.wait();
                manager.create(context, request)
            }));
        }
        start.wait();
        manager.stop();
        for creator in creators {
            // Creation either publishes before stop or rejects stopped admission.
            let _outcome = creator.join().unwrap();
        }
        let registry = recover_for_cleanup(&manager.registry);
        assert!(registry.stopped);
        assert!(registry.jobs.is_empty());
        drop(registry);
        assert_eq!(recover_for_cleanup(&accepted.state).busy, 0);
        assert!(recover_for_cleanup(&accepted.planning).is_none());
        assert!(recover_for_cleanup(&manager.lifecycle).sweeper.is_none());
        assert!(manager.create(fixture.context.clone(), request()).is_err());
        manager.stop();
    }

    fn request() -> ExportRequest {
        serde_json::from_value(json!({"source": {"kind": "note", "id": "00000000-0000-7000-8000-000000000601"},
            "format": "pdf", "start": false, "themeId": "classic", "outsideLinks": "error", "profile": "default"})).unwrap()
    }

    fn job_with_source(
        manager: &Manager,
        state: State,
        source: Option<theme_preview::Source>,
    ) -> Arc<Job> {
        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("document.pdf");
        std::fs::write(&output, b"temporary output").unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let (theme_preview, source_folder) = source.map_or((None, None), |source| {
            (Some(source.guard), Some(source.folder))
        });
        let job = Arc::new(Job {
            id: id.clone(),
            span: tracing::Span::none(),
            created: Instant::now(),
            request: request(),
            context: Context {
                root: folder.path().to_owned(),
                index: None,
                web_fonts: None,
                pdf_fonts: None,
                docx_fonts: None,
                typst: None,
            },
            theme_preview,
            state: Mutex::new(JobState {
                state,
                phase: "Test",
                plan: None,
                folder: Some(folder),
                source_folder,
                output: Some(output),
                preview: None,
                bundle: None,
                error: None,
                busy: 0,
            }),
            idle: Condvar::new(),
            bundling: Mutex::new(()),
            planning: Mutex::new(None),
        });
        recover_for_cleanup(&manager.registry)
            .jobs
            .insert(id, Arc::clone(&job));
        job
    }

    #[test]
    fn poisoned_job_or_registry_rejects_operations_but_allows_shutdown_cleanup() {
        fn poison<T>(mutex: &Mutex<T>) {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = mutex.lock().unwrap();
                panic!("injected export state corruption");
            }));
            assert!(outcome.is_err());
        }
        for poison_registry in [false, true] {
            let manager = Manager::default();
            let prepared = job(&manager, State::Prepared);
            let output = recover_for_cleanup(&prepared.state).output.clone().unwrap();
            if poison_registry {
                poison(&manager.registry);
            } else {
                poison(&prepared.state);
            }
            assert!(
                manager
                    .start(JobRequest {
                        job_id: prepared.id.clone()
                    })
                    .is_err()
            );
            assert!(
                manager
                    .read(ReadRequest {
                        job_id: prepared.id.clone(),
                        include_plan: false,
                    })
                    .is_err()
            );
            assert!(manager.open_artifact(&prepared.id, "").is_err());
            assert!(manager.site_zip(&prepared.id).is_err());
            manager.stop();
            assert!(!output.exists());
            assert!(recover_for_cleanup(&manager.registry).jobs.is_empty());
            assert_eq!(recover_for_cleanup(&prepared.state).busy, 0);
        }
    }

    #[test]
    fn missing_or_expired_artifacts_are_unavailable() {
        let manager = Manager::default();
        assert!(manager.open_artifact("missing", "").unwrap().is_none());
        assert!(manager.site_zip("missing").unwrap().is_none());
        let mut expired = job(&manager, State::Ready);
        recover_for_cleanup(&manager.registry)
            .jobs
            .remove(&expired.id);
        Arc::get_mut(&mut expired).unwrap().created = Instant::now().checked_sub(LIFETIME).unwrap();
        recover_for_cleanup(&manager.registry)
            .jobs
            .insert(expired.id.clone(), Arc::clone(&expired));
        assert!(manager.open_artifact(&expired.id, "").unwrap().is_none());
        assert!(manager.site_zip(&expired.id).unwrap().is_none());
        manager.stop();
    }

    #[test]
    fn abandoned_queued_work_fails_and_reclaims_its_output() {
        let manager = Manager::default();
        let queued = job(&manager, State::Queued);
        let output = recover_for_cleanup(&queued.state).output.clone().unwrap();
        let work = Work::begin(&queued, &mut recover_for_cleanup(&queued.state));
        drop(work);
        let state = recover_for_cleanup(&queued.state);
        assert_eq!(state.state, State::Failed);
        assert_eq!(state.busy, 0);
        assert_eq!(
            state.error.as_deref(),
            Some("Export queue stopped before generation began.")
        );
        assert!(!output.exists());
    }

    #[test]
    fn expiry_uses_creation_time_and_reclaims_every_state() {
        let manager = Manager::default();
        let mut paths = Vec::new();
        for state in [
            State::Planning,
            State::Prepared,
            State::AwaitingThemeApproval,
            State::Queued,
            State::Rendering,
            State::Ready,
            State::Failed,
            State::Cancelled,
        ] {
            let job = job(&manager, state);
            paths.push(recover_for_cleanup(&job.state).output.clone().unwrap());
            let source = tempfile::tempdir().unwrap();
            paths.push(source.path().to_owned());
            recover_for_cleanup(&job.state).source_folder = Some(source);
        }
        assert!(manager.expired(Instant::now()).is_empty());
        let expired = manager.expired(Instant::now() + LIFETIME);
        assert_eq!(expired.len(), 8);
        for job in expired {
            job.cancel();
            job.finish();
        }
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(recover_for_cleanup(&manager.registry).jobs.is_empty());
        manager.stop();
        manager.stop();
    }

    #[test]
    fn cancelling_a_ready_theme_preview_keeps_source_and_output_until_download_finishes() {
        let project = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(project.path(), "UTC").unwrap();
        let theme = crate::engine::publication::theme::capture_theme(
            project.path(),
            "classic",
            PublicationFormat::Pdf,
        )
        .unwrap();
        let source = theme_preview::capture(
            project.path(),
            "classic",
            &theme.sha256,
            &theme.definition,
            PublicationFormat::Pdf,
        )
        .unwrap();
        let source_path = source.folder.path().to_owned();
        let manager = Manager::default();
        let job = job_with_source(&manager, State::Ready, Some(source));
        let output = recover_for_cleanup(&job.state).output.clone().unwrap();
        let artifact = manager.open_artifact(&job.id, "").unwrap().unwrap();
        assert!(
            manager
                .cancel(JobRequest {
                    job_id: job.id.clone()
                })
                .cancelled
        );
        assert_eq!(recover_for_cleanup(&job.state).state, State::Cancelled);
        assert!(source_path.exists());
        assert!(output.exists());
        assert!(manager.open_artifact(&job.id, "").unwrap().is_none());
        drop(artifact);
        assert!(!source_path.exists());
        assert!(!output.exists());
        manager.stop();
    }

    #[test]
    fn cancelling_a_prepared_job_reclaims_owned_preview_sources_immediately() {
        let manager = Manager::default();
        let job = job(&manager, State::Prepared);
        let source = tempfile::tempdir().unwrap();
        let path = source.path().to_owned();
        recover_for_cleanup(&job.state).source_folder = Some(source);
        assert!(
            manager
                .cancel(JobRequest {
                    job_id: job.id.clone()
                })
                .cancelled
        );
        assert!(!path.exists());
        manager.stop();
    }

    #[test]
    fn cancellation_keeps_active_capacity_and_shutdown_waits_for_cleanup() {
        let first = Arc::new(Manager::default());
        let second = Manager::default();
        let active = job(&first, State::Rendering);
        let queued = job(&second, State::Queued);
        let next = job(&second, State::Queued);
        let output = recover_for_cleanup(&active.state).output.clone().unwrap();
        let scheduler = Scheduler::global().unwrap();
        let (started, events) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let work = Work::begin(&active, &mut recover_for_cleanup(&active.state));
        let started_active = started.clone();
        scheduler
            .enqueue(true, active.id.clone(), || {
                move || {
                    started_active.send("active").unwrap();
                    wait.recv().unwrap();
                    drop(work);
                }
            })
            .unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "active"
        );
        for (job, label) in [(&queued, "cancelled"), (&next, "next")] {
            let work = Work::begin(job, &mut recover_for_cleanup(&job.state));
            let started = started.clone();
            scheduler
                .enqueue(true, job.id.clone(), || {
                    move || {
                        started.send(label).unwrap();
                        drop(work);
                    }
                })
                .unwrap();
        }
        assert!(
            first
                .cancel(JobRequest {
                    job_id: active.id.clone()
                })
                .cancelled
        );
        assert!(
            second
                .cancel(JobRequest {
                    job_id: queued.id.clone()
                })
                .cancelled
        );
        assert_eq!(recover_for_cleanup(&queued.state).busy, 0);
        assert!(events.try_recv().is_err());
        assert!(output.exists());
        let (stopped, completion) = mpsc::channel();
        let stopping = Arc::clone(&first);
        let shutdown = thread::spawn(move || {
            stopping.stop();
            stopped.send(()).unwrap();
        });
        assert!(completion.try_recv().is_err());
        release.send(()).unwrap();
        assert_eq!(events.recv_timeout(Duration::from_secs(5)).unwrap(), "next");
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        shutdown.join().unwrap();
        assert!(!output.exists());
        second.stop();
    }

    #[test]
    fn a_panicking_render_is_failed_and_its_tracked_output_is_removed() {
        let manager = Manager::default();
        let job = job(&manager, State::Rendering);
        let output = recover_for_cleanup(&job.state).output.clone().unwrap();
        let work = Work::begin(&job, &mut recover_for_cleanup(&job.state));
        job.attempt(|| panic!("injected render failure"));
        assert_eq!(recover_for_cleanup(&job.state).state, State::Failed);
        assert!(output.exists());
        drop(work);
        assert!(!output.exists());
        assert_eq!(recover_for_cleanup(&job.state).busy, 0);
        manager.stop();
    }

    #[test]
    fn expiry_joins_a_preview_before_removing_its_site() {
        let manager = Manager::default();
        let job = job(&manager, State::Ready);
        let mut state = recover_for_cleanup(&job.state);
        let folder = state.folder.as_ref().unwrap().path().to_owned();
        std::fs::write(folder.join("index.html"), "Preview").unwrap();
        let preview = preview::Preview::start(folder.clone()).unwrap();
        let address = preview
            .url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_owned();
        state.preview = Some(preview);
        drop(state);
        assert!(std::net::TcpStream::connect(&address).is_ok());
        for expired in manager.expired(Instant::now() + LIFETIME) {
            expired.cancel();
            expired.finish();
        }
        assert!(std::net::TcpStream::connect(address).is_err());
        assert!(!folder.exists());
        manager.stop();
    }
}
