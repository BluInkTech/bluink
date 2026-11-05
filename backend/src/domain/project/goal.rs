// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::project::identity::ItemId;
use serde_json::{Value, json};

pub(crate) fn prepare_writing_goal(request: &Value, view: Option<&Value>) -> Result<Value, String> {
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
        ItemId::try_from(view_id)?;
        let view = view.ok_or_else(|| expected("Writing goals need an active saved view."))?;
        if view["meta"]["type"] != "workspace" || view["meta"]["state"] != "active" {
            return Err(expected("Writing goals need an active saved view."));
        }
        let query: crate::domain::project::query::QueryDocument =
            serde_json::from_value(view["content"]["query"].clone())
                .map_err(|error| expected(&format!("The workspace query is invalid: {error}")))?;
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
    Ok(json!({"meta": {"type": "goal", "title": title}, "content": content}))
}

fn expected(message: &str) -> String {
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use crate::application::item_writes::write_item_input;
    use crate::domain::project::config::create_project_config;

    #[test]
    fn direct_concurrent_writers_cannot_create_overlapping_goals() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/pdf-project");
        let config: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("bluink.json")).unwrap()).unwrap();
        create_project_config(&root, config).unwrap();
        crate::domain::project::properties::initialize(&root).unwrap();
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
