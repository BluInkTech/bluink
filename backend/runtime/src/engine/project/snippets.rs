// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Canonical named snippets. Files own identity and revisions; indexes are disposable.
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use chrono::Utc;
use serde_json::{Value, json};

use crate::engine::document::links::valid_external_url;
use crate::engine::document::validation::validate_document;
#[cfg(test)]
use crate::engine::project::identity::ItemId;
use crate::engine::project::identity::is_item_id;
use crate::engine::project::item::{list_project_item_ids, read_project_item};
use crate::engine::project::properties::is_snake_case;
use crate::engine::project::write::{MutationError, mutate_existing_item, write_item_input_typed};

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

/// Resolve the same deterministic item identity used by declarations and startup.
pub(crate) fn item_id(key: &str) -> Result<String, String> {
    validate_snippet_id(key)?;
    Ok(super::named_items::id("snippet", key))
}

#[cfg(test)]
pub(crate) fn path(root: &Path, key: &str) -> Result<PathBuf, String> {
    Ok(ItemId::try_from(item_id(key)?.as_str())?.path(root))
}

/// Snippet-specific shape validation is also used by ordinary item writes.
pub(crate) fn validate_content(item: &Value) -> Result<(), String> {
    if item["meta"]["type"] != "snippet" {
        return Err("Expected a snippet item.".into());
    }
    let key = item["content"]["key"]
        .as_str()
        .ok_or("Snippet has no key.")?;
    if item["id"] != item_id(key)? {
        return Err("Snippet has a mismatched named identity.".into());
    }
    validate_reference_paragraph(&item["content"]["paragraph"], true)
}

/// The browser/export DTO uses the author's key; persistence uses the item UUID.
pub(crate) fn record(item: &Value) -> Result<Value, String> {
    validate_content(item)?;
    Ok(json!({"format":"bluink.snippet", "formatVersion":1,
        "id":item["content"]["key"], "state":item["meta"]["state"],
        "revision":item["revision"], "createdAt":item["createdAt"],
        "modifiedAt":item["modifiedAt"], "paragraph":item["content"]["paragraph"]}))
}

pub(crate) fn read_snippet(root: &Path, key: &str) -> Result<Option<Value>, String> {
    read_project_item(root, &item_id(key)?)
        .map_err(|error| error.to_string())?
        .map(|item| record(&item))
        .transpose()
}

pub(crate) fn write_snippet(
    root: &Path,
    key: &str,
    paragraph: Value,
    expected_revision: Option<&str>,
) -> Result<Value, MutationError> {
    let id = item_id(key).map_err(MutationError::Validation)?;
    validate_reference_paragraph(&paragraph, true).map_err(MutationError::Validation)?;
    if expected_revision.is_none()
        && read_project_item(root, &id)
            .map_err(MutationError::Read)?
            .is_some()
    {
        return Err(MutationError::Validation(format!(
            "Snippet {key} already exists."
        )));
    }
    let mut input = json!({"id":id,"meta":{"type":"snippet","title":key},"content":{"key":key}});
    input["content"]["paragraph"] = paragraph;
    let result = write_item_input_typed(
        root,
        input,
        expected_revision,
        &Utc::now().date_naive().to_string(),
    )?;
    record(&result["item"]).map_err(MutationError::Internal)
}

/// Called by the common item writer while it owns the relationship lock.
pub(crate) fn validate_transition(
    root: &Path,
    existing: Option<&Value>,
    next: &Value,
) -> Result<(), MutationError> {
    if next["meta"]["type"] != "snippet" {
        return Ok(());
    }
    validate_content(next).map_err(MutationError::Validation)?;
    let key = next["content"]["key"]
        .as_str()
        .ok_or_else(|| MutationError::Validation("Snippet has no key.".into()))?;
    if existing.is_some_and(|item| item["meta"]["state"] == "archived") {
        return Err(MutationError::Validation(format!(
            "Snippet {key} is archived."
        )));
    }
    if next["meta"]["state"] == "archived" {
        let count =
            crate::catalogue_usage::UsageCheck::new(root, super::usage::Usage::Snippet(key))
                .count()
                .map_err(|error| MutationError::Internal(error.to_string()))?;
        if count != 0 {
            return Err(MutationError::Validation(format!(
                "Snippet {key} is used by {count} page(s); remove its references before archiving it."
            )));
        }
    }
    Ok(())
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
        let item = read_project_item(root, page_id)
            .map_err(|error| error.to_string())?
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

pub(crate) fn archive_snippet(
    root: &Path,
    key: &str,
    expected_revision: &str,
) -> Result<Value, MutationError> {
    let id = item_id(key).map_err(MutationError::Validation)?;
    let result = mutate_existing_item(
        root,
        &id,
        Some(expected_revision),
        &Utc::now().date_naive().to_string(),
        |_| Ok(json!({"id":id,"meta":{"state":"archived"}})),
    )?;
    record(&result["item"]).map_err(MutationError::Internal)
}

/// Called under the relationship lock, making page saves race-safe with archiving.
pub(crate) fn validate_page_snippet_references(root: &Path, node: &Value) -> Result<(), String> {
    validate_distinct_snippet_references(root, node, &mut std::collections::HashSet::new())
}

fn validate_distinct_snippet_references<'a>(
    root: &Path,
    node: &'a Value,
    checked: &mut std::collections::HashSet<&'a str>,
) -> Result<(), String> {
    let kind = node["type"].as_str().unwrap_or("");
    if (kind == "embed" || (kind == "citation" && node["attrs"]["source"] == "snippet"))
        && let Some(id) = node["attrs"]["snippetId"].as_str()
        && checked.insert(id)
    {
        let record =
            read_snippet(root, id)?.ok_or_else(|| format!("Snippet {id} does not exist."))?;
        if record["state"] != "active" {
            return Err(format!("Snippet {id} is archived."));
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            validate_distinct_snippet_references(root, child, checked)?;
        }
    }
    if let Some(paragraph) = node["attrs"].get("paragraph") {
        validate_distinct_snippet_references(root, paragraph, checked)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::project::io::write_json_atomic;

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

    fn project() -> tempfile::TempDir {
        let temporary = tempfile::tempdir().unwrap();
        crate::engine::project::config::open_or_create_project(
            &temporary.path().join("project"),
            "UTC",
        )
        .unwrap();
        temporary
    }
    fn paragraph(text: &str) -> Value {
        json!({"type":"paragraph", "content":[{"type":"text", "text":text}]})
    }

    #[test]
    fn named_identity_round_trips_and_revisions_guard_writes() {
        let temporary = project();
        let root = temporary.path().join("project");
        let created = write_snippet(&root, "con", paragraph("First"), None).unwrap();
        assert!(path(&root, "con").unwrap().is_file());
        let canonical = read_project_item(&root, &item_id("con").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(canonical["format"], "bluink.item");
        assert_eq!(canonical["meta"]["type"], "snippet");
        assert_eq!(canonical["content"]["key"], "con");
        assert_eq!(canonical["content"]["paragraph"], paragraph("First"));
        assert!(!root.join("snippets").exists());
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
        assert!(matches!(
            write_snippet(
                &root,
                "con",
                paragraph("Stale"),
                created["revision"].as_str()
            ),
            Err(MutationError::RevisionConflict(_))
        ));
        assert!(matches!(
            archive_snippet(&root, "con", created["revision"].as_str().unwrap()),
            Err(MutationError::RevisionConflict(_))
        ));
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
        temporary.close().unwrap();
    }

    #[test]
    fn rejects_identity_content_and_format_errors() {
        let temporary = project();
        let root = temporary.path().join("project");
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
        write_json_atomic(&path(&root, "source").unwrap(), &created).unwrap();
        assert!(
            read_snippet(&root, "source")
                .unwrap_err()
                .contains("mismatched")
        );
        temporary.close().unwrap();
    }

    #[test]
    #[expect(
        clippy::needless_collect,
        reason = "Collect all worker handles before joining so work remains concurrent."
    )]
    fn concurrent_creation_has_one_winner() {
        let temporary = project();
        let root = temporary.path().join("project");
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
        assert!(read_snippet(&root, "shared").unwrap().is_some());
        temporary.close().unwrap();
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
        let temporary = project();
        let root = temporary.path().join("project");
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
        temporary.close().unwrap();
    }
}
