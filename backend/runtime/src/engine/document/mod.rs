// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod content;
pub(crate) mod links;
pub(crate) mod projection;
pub(crate) mod validation;

use serde_json::Value;

/// The `ProseMirror` document inside an item's `content`. Page content either
/// is the `doc` node or wraps it under `content`; validation requires the
/// chosen node to be a `doc`.
pub(crate) fn document_root(content: &Value) -> &Value {
    content
        .get("content")
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("doc"))
        .unwrap_or(content)
}
