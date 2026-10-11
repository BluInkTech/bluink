// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical named snippets. Files own identity and revisions; indexes are disposable.
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::engine::document::links::valid_external_url;
use crate::engine::document::validation::validate_document;
use crate::engine::project::config::read_project_config;
use crate::engine::project::io::{create_json_atomic, write_json_atomic};
use crate::engine::project::item::{
    is_canonical_instant, is_item_id, list_project_item_ids, read_project_item,
};
use crate::engine::project::properties::is_snake_case;
use crate::engine::project::write::{MutationError, lock_relationships};

pub(crate) fn validate_snippet_id(id: &str) -> Result<(), String> {
    if id.len() > 128 || !is_snake_case(id) {
        return Err(
            "Snippet keys must be lowercase snake-case tokens of at most 128 characters."
                .to_owned(),
        );
    }
    Ok(())
}

/// The terminal paragraph profile is shared by persistence and export planning.
pub(crate) fn validate_reference_paragraph(
    paragraph: &Value,
    external_only: bool,
) -> Result<(), String> {
    let object = paragraph
        .as_object()
        .ok_or("Reference content must be one paragraph.")?;
    if object.get("type").and_then(Value::as_str) != Some("paragraph")
        || object
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "content"))
    {
        return Err("Reference content must be one paragraph without attributes.".to_owned());
    }
    if let Some(content) = object.get("content") {
        for child in content
            .as_array()
            .ok_or("Paragraph content must be an array.")?
        {
            let text = child
                .as_object()
                .ok_or("Reference paragraphs contain text only.")?;
            if text.get("type").and_then(Value::as_str) != Some("text")
                || text
                    .keys()
                    .any(|key| !matches!(key.as_str(), "type" | "text" | "marks"))
            {
                return Err(
                    "Reference paragraphs contain text only; nested content is unsupported."
                        .to_owned(),
                );
            }
            if let Some(marks) = text.get("marks") {
                for mark in marks.as_array().ok_or("Text marks must be an array.")? {
                    let mark = mark
                        .as_object()
                        .ok_or("Invalid reference formatting mark.")?;
                    if mark
                        .keys()
                        .any(|key| !matches!(key.as_str(), "type" | "attrs"))
                    {
                        return Err("Unsupported reference formatting attributes.".to_owned());
                    }
                    let kind = mark.get("type").and_then(Value::as_str).unwrap_or("");
                    if !matches!(
                        kind,
                        "strong"
                            | "emphasis"
                            | "underline"
                            | "strike"
                            | "code"
                            | "link"
                            | "superscript"
                            | "subscript"
                    ) {
                        return Err(format!("Unsupported reference formatting mark: {kind}."));
                    }
                    if let Some(attrs) = mark.get("attrs") {
                        let attrs = attrs
                            .as_object()
                            .ok_or("Formatting attributes must be an object.")?;
                        if attrs.keys().any(|key| kind != "link" || key != "href") {
                            return Err("Reference formatting cannot contain metadata or browser attributes.".to_owned());
                        }
                    }
                    if kind == "link" {
                        let href = mark
                            .get("attrs")
                            .and_then(|attrs| attrs.get("href"))
                            .and_then(Value::as_str)
                            .ok_or("Reference link has no destination.")?;
                        let local = !external_only
                            && (href.strip_prefix("/?note=").is_some_and(is_item_id)
                                || (href.starts_with('#')
                                    && href.len() > 1
                                    && !href.chars().any(char::is_whitespace)));
                        if !valid_external_url(href) && !local {
                            return Err("Reference link must be a valid external URL; snippets cannot link to pages.".to_owned());
                        }
                    }
                }
            }
        }
    }
    validate_document(&json!({"type":"doc", "content":[paragraph]}))
        .map_err(|error| error.to_string())
}

fn path(root: &Path, id: &str) -> Result<PathBuf, String> {
    validate_snippet_id(id)?;
    let directory = root.join("snippets");
    if fs::symlink_metadata(&directory).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("The snippet directory cannot be a symbolic link.".to_owned());
    }
    let path = directory.join(format!("snippet_{id}.json"));
    if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("A snippet file cannot be a symbolic link.".to_owned());
    }
    Ok(path)
}

pub(crate) fn read_snippet(root: &Path, id: &str) -> Result<Option<Value>, String> {
    let path = path(root, id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let record: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("Snippet {id}: {error}"))?;
    let object = record
        .as_object()
        .ok_or("Snippet record must be an object.")?;
    if record["format"] != "bluink.snippet"
        || record["formatVersion"] != 1
        || record["id"] != id
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "format"
                    | "formatVersion"
                    | "id"
                    | "state"
                    | "revision"
                    | "createdAt"
                    | "modifiedAt"
                    | "paragraph"
            )
        })
    {
        return Err(format!(
            "Snippet {id} has an unsupported or mismatched record."
        ));
    }
    if !matches!(record["state"].as_str(), Some("active" | "archived")) {
        return Err(format!("Snippet {id} has an invalid state."));
    }
    if record["revision"].as_str().is_none_or(str::is_empty)
        || !["createdAt", "modifiedAt"]
            .iter()
            .all(|key| record[key].as_str().is_some_and(is_canonical_instant))
    {
        return Err(format!(
            "Snippet {id} has an invalid revision or timestamp."
        ));
    }
    validate_reference_paragraph(&record["paragraph"], true)
        .map_err(|error| format!("Snippet {id}: {error}"))?;
    Ok(Some(record))
}
#[expect(
    clippy::needless_pass_by_value,
    reason = "The operation consumes its input snapshot rather than exposing a borrowed transaction API."
)]
pub(crate) fn write_snippet(
    root: &Path,
    id: &str,
    paragraph: Value,
    expected_revision: Option<&str>,
) -> Result<Value, MutationError> {
    validate_snippet_id(id).map_err(MutationError::Expected)?;
    validate_reference_paragraph(&paragraph, true).map_err(MutationError::Expected)?;
    read_project_config(root)
        .map_err(MutationError::Unexpected)?
        .ok_or_else(|| MutationError::Unexpected("Project configuration is missing.".to_owned()))?;
    let _relationships = lock_relationships(root).map_err(MutationError::Unexpected)?;
    let existing = read_snippet(root, id).map_err(MutationError::Unexpected)?;
    match (existing.as_ref(), expected_revision) {
        (Some(_), None) => {
            return Err(MutationError::Expected(format!(
                "Snippet {id} already exists."
            )));
        }
        (None, Some(_)) => {
            return Err(MutationError::Expected(format!(
                "Snippet {id} no longer exists."
            )));
        }
        (Some(record), Some(revision)) if record["revision"] != revision => {
            return Err(MutationError::Expected(format!(
                "Snippet {id} changed since it was read."
            )));
        }
        _ => {}
    }
    if existing
        .as_ref()
        .is_some_and(|record| record["state"] == "archived")
    {
        return Err(MutationError::Expected(format!(
            "Snippet {id} is archived."
        )));
    }
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let next = json!({
        "format":"bluink.snippet", "formatVersion":1, "id":id, "state":"active",
        "revision":uuid::Uuid::now_v7().to_string(),
        "createdAt":existing.as_ref().map_or_else(|| json!(timestamp), |record| record["createdAt"].clone()),
        "modifiedAt":timestamp, "paragraph":paragraph
    });
    let path = path(root, id).map_err(MutationError::Unexpected)?;
    if existing.is_some() {
        write_json_atomic(&path, &next).map_err(MutationError::Unexpected)?;
    } else {
        create_json_atomic(&path, &next).map_err(MutationError::Unexpected)?;
    }
    Ok(next)
}

pub(crate) fn list_snippets(root: &Path) -> Result<Vec<Value>, String> {
    let directory = root.join("snippets");
    if fs::symlink_metadata(&directory).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("The snippet directory cannot be a symbolic link.".to_owned());
    }
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let filename = entry.file_name();
        let Some(id) = filename
            .to_str()
            .and_then(|name| name.strip_prefix("snippet_"))
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        ids.push(id.to_owned());
    }
    ids.sort_unstable();
    ids.into_iter()
        .map(|id| {
            read_snippet(root, &id)?
                .ok_or_else(|| format!("Snippet {id} disappeared while reading."))
        })
        .collect()
}

fn occurrences(node: &Value, id: &str, block_id: Option<&str>, output: &mut Vec<Value>) {
    let block_id = node["attrs"]["id"].as_str().or(block_id);
    let kind = node["type"].as_str().unwrap_or("");
    if (kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet"))
        && node["attrs"]["snippetId"] == id
    {
        let mut occurrence = json!({"kind":kind});
        if let Some(block_id) = block_id {
            occurrence["blockId"] = json!(block_id);
        }
        output.push(occurrence);
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            occurrences(child, id, block_id, output);
        }
    }
    if let Some(paragraph) = node["attrs"].get("paragraph") {
        occurrences(paragraph, id, block_id, output);
    }
}

pub(crate) fn snippet_usages(root: &Path, id: &str) -> Result<Vec<Value>, String> {
    snippet_usages_in_pages(root, id, &list_project_item_ids(root)?)
}

pub(crate) fn snippet_usages_in_pages(
    root: &Path,
    id: &str,
    pages: &[String],
) -> Result<Vec<Value>, String> {
    validate_snippet_id(id)?;
    let mut usages = Vec::new();
    for page_id in pages {
        let item = read_project_item(root, page_id)?
            .ok_or_else(|| format!("Item {page_id} disappeared while checking usages."))?;
        if item["meta"]["type"] != "page" {
            continue;
        }
        let mut matches = Vec::new();
        occurrences(
            crate::engine::publication::source::document(&item),
            id,
            None,
            &mut matches,
        );
        if !matches.is_empty() {
            usages.push(json!({"noteId":page_id, "noteTitle":item["meta"]["title"].as_str().unwrap_or("Untitled"), "revision":item["revision"], "occurrences":matches}));
        }
    }
    Ok(usages)
}

#[cfg(test)]
pub(crate) fn archive_snippet(
    root: &Path,
    id: &str,
    expected_revision: &str,
) -> Result<Value, MutationError> {
    archive_snippet_with_usage_check(root, id, expected_revision, || snippet_usages(root, id))
}

/// The native bridge can reconcile indexed stamps under the same relationship
/// lock instead of reparsing every unchanged page. The checker must inspect
/// canonical changes, including files not yet reported by the watcher.
pub(crate) fn archive_snippet_with_usage_check(
    root: &Path,
    id: &str,
    expected_revision: &str,
    check_usages: impl FnOnce() -> Result<Vec<Value>, String>,
) -> Result<Value, MutationError> {
    validate_snippet_id(id).map_err(MutationError::Expected)?;
    let path = path(root, id).map_err(MutationError::Unexpected)?;
    let _relationships = lock_relationships(root).map_err(MutationError::Unexpected)?;
    let mut existing = read_snippet(root, id)
        .map_err(MutationError::Unexpected)?
        .ok_or_else(|| MutationError::Expected(format!("Snippet {id} does not exist.")))?;
    if existing["revision"] != expected_revision {
        return Err(MutationError::Expected(format!(
            "Snippet {id} changed since it was read."
        )));
    }
    if existing["state"] == "archived" {
        return Err(MutationError::Expected(format!(
            "Snippet {id} is archived."
        )));
    }
    let usages = check_usages().map_err(MutationError::Unexpected)?;
    if !usages.is_empty() {
        return Err(MutationError::Expected(format!(
            "Snippet {id} is used by {} page(s); remove its references before archiving it.",
            usages.len()
        )));
    }
    existing["state"] = json!("archived");
    existing["revision"] = json!(uuid::Uuid::now_v7().to_string());
    existing["modifiedAt"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
    write_json_atomic(&path, &existing).map_err(MutationError::Unexpected)?;
    Ok(existing)
}

/// Called under the relationship lock, making page saves race-safe with archiving.
pub(crate) fn validate_page_snippet_references(root: &Path, node: &Value) -> Result<(), String> {
    let kind = node["type"].as_str().unwrap_or("");
    if (kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet"))
        && let Some(id) = node["attrs"]["snippetId"].as_str()
    {
        let record =
            read_snippet(root, id)?.ok_or_else(|| format!("Snippet {id} does not exist."))?;
        if record["state"] != "active" {
            return Err(format!("Snippet {id} is archived."));
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            validate_page_snippet_references(root, child)?;
        }
    }
    if let Some(paragraph) = node["attrs"].get("paragraph") {
        validate_page_snippet_references(root, paragraph)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_shared_reference_paragraph_contract() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../../test-fixtures/document-contract/reference-paragraphs.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            assert_eq!(
                validate_reference_paragraph(
                    &case["paragraph"],
                    case["externalOnly"].as_bool().unwrap()
                )
                .is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("bluink-snippets-{}", uuid::Uuid::now_v7()));
        crate::engine::project::config::open_or_create_project(&root, "UTC").unwrap();
        root
    }
    fn paragraph(text: &str) -> Value {
        json!({"type":"paragraph", "content":[{"type":"text", "text":text}]})
    }

    #[test]
    fn named_identity_round_trips_and_revisions_guard_writes() {
        let root = project();
        let created = write_snippet(&root, "con", paragraph("First"), None).unwrap();
        assert!(root.join("snippets/snippet_con.json").is_file());
        assert_eq!(read_snippet(&root, "con").unwrap(), Some(created.clone()));
        assert!(
            write_snippet(&root, "con", paragraph("Duplicate"), None)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        let updated = write_snippet(
            &root,
            "con",
            paragraph("Changed"),
            created["revision"].as_str(),
        )
        .unwrap();
        assert_eq!(updated["createdAt"], created["createdAt"]);
        assert_ne!(updated["revision"], created["revision"]);
        assert!(
            write_snippet(
                &root,
                "con",
                paragraph("Stale"),
                created["revision"].as_str()
            )
            .unwrap_err()
            .to_string()
            .contains("changed since")
        );
        assert!(
            archive_snippet(&root, "con", created["revision"].as_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("changed since")
        );
        archive_snippet(&root, "con", updated["revision"].as_str().unwrap()).unwrap();
        let archived = read_snippet(&root, "con").unwrap().unwrap();
        assert_eq!(archived["state"], "archived");
        assert_eq!(archived["paragraph"], updated["paragraph"]);
        assert_ne!(archived["revision"], updated["revision"]);
        assert!(
            write_snippet(&root, "con", paragraph("Duplicate"), None)
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert!(
            write_snippet(
                &root,
                "con",
                paragraph("Edit"),
                archived["revision"].as_str()
            )
            .unwrap_err()
            .to_string()
            .contains("archived")
        );
        assert!(
            validate_page_snippet_references(
                &root,
                &json!({"type":"embed","attrs":{"snippetId":"con"}})
            )
            .unwrap_err()
            .contains("archived")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_identity_content_and_format_errors() {
        let root = project();
        for id in ["../escape", "Smith2024", "", "one/two"] {
            assert!(write_snippet(&root, id, paragraph("Text"), None).is_err());
        }
        assert!(validate_snippet_id(&"a".repeat(129)).is_err());
        for node in [
            json!({"type":"doc","content":[paragraph("Text")]}),
            json!({"type":"paragraph","attrs":{"labels":["claim"]}}),
            json!({"type":"paragraph","content":[{"type":"hard_break"}]}),
            json!({"type":"paragraph","content":[{"type":"text","text":"Link","marks":[{"type":"link","attrs":{"href":"/?note=one"}}]}]}),
        ] {
            assert!(write_snippet(&root, "source", node, None).is_err());
        }
        let mut created = write_snippet(&root, "source", paragraph("Text"), None).unwrap();
        created["id"] = json!("wrong");
        write_json_atomic(&root.join("snippets/snippet_source.json"), &created).unwrap();
        assert!(
            read_snippet(&root, "source")
                .unwrap_err()
                .contains("mismatched")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[expect(
        clippy::needless_collect,
        reason = "Collect all worker handles before joining so work remains concurrent."
    )]
    fn concurrent_creation_has_one_winner() {
        let root = project();
        let outcomes = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| write_snippet(&root, "shared", paragraph("One"), None)))
                .collect();
            tasks
                .into_iter()
                .map(|task| task.join().unwrap().is_ok())
                .collect::<Vec<_>>()
        });
        assert_eq!(outcomes.iter().filter(|success| **success).count(), 1);
        assert_eq!(list_snippets(&root).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn counts_reference_occurrences_instead_of_distinct_targets() {
        let doc = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"block"},"content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},{"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}]},{"type":"embed","attrs":{"id":"embed","snippetId":"shared"}}]});
        let mut matches = Vec::new();
        occurrences(&doc, "shared", None, &mut matches);
        assert_eq!(
            matches,
            vec![
                json!({"kind":"citation","blockId":"block"}),
                json!({"kind":"citation","blockId":"block"}),
                json!({"kind":"embed","blockId":"embed"})
            ]
        );
    }

    #[test]
    fn canonical_pages_guard_snippet_integrity_including_archived_usages() {
        let root = project();
        let snippet = write_snippet(&root, "shared", paragraph("Reusable"), None).unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let page = crate::engine::project::write::write_item_input(
            &root,
            json!({"id":id,"meta":{"type":"page","title":"Reference owner","state":"archived"},
                "content":{"type":"doc","content":[{"type":"paragraph","content":[
                    {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},
                    {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}}
                ]},{"type":"embed","attrs":{"snippetId":"shared"}}]}}),
            None,
            "2026-10-01",
        )
        .unwrap();
        let usages = snippet_usages(&root, "shared").unwrap();
        assert_eq!(usages[0]["noteId"], id);
        assert_eq!(usages[0]["occurrences"].as_array().unwrap().len(), 3);
        assert!(
            archive_snippet(&root, "shared", snippet["revision"].as_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("used")
        );
        crate::engine::project::write::write_item_input(
            &root,
            json!({"id":id,"meta":{},"content":{"type":"doc","content":[paragraph("No dependency")]}}),
            page["item"]["revision"].as_str(),
            "2026-10-01",
        )
        .unwrap();
        archive_snippet(&root, "shared", snippet["revision"].as_str().unwrap()).unwrap();
        assert!(crate::engine::project::write::write_item_input(
            &root,
            json!({"id":id,"meta":{},"content":{"type":"doc","content":[{"type":"embed","attrs":{"snippetId":"missing"}}]}}),
            read_project_item(&root, &id).unwrap().unwrap()["revision"].as_str(),
            "2026-10-01",
        ).unwrap_err().contains("does not exist"));
        fs::remove_dir_all(root).unwrap();
    }
}
