// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::error::Error;
use std::fmt;

use serde_json::{Map, Value};

/// A box is a frame; its kind says what it holds. A quote is someone's words,
/// and the five intents follow the familiar note, tip, important, warning and
/// caution alerts.
pub(crate) const BOX_KINDS: &[&str] = &[
    "plain",
    "quote",
    "note",
    "tip",
    "important",
    "warning",
    "caution",
];
const BLOCK_ATTRS: &[&str] = &["id"];
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DocumentError {
    pub(crate) path: String,
    pub(crate) message: String,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl Error for DocumentError {}

fn invalid(path: &str, message: impl Into<String>) -> DocumentError {
    DocumentError {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// Validate the persisted `ProseMirror` JSON without loading Node or N-API.
/// The frontend schema remains the editing schema; shared fixtures check parity.
pub(crate) fn validate_document(value: &Value) -> Result<(), DocumentError> {
    if value.get("type").and_then(Value::as_str) != Some("doc") {
        return Err(invalid("$", "Expected a document"));
    }
    validate_node(value, "$", 0)
}

/// Persisted page documents have stable identities for every addressable block.
/// Reference paragraphs inside inline attributes are atomic payloads, not blocks.
pub(crate) fn validate_canonical_document(value: &Value) -> Result<(), DocumentError> {
    validate_document(value)?;
    validate_block_ids(value, "$", &mut std::collections::HashSet::new())
}

fn validate_block_ids(
    value: &Value,
    path: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), DocumentError> {
    if value["type"]
        .as_str()
        .is_some_and(super::content::is_addressable_block)
    {
        let id = value["attrs"]["id"]
            .as_str()
            .filter(|id| crate::domain::project::identity::is_item_id(id))
            .ok_or_else(|| invalid(path, "Canonical block UUID is missing or invalid"))?;
        if !seen.insert(id.to_ascii_lowercase()) {
            return Err(invalid(
                path,
                "Canonical block identities must be unique within the document",
            ));
        }
    }
    for (index, child) in value["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        validate_block_ids(child, &format!("{path}.content[{index}]"), seen)?;
    }
    Ok(())
}

fn validate_node(value: &Value, path: &str, depth: usize) -> Result<(), DocumentError> {
    if depth > 256 {
        return Err(invalid(path, "Document nesting is too deep"));
    }
    let node = value
        .as_object()
        .ok_or_else(|| invalid(path, "Invalid document node"))?;
    let kind = node
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(path, "Invalid document node type"))?;
    let allowed = node_attrs(kind)
        .ok_or_else(|| invalid(path, format!("Unsupported document node: {kind}")))?;
    let attrs = attributes(node, allowed, kind, path)?;
    validate_node_values(kind, attrs, node, path, depth)?;
    validate_marks(node.get("marks"), path)?;
    if kind != "text"
        && node
            .get("marks")
            .and_then(Value::as_array)
            .is_some_and(|marks| {
                marks
                    .iter()
                    .any(|mark| matches!(mark["type"].as_str(), Some("superscript" | "subscript")))
            })
    {
        return Err(invalid(
            path,
            "Superscript and subscript apply to text only",
        ));
    }
    if kind == "text" && node.get("content").is_some() {
        return Err(invalid(path, "Text nodes cannot contain child nodes"));
    }
    let empty = Vec::new();
    let children = match node.get("content") {
        Some(value) => value
            .as_array()
            .ok_or_else(|| invalid(path, format!("{kind} content must be an array")))?,
        None => &empty,
    };
    for (index, child) in children.iter().enumerate() {
        let child_path = format!("{path}.content[{index}]");
        validate_node(child, &child_path, depth + 1)?;
        if matches!(kind, "code_block" | "blockTitle" | "heading" | "boxHeading") {
            let child_marks = child
                .get("marks")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            if child_marks.iter().any(|mark| {
                matches!(kind, "code_block" | "blockTitle")
                    || !matches!(mark["type"].as_str(), Some("superscript" | "subscript"))
            }) {
                return Err(invalid(
                    &child_path,
                    format!("Marks are not allowed in {kind}"),
                ));
            }
        }
    }
    validate_children(kind, children, path)?;
    // Only a quote names who said it.
    if kind == "box"
        && attr(attrs, "kind").and_then(Value::as_str) != Some("quote")
        && children.last().and_then(|child| child.get("type"))
            == Some(&Value::from("boxAttribution"))
    {
        return Err(invalid(path, "Only a quote box has an attribution"));
    }
    if kind == "table" {
        validate_table(attrs, children, path)?;
    }
    if kind == "doc" && path != "$" {
        return Err(invalid(path, "A document cannot be nested here"));
    }
    Ok(())
}

fn node_attrs(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "doc" | "text" | "hard_break" | "blockTitle" => Some(&[]),
        "paragraph" => Some(&["id", "labels"]),
        "horizontal_rule" | "boxHeading" | "boxAttribution" | "figure" => Some(BLOCK_ATTRS),
        "code_block" => Some(&["id", "language"]),
        "heading" => Some(&["id", "level"]),
        "list" => Some(&["id", "kind", "order"]),
        "listItem" | "table_row" => Some(&["id"]),
        "table" => Some(&["id", "columnWidthsPercent"]),
        "image" => Some(&["id", "src", "alt", "width", "height"]),
        "box" => Some(&["id", "kind"]),
        "field" => Some(&["key"]),
        "footnote" => Some(&["paragraph"]),
        "citation" => Some(&["source", "paragraph", "snippetId"]),
        "embed" => Some(&["id", "snippetId"]),
        "table_cell" | "table_header" => Some(&["colspan", "rowspan", "id"]),
        _ => None,
    }
}

fn attributes<'a>(
    node: &'a Map<String, Value>,
    allowed: &[&str],
    kind: &str,
    path: &str,
) -> Result<Option<&'a Map<String, Value>>, DocumentError> {
    let Some(value) = node.get("attrs") else {
        return Ok(None);
    };
    let attrs = value
        .as_object()
        .ok_or_else(|| invalid(path, format!("{kind} attributes must be an object")))?;
    for key in attrs.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(
                path,
                format!("Unsupported {kind} attribute: {key}"),
            ));
        }
    }
    Ok(Some(attrs))
}

fn attr<'a>(attrs: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    attrs.and_then(|map| map.get(key))
}

const MAX_BLOCK_LABELS: usize = 32;
const MAX_LABEL_KEY_BYTES: usize = 128;

/// The same grammar label items declare, so every applied label can be named,
/// selected for publication, and declared later.
fn label_keys(value: &Value) -> bool {
    value.as_array().is_some_and(|values| {
        let mut seen = std::collections::BTreeSet::new();
        values.len() <= MAX_BLOCK_LABELS
            && values.iter().all(|item| {
                item.as_str().is_some_and(|key| {
                    key.len() <= MAX_LABEL_KEY_BYTES
                        && crate::domain::project::properties::is_snake_case(key)
                        && seen.insert(key)
                })
            })
    })
}

fn integer_between(value: &Value, low: i64, high: i64) -> bool {
    value
        .as_i64()
        .is_some_and(|number| (low..=high).contains(&number))
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the complete format or operation dispatch together for contract review."
)]
fn validate_node_values(
    kind: &str,
    attrs: Option<&Map<String, Value>>,
    node: &Map<String, Value>,
    path: &str,
    _depth: usize,
) -> Result<(), DocumentError> {
    if kind == "text"
        && node
            .get("text")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid(path, "Text nodes must contain nonempty text"));
    }
    if let Some(value) = attr(attrs, "labels")
        && !label_keys(value)
    {
        return Err(invalid(
            path,
            "Block labels must be distinct lowercase snake-case keys",
        ));
    }
    match kind {
        "heading" if attr(attrs, "level").is_some_and(|v| !integer_between(v, 1, 6)) => {
            return Err(invalid(path, "Heading level must be between 1 and 6"));
        }
        "list" => {
            let list_kind = attr(attrs, "kind").map_or(Some("bullet"), Value::as_str);
            if !matches!(list_kind, Some("bullet" | "ordered")) {
                return Err(invalid(path, "Unsupported list kind"));
            }
            if attr(attrs, "order")
                .is_some_and(|v| !v.is_null() && !integer_between(v, 1, MAX_SAFE_INTEGER))
            {
                return Err(invalid(path, "List order must be a positive integer"));
            }
        }
        "box"
            if !attr(attrs, "kind")
                .map_or(Some("plain"), Value::as_str)
                .is_some_and(|value| BOX_KINDS.contains(&value)) =>
        {
            return Err(invalid(path, "Unsupported box kind"));
        }
        "image" => {
            if attr(attrs, "src").is_some_and(|v| !v.is_string())
                || attr(attrs, "alt").is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(invalid(path, "Invalid image attributes"));
            }
            for key in ["width", "height"] {
                if attr(attrs, key).is_some_and(|v| !v.is_null() && !integer_between(v, 1, 100_000))
                {
                    return Err(invalid(path, format!("Invalid image {key}")));
                }
            }
        }
        "embed" => {
            validate_snippet_key(attr(attrs, "snippetId"), path)?;
        }
        "citation" => match attr(attrs, "source").and_then(Value::as_str) {
            Some("snippet") => {
                validate_snippet_key(attr(attrs, "snippetId"), path)?;
                if attr(attrs, "paragraph").is_some_and(|v| !v.is_null()) {
                    return Err(invalid(
                        path,
                        "Linked citations cannot contain a local paragraph",
                    ));
                }
            }
            Some("inline") => {
                if attr(attrs, "snippetId").is_some_and(|v| !v.is_null()) {
                    return Err(invalid(path, "Inline citations cannot reference a snippet"));
                }
                validate_reference(attr(attrs, "paragraph"), path)?;
            }
            _ => return Err(invalid(path, "Unsupported citation source")),
        },
        "footnote" => validate_reference(attr(attrs, "paragraph"), path)?,
        "field" => {
            if attr(attrs, "key").is_some_and(|v| !v.is_string()) {
                return Err(invalid(path, "Invalid field attributes"));
            }
        }
        "code_block" if attr(attrs, "language").is_some_and(|v| !v.is_string()) => {
            return Err(invalid(path, "Code language must be text"));
        }
        "table_cell" | "table_header" => {
            for key in ["colspan", "rowspan"] {
                if attr(attrs, key).is_some_and(|v| !integer_between(v, 1, 1_000)) {
                    return Err(invalid(path, format!("Invalid table {key}")));
                }
            }
        }
        _ => {}
    }
    if kind == "blockTitle"
        && node
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children.iter().any(|child| {
                    child["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(['\r', '\n']))
                })
            })
    {
        return Err(invalid(path, "Block titles must be single-line text"));
    }
    Ok(())
}

/// The width vector defines logical columns; every row must fill that same grid.
/// Row-end occupancy avoids allocating a second complete cell matrix.
fn validate_table(
    attrs: Option<&Map<String, Value>>,
    rows: &[Value],
    path: &str,
) -> Result<(), DocumentError> {
    let widths = attr(attrs, "columnWidthsPercent")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(path, "Table column percentages are required"))?;
    if widths.is_empty() || widths.len() > 1_000 {
        return Err(invalid(
            path,
            "Table must have between 1 and 1000 logical columns",
        ));
    }
    let mut total = 0.0;
    for width in widths {
        let width = width
            .as_f64()
            .filter(|width| width.is_finite() && *width > 0.0 && *width <= 100.0)
            .ok_or_else(|| invalid(path, "Column percentages must be positive finite numbers"))?;
        total += width;
    }
    if (total - 100.0).abs() > 0.000_001 {
        return Err(invalid(path, "Column percentages must total 100"));
    }
    let mut occupied_until = vec![0; widths.len()];
    for (row_index, row) in rows.iter().enumerate() {
        let mut column = 0;
        for cell in row["content"].as_array().into_iter().flatten() {
            while column < widths.len() && occupied_until[column] > row_index {
                column += 1;
            }
            let colspan = usize::try_from(cell["attrs"]["colspan"].as_u64().unwrap_or(1))
                .map_err(|_| invalid(path, "Table column span exceeds the supported range"))?;
            let rowspan = usize::try_from(cell["attrs"]["rowspan"].as_u64().unwrap_or(1))
                .map_err(|_| invalid(path, "Table row span exceeds the supported range"))?;
            let end = column + colspan;
            if end > widths.len() || row_index + rowspan > rows.len() {
                return Err(invalid(path, "Table span extends beyond its logical grid"));
            }
            if occupied_until[column..end]
                .iter()
                .any(|end| *end > row_index)
            {
                return Err(invalid(path, "Table cells overlap"));
            }
            occupied_until[column..end].fill(row_index + rowspan);
            column = end;
        }
        if occupied_until.iter().any(|end| *end <= row_index) {
            return Err(invalid(path, "Table row does not fill its logical grid"));
        }
    }
    Ok(())
}

fn validate_snippet_key(value: Option<&Value>, path: &str) -> Result<(), DocumentError> {
    let key = value
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(path, "Snippet key is required"))?;
    crate::domain::project::snippets::validate_snippet_id(key).map_err(|error| invalid(path, error))
}

fn validate_reference(value: Option<&Value>, path: &str) -> Result<(), DocumentError> {
    let paragraph = value.ok_or_else(|| invalid(path, "Reference paragraph is required"))?;
    crate::domain::project::snippets::validate_reference_paragraph(paragraph, false)
        .map_err(|error| invalid(path, error))
}

fn validate_marks(marks: Option<&Value>, path: &str) -> Result<(), DocumentError> {
    let Some(marks) = marks else {
        return Ok(());
    };
    let marks = marks
        .as_array()
        .ok_or_else(|| invalid(path, "Marks must be an array"))?;
    let mut types = Vec::new();
    for (index, mark) in marks.iter().enumerate() {
        let mark_path = format!("{path}.marks[{index}]");
        let mark = mark
            .as_object()
            .ok_or_else(|| invalid(&mark_path, "Invalid document mark"))?;
        let kind = mark
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(&mark_path, "Invalid mark type"))?;
        let allowed: &[&str] = match kind {
            "strong" | "emphasis" | "underline" | "strike" | "code" | "superscript"
            | "subscript" => &[],
            "link" => &["href"],
            _ => return Err(invalid(&mark_path, format!("Unsupported mark: {kind}"))),
        };
        let attrs = attributes(mark, allowed, kind, &mark_path)?;
        if kind == "link" && attr(attrs, "href").is_some_and(|v| !v.is_string()) {
            return Err(invalid(&mark_path, "Link destination must be text"));
        }
        if types.contains(&kind.to_owned())
            || (kind == "code" && !types.is_empty())
            || (!types.is_empty() && types.contains(&"code".to_owned()))
            || (["strong", "emphasis", "underline"].contains(&kind)
                && types
                    .iter()
                    .any(|other| ["strong", "emphasis", "underline"].contains(&other.as_str())))
            || (["superscript", "subscript"].contains(&kind)
                && types
                    .iter()
                    .any(|other| ["superscript", "subscript"].contains(&other.as_str())))
        {
            return Err(invalid(&mark_path, "Invalid collection of marks"));
        }
        types.push(kind.to_owned());
    }
    Ok(())
}

fn validate_children(kind: &str, children: &[Value], path: &str) -> Result<(), DocumentError> {
    let types: Vec<Option<&str>> = children
        .iter()
        .map(|child| child.get("type").and_then(Value::as_str))
        .collect();
    let all = |predicate: fn(&str) -> bool| types.iter().all(|kind| kind.is_some_and(predicate));
    let valid = match kind {
        "doc" => !types.is_empty() && all(is_block),
        "table_cell" | "table_header" => !types.is_empty() && all(|kind| kind == "paragraph"),
        "paragraph" | "heading" | "boxHeading" | "boxAttribution" => all(is_inline),
        "list" => !types.is_empty() && all(|kind| kind == "listItem"),
        "listItem" => types.len() == 1 && types[0] == Some("paragraph"),
        "code_block" | "blockTitle" => all(|kind| kind == "text"),
        "figure" => {
            types.len() == 2
                && types[0] == Some("blockTitle")
                && matches!(types[1], Some("image" | "table" | "code_block"))
        }
        "box" => {
            // An optional heading, the body, then an optional attribution.
            let after_heading = usize::from(types.first() == Some(&Some("boxHeading")));
            let before_attribution =
                types.len() - usize::from(types.last() == Some(&Some("boxAttribution")));
            before_attribution > after_heading
                && types[after_heading..before_attribution]
                    .iter()
                    .all(|kind| matches!(kind, Some("paragraph" | "list")))
        }
        "table" => !types.is_empty() && all(|kind| kind == "table_row"),
        "table_row" => all(|kind| matches!(kind, "table_cell" | "table_header")),
        _ => types.is_empty(),
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(path, format!("Invalid content for {kind}")))
    }
}

fn is_block(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "embed"
            | "horizontal_rule"
            | "code_block"
            | "heading"
            | "list"
            | "image"
            | "box"
            | "table"
            | "figure"
    )
}

fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "footnote" | "citation" | "hard_break" | "field"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::validate_document;

    #[test]
    fn canonical_documents_require_unique_case_independent_block_ids() {
        let id = "ABCDEF01-2345-7000-ABCD-EF0123456789";
        let mut document = serde_json::json!({"type":"doc","content":[{"type":"paragraph"}]});
        assert!(super::validate_canonical_document(&document).is_err());
        document["content"][0]["attrs"] = serde_json::json!({"id":id});
        assert!(super::validate_canonical_document(&document).is_ok());
        document["content"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type":"paragraph","attrs":{"id":id.to_ascii_lowercase()}}));
        assert!(
            super::validate_canonical_document(&document)
                .unwrap_err()
                .message
                .contains("unique")
        );
    }

    #[test]
    fn paragraph_labels_use_the_label_key_grammar() {
        let labelled = |labels: Value| {
            validate_document(&json!({"type":"doc","content":[
                {"type":"paragraph","attrs":{"labels":labels},"content":[{"type":"text","text":"Claim"}]}
            ]}))
        };
        assert!(labelled(json!(["needs_review", "claim2"])).is_ok());
        for labels in [
            json!(["Needs Review"]),
            json!([""]),
            json!(["claim", "claim"]),
            json!([42]),
            json!(["x".repeat(129)]),
            Value::Array((0..33).map(|n| json!(format!("label_{n}"))).collect()),
        ] {
            assert!(labelled(labels.clone()).is_err(), "{labels}");
        }
    }

    #[test]
    fn matches_frontend_prosemirror_contract_cases() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../test-fixtures/document-contract/cases.json"
        ))
        .unwrap();
        for case in cases {
            let valid = case["valid"].as_bool().unwrap();
            let result = validate_document(&case["document"]);
            assert_eq!(result.is_ok(), valid, "{}: {result:?}", case["name"]);
        }
    }

    #[test]
    fn validates_every_page_in_portable_export_fixture() {
        for id in 1..=4 {
            let id = format!("01a0e2a0-0000-7000-8000-{id:012}");
            let path = crate::domain::project::identity::ItemId::try_from(id.as_str())
                .unwrap()
                .path(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("test-fixtures/pdf-project"),
                );
            let item: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            if item["meta"]["type"] == "page" {
                let document = &item["content"]["document"];
                assert!(
                    validate_document(document).is_ok(),
                    "page {id}: {:?}",
                    validate_document(document)
                );
            }
        }
    }

    #[test]
    fn identifies_the_invalid_node_path() {
        let document =
            json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"widget"}]}]});
        let error = validate_document(&document).unwrap_err();
        assert_eq!(error.path, "$.content[0].content[0]");
    }
}
