// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::properties::is_snake_case;
use serde_json::Value;

#[cfg(test)]
pub(crate) fn initialize(
    root: &std::path::Path,
) -> Result<(), crate::application::error::MutationError> {
    super::properties::initialize(root)
}

pub(crate) fn validate_content(kind: &str, content: &Value) -> Result<(), String> {
    let key = content["key"].as_str().ok_or("Facet has no key")?;
    validate_facet_key(kind, key)
}

pub(crate) fn validate_facet_key(kind: &str, key: &str) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::validate_facet_key;

    #[test]
    fn facet_keys_follow_their_kind_without_project_state() {
        for (kind, key, valid) in [
            ("tag", "births/1900", true),
            ("label", "births/1900", false),
            ("label", "needs_review", true),
            ("tag", "1900/births", false),
            ("property", "needs_review", false),
        ] {
            assert_eq!(
                validate_facet_key(kind, key).is_ok(),
                valid,
                "{kind}: {key}"
            );
        }
    }
}
