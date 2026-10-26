// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::Value;

fn is_document_item(item: &Value) -> bool {
    matches!(item["meta"]["type"].as_str(), Some("page" | "template"))
}

/// The runtime supplies candidate selection; the engine owns canonical semantics.
#[derive(Clone, Copy)]
pub(crate) enum Usage<'a> {
    Tag(&'a str),
    Label(&'a str),
    Property(&'a str),
    Snippet(&'a str),
}

impl Usage<'_> {
    pub(crate) fn facet<'a>(kind: &str, key: &'a str) -> Result<Usage<'a>, String> {
        match kind {
            "tag" => Ok(Usage::Tag(key)),
            "label" => Ok(Usage::Label(key)),
            _ => Err("Facet kind must be tag or label.".to_owned()),
        }
    }

    pub(crate) const fn active_owners_only(self) -> bool {
        match self {
            Self::Tag(_) | Self::Label(_) | Self::Property(_) => true,
            Self::Snippet(_) => false,
        }
    }

    pub(crate) fn matches(self, item: &Value) -> bool {
        if self.active_owners_only() && item["meta"]["state"] != "active" {
            return false;
        }
        match self {
            Self::Property(key) => item["meta"].get(key).is_some_and(|value| !value.is_null()),
            Self::Tag(key) => item["tags"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(key))),
            // A template's document protects what it uses, as a page's does.
            Self::Label(key) if is_document_item(item) => {
                crate::engine::document::projection::project_document(
                    crate::engine::publication::source::document(item),
                )
                .labels
                .iter()
                .any(|label| label == key)
            }
            Self::Snippet(key) if is_document_item(item) => {
                let projection = crate::engine::document::projection::project_document(
                    crate::engine::publication::source::document(item),
                );
                projection
                    .snippet_citations
                    .iter()
                    .chain(&projection.snippet_embeds)
                    .any(|id| id == key)
            }
            Self::Label(_) | Self::Snippet(_) => false,
        }
    }

    fn blocked_message(self, count: usize) -> String {
        match self {
            Self::Tag(_) | Self::Label(_) => {
                "A facet declaration can be removed only when it is unused.".to_owned()
            }
            Self::Property(key) => format!("Property {key} is used by one or more items."),
            Self::Snippet(id) => format!(
                "Snippet {id} is used by {count} page(s); remove its references before archiving it."
            ),
        }
    }
}

/// Target validation, relationship verification and persistence share one lock.
/// Usage checks select indexed and pending candidates instead of scanning the
/// project. Item mutations must publish pending revisions before returning.
pub(crate) fn remove_if_unused<T, R>(
    root: &Path,
    usage: Usage<'_>,
    validate: impl FnOnce() -> Result<T, super::write::MutationError>,
    blockers: impl FnOnce() -> Result<usize, super::write::MutationError>,
    mutate: impl FnOnce(T) -> Result<R, super::write::MutationError>,
) -> Result<R, super::write::MutationError> {
    let _relationships =
        super::write::lock_relationships(root).map_err(super::write::MutationError::Storage)?;
    let target = validate()?;
    let count = blockers()?;
    if count != 0 {
        return Err(super::write::MutationError::Validation(
            usage.blocked_message(count),
        ));
    }
    mutate(target)
}

/// Connect canonical writes and relationship checks to the runtime projection.
pub(crate) trait WriteObserver: Send + Sync {
    /// Mark a published revision before releasing the relationship lock.
    /// This callback must not read files or wait for workers.
    fn pending(&self, id: &str, revision: &str) -> Result<(), String>;
    fn item_candidates(&self, kind: &str) -> Result<Vec<String>, String>;
}

type Observers = HashMap<PathBuf, Vec<Weak<dyn WriteObserver>>>;
static OBSERVERS: OnceLock<Mutex<Observers>> = OnceLock::new();

fn observers() -> &'static Mutex<Observers> {
    OBSERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn observe(root: &Path, observer: &Arc<dyn WriteObserver>) -> Result<(), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut entries = observers().lock().map_err(|error| error.to_string())?;
    entries.retain(|_, observers| {
        observers.retain(|observer| observer.strong_count() != 0);
        !observers.is_empty()
    });
    entries
        .entry(root)
        .or_default()
        .push(Arc::downgrade(observer));
    drop(entries);
    Ok(())
}

pub(crate) fn pending(root: &Path, id: &str, revision: &str) -> Result<(), String> {
    for listener in listeners(root)? {
        listener.pending(id, revision)?;
    }
    Ok(())
}

/// Direct engine tools without a runtime retain canonical discovery. An attached
/// runtime must supply a complete baseline, never silently fall back to a scan.
pub(crate) fn item_candidates(root: &Path, kind: &str) -> Result<Option<Vec<String>>, String> {
    listeners(root)?
        .first()
        .map(|listener| listener.item_candidates(kind))
        .transpose()
}

fn listeners(root: &Path) -> Result<Vec<Arc<dyn WriteObserver>>, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let listeners: Vec<_> = observers()
        .lock()
        .map_err(|error| error.to_string())?
        .get(&root)
        .into_iter()
        .flatten()
        .filter_map(Weak::upgrade)
        .collect();
    // No callback runs under the process-wide registry lock.
    Ok(listeners)
}
