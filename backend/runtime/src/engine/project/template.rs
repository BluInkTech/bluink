// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Note templates: page-shaped starting points. A template's metadata, tags
//! and document follow the rules a page follows; these are the rules that are
//! a template's own. Its fields to fill are its own `meta.propertyKeys`, the
//! empty properties a page already shows. Notes made from it keep no link.
use serde_json::Value;

pub(crate) fn validate(item: &Value, id: &str) -> Result<(), String> {
    if item["meta"]["title"]
        .as_str()
        .is_none_or(|title| title.trim().is_empty())
    {
        return Err(format!("Template {id} needs a name."));
    }
    // Uploaded files belong to the item that owns them, so a template's copy
    // would point at the template's files. Templates are text-only for now.
    if contains_asset(&item["meta"]) || contains_asset(&item["content"]) {
        return Err(format!(
            "Template {id} can't contain images or attachments; they stay with the note that owns them."
        ));
    }
    Ok(())
}

fn contains_asset(value: &Value) -> bool {
    match value {
        Value::String(text) => text.starts_with("/assets/"),
        Value::Array(values) => values.iter().any(contains_asset),
        Value::Object(map) => map.values().any(contains_asset),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn template(meta: Value, content: Value) -> Value {
        Value::Object(serde_json::Map::from_iter([
            ("meta".to_owned(), meta),
            ("content".to_owned(), content),
        ]))
    }

    #[test]
    fn templates_need_a_name() {
        let id = "01a0e2a0-0000-7000-8000-0000000000a1";
        let doc = json!({ "type": "doc", "content": [] });
        validate(
            &template(json!({ "title": "Book" }), json!({ "content": doc })),
            id,
        )
        .unwrap();
        assert!(
            validate(
                &template(json!({ "title": " " }), json!({ "content": doc })),
                id
            )
            .is_err()
        );
    }

    #[test]
    fn templates_are_text_only() {
        let id = "01a0e2a0-0000-7000-8000-0000000000a1";
        let image = json!({ "type": "doc", "content": [{ "type": "image", "attrs": { "src": format!("/assets/{id}/cover.png") } }] });
        assert!(
            validate(
                &template(json!({ "title": "Book" }), json!({ "content": image })),
                id
            )
            .is_err()
        );
        let cover = json!({ "title": "Book", "image": format!("/assets/{id}/cover.png") });
        assert!(
            validate(
                &template(
                    cover,
                    json!({ "content": { "type": "doc", "content": [] } })
                ),
                id
            )
            .is_err()
        );
    }
}
