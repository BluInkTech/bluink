// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::{HashMap, HashSet};

use serde_json::Value;

pub(crate) fn document(item: &Value) -> &Value {
    crate::domain::document::document_root(&item["content"])
}

/// Values for the fields the rendered document shows. Excluded blocks are not
/// rendered, so a field only they use needs no value, as in preflight.
pub(crate) fn resolved_fields(
    item: &Value,
    node: &Value,
    exclusions: &HashSet<String>,
) -> Result<HashMap<String, String>, String> {
    let mut keys = HashSet::new();
    collect_field_keys(node, exclusions, &mut keys);
    let mut fields = HashMap::new();
    for key in keys {
        let value = &item["meta"][&key];
        let text = if let Some(text) = value.as_str() {
            text.to_owned()
        } else if value.is_number() || value.is_boolean() {
            value.to_string()
        } else {
            return Err(format!("Field {key} has no scalar source value."));
        };
        fields.insert(key, text);
    }
    Ok(fields)
}

fn collect_field_keys(node: &Value, exclusions: &HashSet<String>, keys: &mut HashSet<String>) {
    if node["attrs"]["id"]
        .as_str()
        .is_some_and(|id| exclusions.contains(id))
    {
        return;
    }
    if node["type"] == "field"
        && let Some(key) = node["attrs"]["key"].as_str()
    {
        keys.insert(key.to_owned());
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            collect_field_keys(child, exclusions, keys);
        }
    }
    if let Some(fragment) = node["attrs"]["paragraph"].as_object() {
        collect_field_keys(&Value::Object(fragment.clone()), exclusions, keys);
    }
}

pub(crate) fn parse_language(value: &str) -> Option<(String, Option<String>)> {
    let mut parts = value.trim().split('-');
    let lang = parts.next()?;
    let region = parts.next();
    if !(2..=3).contains(&lang.len())
        || !lang.bytes().all(|byte| byte.is_ascii_alphabetic())
        || region.is_some_and(|part| {
            part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
        || parts.next().is_some()
    {
        return None;
    }
    Some((
        lang.to_ascii_lowercase(),
        region.map(str::to_ascii_uppercase),
    ))
}

pub(crate) fn min_heading_level(node: &Value, exclusions: &HashSet<String>) -> Option<u64> {
    if node["attrs"]["id"]
        .as_str()
        .is_some_and(|id| exclusions.contains(id))
    {
        return None;
    }
    let own = if node["type"] == "heading" {
        node["attrs"]["level"].as_u64()
    } else {
        None
    };
    node["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|child| min_heading_level(child, exclusions))
        .chain(own)
        .min()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::resolved_fields;

    #[test]
    fn fields_inside_excluded_blocks_need_no_value() {
        let item = json!({"meta": {"title": "Page", "edition": 2}});
        let document = json!({"type": "doc", "content": [
            {"type": "paragraph", "attrs": {"id": "kept"}, "content": [
                {"type": "field", "attrs": {"key": "edition"}}
            ]},
            {"type": "paragraph", "attrs": {"id": "draft"}, "content": [
                {"type": "field", "attrs": {"key": "missing"}}
            ]}
        ]});
        assert!(resolved_fields(&item, &document, &HashSet::new()).is_err());
        let fields =
            resolved_fields(&item, &document, &HashSet::from(["draft".to_owned()])).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields["edition"], "2");
    }
}
