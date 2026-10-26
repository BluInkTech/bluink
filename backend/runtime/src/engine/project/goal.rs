// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::path::Path;

use serde_json::{Value, json};

use crate::engine::project::item::{list_project_item_ids, read_project_item};
use crate::engine::project::write::{MutationError, write_item_input_typed};

pub(crate) fn create_writing_goal(
    root: &Path,
    request: &Value,
    activity_date: &str,
    validate_query: impl FnOnce(
        &crate::engine::project::query::QueryDocument,
    ) -> Result<(), MutationError>,
) -> Result<Value, MutationError> {
    let scope = request["scope"]
        .as_str()
        .ok_or_else(|| expected("Choose All Notes or a saved view for this writing goal."))?;
    if scope != "all_notes" && scope != "view" {
        return Err(expected(
            "Choose All Notes or a saved view for this writing goal.",
        ));
    }
    let mut content = json!({
        "kind": "writing_challenge",
        "scope": scope,
        "startDate": request["startDate"],
        "endDate": request["endDate"],
        "dailyWordTarget": request["dailyWordTarget"],
    });
    if let Some(total) = request.get("totalWordTarget") {
        content["totalWordTarget"] = total.clone();
    }
    if scope == "view" {
        let view_id = request["viewId"]
            .as_str()
            .ok_or_else(|| expected("Choose a saved view for this writing goal."))?;
        let view = read_project_item(root, view_id)
            .map_err(MutationError::Read)?
            .ok_or_else(|| expected("Writing goals need an active saved view."))?;
        if view["meta"]["type"] != "workspace" || view["meta"]["state"] != "active" {
            return Err(expected("Writing goals need an active saved view."));
        }
        let query: crate::engine::project::query::QueryDocument =
            serde_json::from_value(view["content"]["query"].clone())
                .map_err(|error| expected(&format!("The workspace query is invalid: {error}")))?;
        let scoped = crate::engine::project::query::QueryDocument::active_pages()
            .compose(&query)
            .map_err(|errors| expected(&errors[0].message))?;
        validate_query(&scoped)?;
        content["viewId"] = json!(view_id);
        content["query"] = json!(query);
    } else if request.get("viewId").is_some() {
        return Err(expected("All Notes goals cannot have a saved view."));
    }
    let title = request["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Writing goal");
    write_item_input_typed(
        root,
        json!({"meta": {"type": "goal", "title": title}, "content": content}),
        None,
        activity_date,
    )
}

/// Called while the project relationship lock is held, immediately before a
/// goal becomes active. It protects all goal writers, including direct item writes.
pub(crate) fn validate_goal_activation(
    root: &Path,
    existing: Option<&Value>,
    next: &Value,
) -> Result<(), MutationError> {
    if next["meta"]["type"] != "goal" || next["meta"]["state"] != "active" {
        return Ok(());
    }
    if existing.is_some_and(|item| item["meta"]["state"] == "active") {
        return Ok(());
    }
    let scope = &next["content"]["scope"];
    let view_id = next["content"]["viewId"].as_str();
    if scope == "view" {
        let id = view_id.ok_or_else(|| expected("Choose a saved view for this writing goal."))?;
        let view = read_project_item(root, id)
            .map_err(MutationError::Read)?
            .ok_or_else(|| expected("Writing goals need an active saved view."))?;
        if view["meta"]["type"] != "workspace"
            || view["meta"]["state"] != "active"
            || view["content"]["query"] != next["content"]["query"]
        {
            return Err(expected(
                "Writing goals need an active saved view with the captured query.",
            ));
        }
    }
    let id = next["id"]
        .as_str()
        .ok_or_else(|| expected("Writing goal has no ID."))?;
    let start = next["content"]["startDate"]
        .as_str()
        .ok_or_else(|| expected("Goal has no start date."))?;
    let end = next["content"]["endDate"]
        .as_str()
        .ok_or_else(|| expected("Goal has no end date."))?;
    let candidates =
        match super::usage::item_candidates(root, "goal").map_err(MutationError::Internal)? {
            Some(ids) => ids,
            None => list_project_item_ids(root).map_err(MutationError::Internal)?,
        };
    for other_id in candidates {
        if other_id == id {
            continue;
        }
        let Some(other) = read_project_item(root, &other_id).map_err(MutationError::Read)? else {
            continue;
        };
        if other["meta"]["type"] != "goal" || other["meta"]["state"] != "active" {
            continue;
        }
        if other["content"]["scope"] != *scope
            || (scope == "view" && other["content"]["viewId"].as_str() != view_id)
        {
            continue;
        }
        let other_start = other["content"]["startDate"].as_str().unwrap_or("");
        let other_end = other["content"]["endDate"].as_str().unwrap_or("");
        if other_start <= end && other_end >= start {
            return Err(expected(
                "This scope already has a writing goal for those dates.",
            ));
        }
    }
    Ok(())
}

fn expected(message: &str) -> MutationError {
    MutationError::Validation(message.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use crate::engine::project::config::create_project_config;
    use crate::engine::project::write::write_item_input;

    #[test]
    fn direct_concurrent_writers_cannot_create_overlapping_goals() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::engine::project::properties::initialize(&root).unwrap();
        std::fs::create_dir_all(root.join("items")).unwrap();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            workers.push(std::thread::spawn(move || {
                write_item_input(
                    &root,
                    json!({
                        "meta": {"type": "goal", "title": "Writing goal"},
                        "content": {
                            "kind": "writing_challenge", "scope": "all_notes",
                            "startDate": "2026-10-01", "endDate": "2026-10-31",
                            "dailyWordTarget": 500
                        }
                    }),
                    None,
                    "2026-10-06",
                )
            }));
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .any(|error| error.contains("already has a writing goal"))
        );
        temporary.close().unwrap();
    }
}
