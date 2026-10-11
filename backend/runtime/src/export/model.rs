// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::sync::Arc;

use crate::engine::publication::{
    article::ArticlePlan,
    note_pdf::NotePdfPlan,
    plan::{PublicationFormat, PublicationPlan},
    theme::ThemeSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum Plan {
    Article(Box<ArticlePlan>),
    NotePdf(Box<NotePdfPlan>),
    Publication(Box<PublicationPlan>),
}

impl Plan {
    pub(crate) fn theme(&self) -> &ThemeSnapshot {
        match self {
            Self::Article(plan) => &plan.theme,
            Self::NotePdf(plan) => &plan.theme,
            Self::Publication(plan) => &plan.theme,
        }
    }

    pub(crate) fn summary(&self) -> Summary {
        match self {
            Self::Article(plan) => Summary {
                title: plan.title.clone(),
                pages: 1,
                collections: 0,
                assets: plan.assets.len(),
                links: plan.links.len(),
            },
            Self::NotePdf(plan) => Summary {
                title: plan.title.clone(),
                pages: 1,
                collections: 0,
                assets: plan.assets.len(),
                links: plan.links.len(),
            },
            Self::Publication(plan) => Summary {
                title: plan.title.clone(),
                pages: plan.pages.len(),
                collections: plan.collections.len(),
                assets: plan.assets.len(),
                links: plan.links.len(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum State {
    Planning,
    Prepared,
    AwaitingThemeApproval,
    Queued,
    Rendering,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Summary {
    title: String,
    pages: usize,
    collections: usize,
    assets: usize,
    links: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ThemeTrust {
    pub(crate) hash: String,
    pub(crate) trusted: bool,
    pub(crate) root: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub(crate) state: State,
    pub(crate) phase: &'static str,
    pub(crate) format: PublicationFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) plan: Option<Arc<Plan>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) summary: Option<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) theme_trust: Option<ThemeTrust>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) artifact_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) bundle_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) preview_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) error: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct JobRequest {
    pub(crate) job_id: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ReadRequest {
    pub(crate) job_id: String,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<bool>", optional))]
    pub(crate) include_plan: bool,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, ts(optional_fields = nullable))]
pub(crate) struct ApproveRequest {
    pub(crate) job_id: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobId {
    pub(crate) job_id: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Approved {
    pub(crate) trusted: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Cancelled {
    pub(crate) cancelled: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PreviewUrl {
    pub(crate) url: String,
}

#[cfg(test)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Artifact {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) content_type: &'static str,
    pub(crate) site: bool,
}
