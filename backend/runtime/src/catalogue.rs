// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::application::error::{Result, internal_error, invalid_request};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::project::types::{FacetKind, FieldSource, MetadataFieldValueType};
use crate::relations;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataField {
    pub(crate) key: String,
    pub(crate) value_type: MetadataFieldValueType,
    pub(crate) source: FieldSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) system: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) options: Option<Vec<String>>,
    /// How chosen options look; options without an entry are automatic.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) option_appearance:
        Option<std::collections::BTreeMap<String, crate::engine::project::types::ChoiceAppearance>>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub(crate) struct FacetEntry {
    pub(crate) key: String,
    pub(crate) kind: FacetKind,
    pub(crate) usage_count: usize,
    pub(crate) inferred: bool,
}

type DeclaredFacets = Arc<Vec<Value>>;
type FacetDeclarations = Arc<Mutex<Option<DeclaredFacets>>>;
type FacetCache = Mutex<HashMap<(String, String), FacetDeclarations>>;
static FACET_DECLARATIONS: OnceLock<FacetCache> = OnceLock::new();

fn facet_cache() -> &'static FacetCache {
    FACET_DECLARATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn invalidate_facets(project_root: &str) -> Result<()> {
    facet_cache()
        .lock()
        .map_err(crate::application::error::poison_error)?
        .retain(|(root, _), _| root != project_root);
    Ok(())
}
#[expect(
    clippy::significant_drop_tightening,
    reason = "Keep catalogue cache reads and replacement in one guarded operation."
)]
fn declared_facets(project_root: &str, kind: &str) -> Result<DeclaredFacets> {
    // Hold the registry only while locating the entry, never during file I/O.
    // Labels must not queue behind a cold tag catalogue (or another project).
    let entry = facet_cache()
        .lock()
        .map_err(crate::application::error::poison_error)?
        .entry((project_root.to_owned(), kind.to_owned()))
        .or_default()
        .clone();
    let waiting = Instant::now();
    let mut cache = entry
        .lock()
        .map_err(crate::application::error::poison_error)?;
    let cache_wait_ms = waiting.elapsed().as_secs_f64() * 1000.0;
    if let Some(declared) = cache.as_ref() {
        tracing::debug!(
            operation = "facet_declarations",
            kind,
            cache_hit = true,
            cache_wait_ms
        );
        return Ok(Arc::clone(declared));
    }
    let loading = Instant::now();
    let declared = crate::engine::project::facets::list_facets(Path::new(project_root), Some(kind))
        .map_err(internal_error)?;
    let serde_json::Value::Array(declared) = declared else {
        return Err(internal_error("Facet catalogue must be an array."));
    };
    let declared = Arc::new(declared);
    tracing::debug!(
        operation = "facet_declarations",
        kind,
        cache_hit = false,
        cache_wait_ms,
        declaration_load_ms = loading.elapsed().as_secs_f64() * 1000.0,
        declaration_count = declared.len()
    );
    *cache = Some(Arc::clone(&declared));
    Ok(declared)
}

pub(crate) fn read_metadata_fields(project_root: &str) -> Result<Vec<MetadataField>> {
    let declarations =
        crate::engine::project::properties::list_property_declarations(Path::new(project_root))
            .map_err(internal_error)?;
    let mut fields = vec![MetadataField {
        key: "summary".to_owned(),
        value_type: MetadataFieldValueType::Text,
        source: FieldSource::Builtin,
        system: None,
        options: None,
        option_appearance: None,
    }];
    let declarations = declarations
        .as_array()
        .ok_or_else(|| internal_error("Property catalogue must be an array."))?;
    for declaration in declarations {
        fields.push(metadata_field(declaration)?);
    }
    Ok(fields)
}

pub(crate) fn metadata_field(declaration: &Value) -> Result<MetadataField> {
    let key = declaration["key"]
        .as_str()
        .ok_or_else(|| internal_error("Property declaration has no key."))?;
    let system = declaration["system"] == true;
    let options = if declaration["options"]
        .as_array()
        .is_some_and(|options| !options.is_empty())
    {
        Some(Vec::<String>::deserialize(&declaration["options"]).map_err(internal_error)?)
    } else {
        None
    };
    Ok(MetadataField {
        key: key.to_owned(),
        value_type: MetadataFieldValueType::deserialize(&declaration["valueType"])
            .map_err(internal_error)?,
        source: if system {
            FieldSource::Builtin
        } else {
            FieldSource::Registered
        },
        system: system.then_some(true),
        options,
        option_appearance: declaration
            .get("optionAppearance")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(internal_error)?,
    })
}

pub(crate) fn list_facet_entries(
    project_root: &str,
    index_path: &str,
    kind: Option<&str>,
) -> Result<Vec<FacetEntry>> {
    let _span =
        tracing::debug_span!("list_project_facet_entries", kind = kind.unwrap_or("all")).entered();
    if kind.is_some_and(|kind| !matches!(kind, "tag" | "label")) {
        return Err(invalid_request("Facet kind must be tag or label."));
    }
    let mut output = BTreeMap::<(String, String), FacetEntry>::new();
    for current in ["tag", "label"] {
        if kind.is_some_and(|requested| requested != current) {
            continue;
        }
        let declared = declared_facets(project_root, current)?;
        let declared_keys = declared
            .iter()
            .filter_map(|entry| entry["key"].as_str())
            .collect::<HashSet<_>>();
        let aggregation_started = Instant::now();
        let counts = relations::facet_service_sync(
            index_path.to_owned(),
            if current == "tag" { "tags" } else { "labels" }.to_owned(),
            Vec::new(),
            // Usage counts are about notes; templates protect facets but aren't counted.
            Some("page".to_owned()),
        )?;
        tracing::debug!(
            operation = "facet_usage_counts",
            kind = current,
            aggregation_ms = aggregation_started.elapsed().as_secs_f64() * 1000.0
        );
        let usages = counts
            .iter()
            .map(|entry| (entry.tid.as_str(), entry.count))
            .collect::<HashMap<_, _>>();
        let keys = declared_keys
            .iter()
            .copied()
            .chain(usages.keys().copied())
            .collect::<HashSet<_>>();
        for key in keys {
            output.insert(
                (current.to_owned(), key.to_owned()),
                FacetEntry {
                    key: key.to_owned(),
                    kind: if current == "tag" {
                        FacetKind::Tag
                    } else {
                        FacetKind::Label
                    },
                    usage_count: usages.get(key).copied().unwrap_or(0),
                    inferred: !declared_keys.contains(key),
                },
            );
        }
    }

    Ok(output.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::{declared_facets, facet_cache, invalidate_facets};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn label_catalogue_does_not_wait_for_the_tag_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        crate::engine::project::facets::initialize(&root).unwrap();
        crate::engine::project::facets::create_facet(&root, "tag", "research").unwrap();
        crate::engine::project::facets::create_facet(&root, "label", "review").unwrap();
        let root_string = root.to_str().unwrap().to_owned();
        declared_facets(&root_string, "tag").unwrap();
        let entry = facet_cache().lock().unwrap()[&(root_string.clone(), "tag".to_owned())].clone();
        let tag_loading = entry.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        let thread_root = root_string.clone();
        let worker = std::thread::spawn(move || {
            sender.send(declared_facets(&thread_root, "label")).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        drop(tag_loading);
        worker.join().unwrap();
        let labels = result
            .expect("Labels waited for an unrelated tag cache lock")
            .unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0]["key"], "review");
        invalidate_facets(&root_string).unwrap();
        temporary.close().unwrap();
    }
}
