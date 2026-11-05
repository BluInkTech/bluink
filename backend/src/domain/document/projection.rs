// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;

use serde_json::Value as JsonValue;

pub(crate) const PREVIEW_WORD_LIMIT: usize = 100;

#[derive(Debug, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub(crate) struct DocumentProjection {
    pub(crate) text: String,
    pub(crate) labels: Vec<String>,
    pub(crate) preview_image: Option<String>,
    /// Internal page links in the order each first appears in the document,
    /// the order a linked-pages export discovers them.
    pub(crate) page_links: Vec<String>,
    pub(crate) snippet_citations: Vec<String>,
    pub(crate) snippet_embeds: Vec<String>,
}

#[must_use]
pub(crate) fn bounded_preview(body: &str) -> String {
    let mut words = body.split_whitespace();
    let mut preview = words
        .by_ref()
        .take(PREVIEW_WORD_LIMIT)
        .collect::<Vec<_>>()
        .join(" ");
    if words.next().is_some() {
        preview.push('…');
    }
    preview
}

#[must_use]
pub(crate) fn project_document(document: &JsonValue) -> DocumentProjection {
    let root = document;
    let mut text = String::new();
    let mut labels = HashSet::new();
    let mut preview_image = None;
    visit_document(root, &mut text, &mut labels, &mut preview_image);
    let mut page_links = OrderedIds::default();
    let mut snippet_citations = HashSet::new();
    let mut snippet_embeds = HashSet::new();
    visit_relationships(
        root,
        &mut page_links,
        &mut snippet_citations,
        &mut snippet_embeds,
    );
    let sorted = |values: HashSet<String>| {
        let mut values: Vec<_> = values.into_iter().collect();
        values.sort();
        values
    };
    DocumentProjection {
        text: text.trim_end().to_owned(),
        labels: sorted(labels),
        preview_image,
        page_links: page_links.ids,
        snippet_citations: sorted(snippet_citations),
        snippet_embeds: sorted(snippet_embeds),
    }
}

/// IDs kept in first-occurrence order without repeating any.
#[derive(Default)]
struct OrderedIds {
    ids: Vec<String>,
    seen: HashSet<String>,
}

impl OrderedIds {
    fn insert(&mut self, id: String) {
        if self.seen.insert(id.clone()) {
            self.ids.push(id);
        }
    }
}

fn visit_relationships(
    node: &JsonValue,
    pages: &mut OrderedIds,
    citations: &mut HashSet<String>,
    embeds: &mut HashSet<String>,
) {
    if let Some(marks) = node["marks"].as_array() {
        for mark in marks {
            if mark["type"] == "link"
                && let Some(href) = mark["attrs"]["href"].as_str()
                && let Some(id) = crate::domain::publication::preflight::internal_page_id(href)
            {
                pages.insert(id);
            }
        }
    }
    if let Some(id) = node["attrs"]["snippetId"].as_str() {
        if node["type"] == "citation" && node["attrs"]["source"] == "snippet" {
            citations.insert(id.to_owned());
        }
        if node["type"] == "embed" {
            embeds.insert(id.to_owned());
        }
    }
    if let Some(children) = node["content"].as_array() {
        for child in children {
            visit_relationships(child, pages, citations, embeds);
        }
    }
    if let Some(paragraph) = node["attrs"].get("paragraph") {
        visit_relationships(paragraph, pages, citations, embeds);
    }
}

fn string_array(value: Option<&JsonValue>) -> Vec<String> {
    value
        .and_then(JsonValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Text nodes split wherever marks change, inside a word as often as between
/// words, so only block boundaries, line breaks and notes separate words.
fn separate(text: &mut String) {
    if text
        .chars()
        .next_back()
        .is_some_and(|last| !last.is_whitespace())
    {
        text.push(' ');
    }
}

fn visit_document(
    node: &JsonValue,
    text: &mut String,
    labels: &mut HashSet<String>,
    preview_image: &mut Option<String>,
) {
    let Some(object) = node.as_object() else {
        return;
    };
    let node_type = object.get("type").and_then(JsonValue::as_str);
    if node_type == Some("text")
        && let Some(value) = object.get("text").and_then(JsonValue::as_str)
    {
        text.push_str(value);
    }
    if node_type == Some("hard_break") {
        separate(text);
    }
    if let Some(attrs) = object.get("attrs").and_then(JsonValue::as_object) {
        if node_type == Some("paragraph") {
            for id in string_array(attrs.get("labels")) {
                labels.insert(id);
            }
        }
        if node_type == Some("image")
            && preview_image.is_none()
            && let Some(source) = attrs.get("src").and_then(JsonValue::as_str)
            && !source.is_empty()
        {
            *preview_image = Some(source.to_owned());
        }
        if node_type == Some("footnote")
            && let Some(fragment) = attrs.get("paragraph")
        {
            separate(text);
            collect_local_text(fragment, text);
            separate(text);
        }
    }
    // Only blocks have content; inline nodes are text or atoms.
    if let Some(children) = object.get("content").and_then(JsonValue::as_array) {
        separate(text);
        for child in children {
            visit_document(child, text, labels, preview_image);
        }
        separate(text);
    }
}

fn collect_local_text(node: &JsonValue, text: &mut String) {
    match node.get("type").and_then(JsonValue::as_str) {
        Some("text") => {
            if let Some(value) = node.get("text").and_then(JsonValue::as_str) {
                text.push_str(value);
            }
        }
        Some("hard_break") => separate(text),
        _ => {}
    }
    if let Some(children) = node.get("content").and_then(JsonValue::as_array) {
        separate(text);
        for child in children {
            collect_local_text(child, text);
        }
        separate(text);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{bounded_preview, project_document};

    #[test]
    fn projects_text_labels_and_first_image() {
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":"block-one","labels":["chapter"]},"content":[
                {"type":"text","text":"Visible text"},
                {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"local note"}]}}},
                {"type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{"type":"text","text":"private source"}]}}}
            ]},
            {"type":"image","attrs":{"src":"/assets/page/first.png"}},
            {"type":"image","attrs":{"src":"/assets/page/second.png"}},
            {"type":"embed","attrs":{"snippetId":"claim"}}
        ]});
        let projected = project_document(&document);
        assert_eq!(projected.text, "Visible text local note");
        assert_eq!(
            projected.preview_image.as_deref(),
            Some("/assets/page/first.png")
        );
        let labels: std::collections::HashSet<_> = projected.labels.into_iter().collect();
        assert_eq!(labels, ["chapter".to_owned()].into());
    }

    #[test]
    fn words_split_by_marks_stay_whole_and_blocks_stay_apart() {
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[
                {"type":"text","text":"un"},
                {"type":"text","text":"believ","marks":[{"type":"bold"}]},
                {"type":"text","text":"able "},
                {"type":"text","text":"Smith","marks":[{"type":"link","attrs":{"href":"https://example.com"}}]},
                {"type":"text","text":"'s line"},
                {"type":"hard_break"},
                {"type":"text","text":"next"}
            ]},
            {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Heading"}]},
            {"type":"list","content":[{"type":"listItem","content":[
                {"type":"paragraph","content":[{"type":"text","text":"item"}]}
            ]}]}
        ]});
        assert_eq!(
            project_document(&document).text,
            "unbelievable Smith's line next Heading item"
        );
    }

    #[test]
    fn projects_typed_relationships_from_the_document_and_local_reference_content() {
        let page = "01a0e2a0-0000-7000-8000-000000000001";
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"Page","marks":[{"type":"link","attrs":{"href":format!("/?note={page}")}}]},
            {"type":"citation","attrs":{"source":"snippet","snippetId":"shared"}},
            {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Same page","marks":[{"type":"link","attrs":{"href":format!("/?note={page}")}}]}]}}}
        ]},{"type":"embed","attrs":{"snippetId":"shared"}}]});
        let projection = project_document(&document);
        assert_eq!(projection.page_links, vec![page]);
        assert_eq!(projection.snippet_citations, vec!["shared"]);
        assert_eq!(projection.snippet_embeds, vec!["shared"]);
    }

    #[test]
    fn keeps_page_links_in_the_order_each_first_appears() {
        let [a, b, c] = [1, 2, 3].map(|n| format!("01a0e2a0-0000-7000-8000-00000000000{n}"));
        let link = |id: &str| json!({"type":"text","text":"x","marks":[{"type":"link","attrs":{"href":format!("/?note={id}")}}]});
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[link(&c), link(&a), link(&c)]},
            {"type":"paragraph","content":[{"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[link(&b), link(&a)]}}}]}
        ]});
        assert_eq!(project_document(&document).page_links, vec![c, a, b]);
    }

    #[test]
    fn preview_is_bounded_by_words() {
        let body = std::iter::repeat_n("word", 101)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(bounded_preview(&body).split_whitespace().count(), 100);
        assert!(bounded_preview(&body).ends_with('…'));
    }
}
