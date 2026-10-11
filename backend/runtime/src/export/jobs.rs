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
    article, note_pdf,
    plan::PublicationFormat,
    request::{ExportRequest, SourceKind},
    theme::ThemeOrigin,
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
    pub(crate) typst: Option<String>,
}

impl Context {
    fn resources(&self) -> Resources<'_> {
        Resources {
            web_fonts: self.web_fonts.as_deref(),
            pdf_fonts: self.pdf_fonts.as_deref(),
            typst: self.typst.as_deref(),
        }
    }
}

struct JobState {
    state: State,
    phase: &'static str,
    plan: Option<Arc<Plan>>,
    folder: Option<TempDir>,
    output: Option<PathBuf>,
    preview: Option<preview::Preview>,
    bundle: Option<Arc<Vec<u8>>>,
    error: Option<String>,
    busy: usize,
}

struct Job {
    id: String,
    span: tracing::Span,
    created: Instant,
    request: ExportRequest,
    context: Context,
    state: Mutex<JobState>,
    idle: Condvar,
    planning: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Work panics are caught outside these short critical sections. Recovering
    // a poisoned lock still allows shutdown to reclaim owned resources.
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
        let mut state = lock(&self.0.state);
        if state.busy == 1 && matches!(state.state, State::Failed | State::Cancelled) {
            // Cleanup belongs to the last tracked task; stop waits for it too.
            drop(state);
            self.0.dispose();
            state = lock(&self.0.state);
        }
        state.busy -= 1;
        self.0.idle.notify_all();
    }
}

impl Job {
    fn measuring(&self, phase: &'static str) -> Phase {
        Phase {
            started: Instant::now(),
            span: tracing::info_span!(parent: &self.span,
            "export.phase", phase),
        }
    }

    fn cancelled(&self) -> bool {
        lock(&self.state).state == State::Cancelled
    }

    fn phase(&self, phase: &'static str) {
        let mut state = lock(&self.state);
        if state.state != State::Cancelled {
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
            let mut state = lock(&self.state);
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
            let mut state = lock(&self.state);
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
        let (preview, folder) = {
            let mut state = lock(&self.state);
            state.output = None;
            state.bundle = None;
            (state.preview.take(), state.folder.take())
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
    }

    fn finish(&self) {
        if let Some(planning) = lock(&self.planning).take()
            && planning.join().is_err()
        {
            tracing::error!(target: "bluink.export", "Export planning thread failed");
        }
        let mut state = lock(&self.state);
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
        let request = &self.request;
        let context = &self.context;
        match request.source {
            SourceKind::Publication => {
                self.phase("Waiting for the project index");
                let index = context.index.as_deref().ok_or_else(|| {
                    internal_error("Initialize the project index before preparing a Publication.")
                })?;
                let deadline = Instant::now() + INDEX_WAIT;
                let readiness = self.measuring("readiness");
                loop {
                    if self.cancelled() {
                        return Err(internal_error("Export cancelled."));
                    }
                    let status = crate::service::index_service_status_typed(index)?;
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
                    let state = lock(&self.state);
                    let _wait = self
                        .idle
                        .wait_timeout(state, Duration::from_millis(100))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                drop(readiness);
                self.phase("Resolving page selections");
                let resolution = self.measuring("resolve");
                let inputs = super::resolve_inputs(&context.root, &request.id, Some(index))?;
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
                if request.format == PublicationFormat::Pdf {
                    Ok(Plan::NotePdf(Box::new(
                        note_pdf::plan_note_pdf(
                            &context.root,
                            &request.id,
                            &page.theme_id,
                            page.profile,
                            page.outside_links.clone(),
                        )
                        .map_err(internal_error)?,
                    )))
                } else {
                    Ok(Plan::Article(Box::new(
                        article::plan_article(
                            &context.root,
                            &request.id,
                            &page.theme_id,
                            page.outside_links.clone(),
                        )
                        .map_err(internal_error)?,
                    )))
                }
            }
        }
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    fn render(&self) -> Result<()> {
        let _phase = self.measuring("rendering");
        let (plan, output) = {
            let mut state = lock(&self.state);
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
                PublicationFormat::Html => "Building HTML document",
                PublicationFormat::Site => "Building website",
            };
            let folder = tempfile::Builder::new()
                .prefix("bluink-export-")
                .tempdir()?;
            let output = folder.path().join(match self.request.format {
                PublicationFormat::Pdf => "document.pdf",
                PublicationFormat::Html => "document.html",
                PublicationFormat::Site => "site",
            });
            state.output = Some(output.clone());
            state.folder = Some(folder);
            (plan, output)
        };
        let hash = matches!(plan.theme().origin, ThemeOrigin::Project)
            .then_some(plan.theme().sha256.as_str());
        let context = &self.context;
        match plan.as_ref() {
            Plan::Article(plan) => {
                super::render_article(
                    &context.root,
                    plan,
                    Some(&output),
                    hash,
                    &context.resources(),
                )?;
            }
            Plan::NotePdf(plan) => {
                super::render_note_pdf(&context.root, plan, &output, hash, &context.resources())?;
            }
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
        let mut state = lock(&self.state);
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
    sweeper: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct Manager {
    registry: Mutex<Registry>,
    changed: Condvar,
    cleanup: Mutex<()>,
}

impl Manager {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    pub(crate) fn create(
        self: &Arc<Self>,
        context: Context,
        request: ExportRequest,
    ) -> Result<JobId> {
        // Fail resource allocation before registering work, and serialize create
        // with stop so shutdown cannot miss an accepted planning thread.
        Scheduler::global()?;
        let mut registry = lock(&self.registry);
        if registry.stopped {
            return Err(internal_error("The application is shutting down."));
        }
        if registry.sweeper.is_none() {
            let manager = Arc::downgrade(self);
            registry.sweeper = Some(
                thread::Builder::new()
                    .name("bluink-export-expiry".to_owned())
                    .spawn(move || Self::sweep(manager))?,
            );
        }
        let id = uuid::Uuid::now_v7().to_string();
        let job = Arc::new(Job {
            id: id.clone(),
            span: tracing::info_span!(target: "bluink.export", "export.job",
                export_seq = SEQUENCE.fetch_add(1, Ordering::Relaxed)),
            created: Instant::now(),
            request,
            context,
            state: Mutex::new(JobState {
                state: State::Planning,
                phase: "Preparing export",
                plan: None,
                folder: None,
                output: None,
                preview: None,
                bundle: None,
                error: None,
                busy: 1,
            }),
            idle: Condvar::new(),
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
        *lock(&job.planning) = Some(thread);
        registry.jobs.insert(id.clone(), job);
        self.changed.notify_all();
        Ok(JobId { job_id: id })
    }

    fn prepare(&self, job: &Arc<Job>) -> Result<()> {
        let plan = Arc::new(job.plan()?);
        let approved = lock(&self.registry).approved.contains(&plan.theme().sha256);
        let mut state = lock(&job.state);
        if state.state == State::Cancelled {
            return Ok(());
        }
        let needs_approval = matches!(plan.theme().origin, ThemeOrigin::Project) && !approved;
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
        state.state = State::Queued;
        state.phase = "Waiting to generate";
        let work = Work::begin(job, state);
        let rendering = Arc::clone(job);
        let queued = Instant::now();
        let _span = job.span.enter();
        scheduler.enqueue(
            job.request.format == PublicationFormat::Pdf,
            job.id.clone(),
            move || {
                let _span = rendering.span.enter();
                tracing::debug!(target: "bluink.export", event = "export_dequeued",
                    queue_ms = queued.elapsed().as_secs_f64() * 1000.0);
                rendering.attempt(|| rendering.render());
                drop(work);
            },
        );
        Ok(())
    }
    #[expect(
        clippy::significant_drop_tightening,
        reason = "Keep job and registry state guarded through lifecycle updates and notifications."
    )]
    fn fresh(&self, id: &str) -> Result<Arc<Job>> {
        let registry = lock(&self.registry);
        let job = registry
            .jobs
            .get(id)
            .ok_or_else(|| internal_error("Export was not found."))?;
        if job.created.elapsed() >= LIFETIME {
            self.changed.notify_all();
            return Err(internal_error("Export expired. Prepare it again."));
        }
        Ok(Arc::clone(job))
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn read(&self, request: ReadRequest) -> Result<Status> {
        let job = self.fresh(&request.job_id)?;
        let approved = lock(&self.registry).approved.clone();
        let state = lock(&job.state);
        let theme_trust = state
            .plan
            .as_ref()
            .filter(|plan| matches!(plan.theme().origin, ThemeOrigin::Project))
            .map(|plan| ThemeTrust {
                hash: plan.theme().sha256.clone(),
                trusted: approved.contains(&plan.theme().sha256),
                root: job
                    .context
                    .root
                    .join("themes")
                    .join(&plan.theme().id)
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
        let mut state = lock(&job.state);
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
        let mut registry = lock(&self.registry);
        let mut state = lock(&job.state);
        if state.state != State::AwaitingThemeApproval
            || state
                .plan
                .as_ref()
                .is_none_or(|plan| plan.theme().sha256 != request.hash)
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
        let job = lock(&self.registry).jobs.get(&request.job_id).cloned();
        let Some(job) = job else {
            return Cancelled { cancelled: false };
        };
        {
            let mut state = lock(&job.state);
            if matches!(state.state, State::Ready | State::Failed | State::Cancelled) {
                return Cancelled { cancelled: false };
            }
            state.state = State::Cancelled;
            state.phase = "Cancelled";
        }
        job.idle.notify_all();
        if let Ok(scheduler) = Scheduler::global() {
            scheduler.cancel(&job.id);
        }
        Cancelled { cancelled: true }
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The service consumes its decoded request or owned work at the application boundary."
    )]
    pub(crate) fn preview(&self, request: JobRequest) -> Result<PreviewUrl> {
        let job = self.fresh(&request.job_id)?;
        let mut state = lock(&job.state);
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
        let Ok(job) = self.fresh(id) else {
            return Ok(None);
        };
        let (root, lease) = {
            let mut state = lock(&job.state);
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
    pub(crate) fn site_zip(&self, id: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let Ok(job) = self.fresh(id) else {
            return Ok(None);
        };
        let (root, work) = {
            let mut state = lock(&job.state);
            if state.state != State::Ready || job.request.format != PublicationFormat::Site {
                return Ok(None);
            }
            if let Some(bundle) = &state.bundle {
                return Ok(Some(Arc::clone(bundle)));
            }
            let root = state
                .output
                .clone()
                .ok_or_else(|| internal_error("Generated site is unavailable."))?;
            (root, Work::begin(&job, &mut state))
        };
        let bundle = Arc::new(artifacts::site_zip(&root)?);
        let mut state = lock(&job.state);
        let result = if state.state == State::Ready {
            state.bundle = Some(Arc::clone(&bundle));
            Some(bundle)
        } else {
            None
        };
        drop(state);
        drop(work);
        Ok(result)
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
        let mut registry = lock(&self.registry);
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
            let registry = lock(&manager.registry);
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
        let _cleanup = lock(&self.cleanup);
        let (jobs, sweeper) = {
            let mut registry = lock(&self.registry);
            registry.stopped = true;
            (
                registry.jobs.values().cloned().collect::<Vec<_>>(),
                registry.sweeper.take(),
            )
        };
        self.changed.notify_all();
        for job in &jobs {
            job.cancel();
        }
        if let Some(sweeper) = sweeper
            && sweeper.join().is_err()
        {
            tracing::error!(target: "bluink.export", "Export expiry thread failed");
        }
        for job in jobs {
            job.finish();
        }
        let mut registry = lock(&self.registry);
        registry.jobs.clear();
        registry.approved.clear();
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
        let path = lock(&job.state).output.clone().unwrap();
        let mut artifact = manager.open_artifact(&job.id, "").unwrap().unwrap();
        assert_eq!(lock(&job.state).busy, 1);
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
        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("document.pdf");
        std::fs::write(&output, b"temporary output").unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let job = Arc::new(Job {
            id: id.clone(), span: tracing::Span::none(), created: Instant::now(),
            request: serde_json::from_value(json!({"source": {"kind": "note", "id": "00000000-0000-7000-8000-000000000601"},
                "format": "pdf", "start": false, "themeId": "classic", "outsideLinks": "error", "profile": "default"})).unwrap(),
            context: Context { root: folder.path().to_owned(), index: None, web_fonts: None, pdf_fonts: None, typst: None },
            state: Mutex::new(JobState { state, phase: "Test", plan: None, folder: Some(folder), output: Some(output), preview: None, bundle: None, error: None, busy: 0 }),
            idle: Condvar::new(), planning: Mutex::new(None),
        });
        lock(&manager.registry).jobs.insert(id, Arc::clone(&job));
        job
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
            paths.push(lock(&job.state).output.clone().unwrap());
        }
        assert!(manager.expired(Instant::now()).is_empty());
        let expired = manager.expired(Instant::now() + LIFETIME);
        assert_eq!(expired.len(), 8);
        for job in expired {
            job.cancel();
            job.finish();
        }
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(lock(&manager.registry).jobs.is_empty());
        manager.stop();
        manager.stop();
    }

    #[test]
    fn cancellation_keeps_active_capacity_and_shutdown_waits_for_cleanup() {
        let first = Arc::new(Manager::default());
        let second = Manager::default();
        let active = job(&first, State::Rendering);
        let queued = job(&second, State::Queued);
        let next = job(&second, State::Queued);
        let output = lock(&active.state).output.clone().unwrap();
        let scheduler = Scheduler::global().unwrap();
        let (started, events) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let work = Work::begin(&active, &mut lock(&active.state));
        let started_active = started.clone();
        scheduler.enqueue(true, active.id.clone(), move || {
            started_active.send("active").unwrap();
            wait.recv().unwrap();
            drop(work);
        });
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            "active"
        );
        for (job, label) in [(&queued, "cancelled"), (&next, "next")] {
            let work = Work::begin(job, &mut lock(&job.state));
            let started = started.clone();
            scheduler.enqueue(true, job.id.clone(), move || {
                started.send(label).unwrap();
                drop(work);
            });
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
        assert_eq!(lock(&queued.state).busy, 0);
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
        let output = lock(&job.state).output.clone().unwrap();
        let work = Work::begin(&job, &mut lock(&job.state));
        job.attempt(|| panic!("injected render failure"));
        assert_eq!(lock(&job.state).state, State::Failed);
        assert!(output.exists());
        drop(work);
        assert!(!output.exists());
        assert_eq!(lock(&job.state).busy, 0);
        manager.stop();
    }

    #[test]
    fn expiry_joins_a_preview_before_removing_its_site() {
        let manager = Manager::default();
        let job = job(&manager, State::Ready);
        let mut state = lock(&job.state);
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
