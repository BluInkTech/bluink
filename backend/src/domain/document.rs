// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod content;
pub(crate) mod links;
pub(crate) mod projection;
pub(crate) mod validation;

use serde_json::Value;

/// The `ProseMirror` document a page or template keeps under `content.document`.
/// A missing document reads as null, which validation rejects.
pub(crate) fn document_root(content: &Value) -> &Value {
    content.get("document").unwrap_or(&Value::Null)
}
