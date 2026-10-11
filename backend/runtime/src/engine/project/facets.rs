// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::engine::project::io::create_json_atomic;
use crate::engine::project::properties::is_snake_case;
use crate::engine::project::write::{MutationError, lock_relationships};

pub(crate) fn list_facets(root: &Path, kind: Option<&str>) -> Result<Value, String> {
    if kind.is_some_and(|kind| !matches!(kind, "tag" | "label")) {
        return Err("Facet kind must be tag or label.".to_owned());
    }
    with_facets_lock(root, true, || {
        let mut markers = Vec::new();
        scan_facets(&root.join("facets"), &[], kind, &mut markers)?;
        let mut facets = read_markers(&markers)?;
        facets.sort_by(|left, right| {
            let left_kind = left["kind"].as_str().unwrap_or("");
            let right_kind = right["kind"].as_str().unwrap_or("");
            left_kind.cmp(right_kind).then_with(|| {
                left["key"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(right["key"].as_str().unwrap_or(""))
            })
        });
        Ok(Value::Array(facets))
    })
}

pub(crate) fn create_facet(root: &Path, kind: &str, key: &str) -> Result<Value, MutationError> {
    let path = facet_path(root, kind, key).map_err(MutationError::Expected)?;
    let _lock = facets_lock(root, false).map_err(MutationError::Unexpected)?;
    if path
        .try_exists()
        .map_err(|error| MutationError::Unexpected(error.to_string()))?
    {
        return Err(MutationError::Expected(format!(
            "Facet {key}.{kind} already exists."
        )));
    }
    create_json_atomic(&path, &json!({"format":"bluink.facet","formatVersion":1}))
        .map_err(MutationError::Unexpected)?;
    Ok(json!({"key":key,"kind":kind}))
}

pub(crate) fn remove_facet(root: &Path, kind: &str, key: &str) -> Result<(), MutationError> {
    let path = facet_path(root, kind, key).map_err(MutationError::Expected)?;
    let _relationships = lock_relationships(root).map_err(MutationError::Unexpected)?;
    if crate::engine::project::usage::facet_used(root, kind, key)
        .map_err(MutationError::Unexpected)?
    {
        return Err(MutationError::Expected(
            "A facet declaration can be removed only when it is unused.".to_owned(),
        ));
    }
    let _lock = facets_lock(root, false).map_err(MutationError::Unexpected)?;
    fs::remove_file(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            // Windows also reports NotFound when an ancestor is a file. Only
            // classify an absent declaration as a user error after checking
            // the nearest existing ancestor, including missing nested paths.
            for parent in path.ancestors().skip(1) {
                match fs::metadata(parent) {
                    Ok(metadata) if metadata.is_dir() => break,
                    Ok(_) => {
                        return MutationError::Unexpected(format!(
                            "{} is not a directory.",
                            parent.display()
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return MutationError::Unexpected(format!("{}: {error}", parent.display()));
                    }
                }
            }
            MutationError::Expected(format!("Facet {key}.{kind} does not exist."))
        } else {
            MutationError::Unexpected(format!("{}: {error}", path.display()))
        }
    })
}

fn with_facets_lock<T>(
    root: &Path,
    shared: bool,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _lock = facets_lock(root, shared)?;
    operation()
}

fn facets_lock(root: &Path, shared: bool) -> Result<fs::File, String> {
    let directory = root.join(".cache");
    fs::create_dir_all(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join("facet-declarations.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    // Independent catalogue readers can run together; marker writes still
    // exclude readers for the duration of their canonical mutation.
    if shared {
        FileExt::lock_shared(&lock)
    } else {
        lock.lock_exclusive()
    }
    .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(lock)
}

fn facet_path(root: &Path, kind: &str, key: &str) -> Result<PathBuf, String> {
    validate_facet_key(kind, key)?;
    let segments: Vec<&str> = if kind == "tag" {
        key.split('/').collect()
    } else {
        vec![key]
    };
    let mut path = root.join("facets");
    for segment in &segments[..segments.len() - 1] {
        path.push(segment);
    }
    path.push(format!("{}.{}", segments.last().unwrap(), kind));
    Ok(path)
}

fn validate_facet_key(kind: &str, key: &str) -> Result<(), String> {
    if !matches!(kind, "tag" | "label") {
        return Err("Facet kind must be tag or label.".to_owned());
    }
    if kind == "label" {
        return if is_snake_case(key) {
            Ok(())
        } else {
            Err("Label keys must be lowercase snake-case tokens.".to_owned())
        };
    }
    let valid = key.split('/').enumerate().all(|(index, segment)| {
        is_snake_case(segment)
            || (index > 0
                && (1..=4).contains(&segment.len())
                && segment.bytes().all(|byte| byte.is_ascii_digit()))
    });
    if valid {
        Ok(())
    } else {
        Err(
            "Tag keys must be lowercase snake-case paths; numeric leaf segments are allowed."
                .to_owned(),
        )
    }
}

struct FacetMarker {
    path: PathBuf,
    key: String,
    kind: String,
}

fn read_marker(marker: &FacetMarker) -> Result<Value, String> {
    let path = &marker.path;
    let value: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    if value.get("format").and_then(Value::as_str) != Some("bluink.facet")
        || value.get("formatVersion").and_then(Value::as_u64) != Some(1)
    {
        return Err(format!(
            "{} is not a supported Bluink facet marker.",
            path.display()
        ));
    }
    Ok(json!({"key":marker.key,"kind":marker.kind}))
}

fn read_markers(markers: &[FacetMarker]) -> Result<Vec<Value>, String> {
    // Thousands of tiny canonical files otherwise incur serial disk latency on
    // the first request after startup. Reuse the indexer's Rayon pool while the
    // shared catalogue lock is held; validation errors still fail the request.
    crate::parallel::files(|| markers.par_iter().map(read_marker).collect())
}

fn scan_facets(
    directory: &Path,
    segments: &[String],
    requested_kind: Option<&str>,
    markers: &mut Vec<FacetMarker>,
) -> Result<(), String> {
    for entry in
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            if requested_kind == Some("label") {
                continue;
            }
            let mut nested = segments.to_vec();
            nested.push(entry.file_name().to_string_lossy().into_owned());
            scan_facets(&entry.path(), &nested, requested_kind, markers)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((leaf, kind)) = name.rsplit_once('.') else {
            continue;
        };
        if !matches!(kind, "tag" | "label") || (kind == "label" && !segments.is_empty()) {
            continue;
        }
        if requested_kind.is_some_and(|requested| requested != kind) {
            continue;
        }
        let key = segments
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(leaf))
            .collect::<Vec<_>>()
            .join("/");
        if validate_facet_key(kind, &key).is_err() {
            continue;
        }
        markers.push(FacetMarker {
            path: entry.path(),
            key,
            kind: kind.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_facet, list_facets, remove_facet};
    use crate::engine::project::write::MutationError;
    use serde_json::json;

    #[test]
    fn reads_nested_markers_and_owns_changes() {
        let root = std::env::temp_dir().join(format!("bluink-facets-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("facets")).unwrap();
        std::fs::create_dir_all(root.join("items")).unwrap();
        create_facet(&root, "tag", "births/1900").unwrap();
        create_facet(&root, "label", "foreshadowing").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("facets/births/1900.tag")).unwrap(),
            r#"{"format":"bluink.facet","formatVersion":1}"#
        );
        assert_eq!(
            list_facets(&root, None).unwrap(),
            json!([
                {"key":"foreshadowing","kind":"label"},
                {"key":"births/1900","kind":"tag"}
            ])
        );
        assert_eq!(
            list_facets(&root, Some("label")).unwrap(),
            json!([{"key":"foreshadowing","kind":"label"}])
        );
        assert_eq!(
            list_facets(&root, Some("tag")).unwrap(),
            json!([{"key":"births/1900","kind":"tag"}])
        );
        assert!(list_facets(&root, Some("unknown")).is_err());
        assert!(
            create_facet(&root, "tag", "births/1900")
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert!(
            create_facet(&root, "tag", "Not valid")
                .unwrap_err()
                .to_string()
                .contains("snake-case")
        );
        assert!(
            create_facet(&root, "label", "births/1900")
                .unwrap_err()
                .to_string()
                .contains("snake-case")
        );
        remove_facet(&root, "tag", "births/1900").unwrap();
        assert!(
            remove_facet(&root, "tag", "births/1900")
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removal_distinguishes_absent_nested_tags_from_blocked_parent_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("facets/valid")).unwrap();
        std::fs::create_dir_all(root.path().join("items")).unwrap();
        for key in ["absent", "valid/absent", "missing/nested/absent"] {
            assert!(
                matches!(
                    remove_facet(root.path(), "tag", key),
                    Err(MutationError::Expected(_))
                ),
                "An absent tag is a user error: {key}"
            );
        }
        std::fs::write(root.path().join("facets/blocker"), "not a directory").unwrap();
        for key in ["blocker/child", "blocker/missing/child"] {
            let result = remove_facet(root.path(), "tag", key);
            assert!(
                matches!(result, Err(MutationError::Unexpected(_))),
                "A blocked parent is a filesystem error: {key}: {result:?}"
            );
        }
        // Failed removal must not delete the blocking file.
        assert_eq!(
            std::fs::read_to_string(root.path().join("facets/blocker")).unwrap(),
            "not a directory"
        );
    }

    #[test]
    fn label_reads_do_not_open_unrelated_tag_markers() {
        let root = std::env::temp_dir().join(format!("bluink-facets-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("facets")).unwrap();
        create_facet(&root, "label", "review").unwrap();
        std::fs::write(root.join("facets/broken.tag"), "invalid JSON").unwrap();
        assert_eq!(
            list_facets(&root, Some("label")).unwrap(),
            json!([{"key":"review","kind":"label"}])
        );
        assert!(list_facets(&root, Some("tag")).is_err());
        assert!(list_facets(&root, None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_catalogue_reads_are_sorted_and_reject_corrupt_markers() {
        let root = std::env::temp_dir().join(format!("bluink-facets-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("facets")).unwrap();
        for index in (0..128).rev() {
            create_facet(&root, "tag", &format!("topic/item_{index:03}")).unwrap();
        }
        let expected: Vec<_> = (0..128)
            .map(|index| json!({"key":format!("topic/item_{index:03}"),"kind":"tag"}))
            .collect();
        assert_eq!(list_facets(&root, Some("tag")).unwrap(), json!(expected));
        let broken = root.join("facets/topic/item_064.tag");
        for contents in [
            "invalid JSON",
            r#"{"format":"bluink.facet","formatVersion":2}"#,
        ] {
            std::fs::write(&broken, contents).unwrap();
            assert!(
                list_facets(&root, Some("tag"))
                    .unwrap_err()
                    .contains("item_064.tag")
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
