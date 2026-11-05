// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashSet;

use serde_json::{Value, json};

use super::*;

#[derive(Default)]
struct Recording {
    events: Vec<String>,
    nodes: Vec<NodeInfo>,
    depth: usize,
    marks: Vec<Mark>,
}

impl Formatter for Recording {
    type Output = Self;

    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
        match event {
            RenderEvent::Enter(info) => {
                self.events.push(format!("+{:?}", info.kind));
                self.nodes.push(info.clone());
                self.depth += 1;
            }
            RenderEvent::Exit(info) => {
                self.events.push(format!("-{:?}", info.kind));
                self.depth -= 1;
            }
            RenderEvent::EnterMark(mark) => {
                self.events.push(format!("+mark:{mark:?}"));
                self.marks.push(mark.clone());
            }
            RenderEvent::ExitMark(mark) => {
                self.events.push(format!("-mark:{mark:?}"));
                assert_eq!(self.marks.pop().as_ref(), Some(mark));
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Self::Output, ExportError> {
        assert_eq!(self.depth, 0);
        assert_eq!(self.marks, []);
        Ok(self)
    }
}

#[derive(Default)]
struct Fixture {
    links: HashMap<String, Destination>,
    assets: HashMap<String, String>,
    fields: HashMap<String, String>,
    snippets: HashMap<String, Value>,
    exclusions: HashSet<String>,
}

impl Fixture {
    fn resources(&self) -> Resources<'_> {
        Resources {
            links: &self.links,
            assets: &self.assets,
            fields: &self.fields,
            snippets: &self.snippets,
            excluded_block_ids: &self.exclusions,
        }
    }

    fn record(&self, document: &Value, scope: &mut ReferenceScope) -> Recording {
        let prepared = prepare_page("page-one", document, &self.resources(), 0, scope).unwrap();
        render(&prepared, Recording::default()).unwrap()
    }
}

#[test]
fn row_groups_are_known_before_cells_even_when_the_span_starts_in_a_later_column() {
    let document = json!({"type":"doc","content":[{"type":"table","attrs":{"columnWidthsPercent":[50,50]},"content":[
        {"type":"table_row","content":[
            {"type":"table_header","content":[{"type":"paragraph","content":[{"type":"text","text":"First cell"}]}]},
            {"type":"table_header","attrs":{"rowspan":2},"content":[{"type":"paragraph","content":[{"type":"text","text":"Spanning cell"}]}]}]},
        {"type":"table_row","content":[{"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"Second row"}]}]}]},
        {"type":"table_row","content":[{"type":"table_cell","content":[{"type":"paragraph"}]},{"type":"table_cell","content":[{"type":"paragraph"}]}]}
    ]}]});
    let output = Fixture::default().record(&document, &mut ReferenceScope::default());
    let rows: Vec<_> = output
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            Kind::TableRow {
                index,
                repeat_header,
                joins_next_row,
            } => Some((index, repeat_header, joins_next_row)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [(0, false, true), (1, false, false), (2, false, false)]
    );
}

#[test]
fn event_order_is_exact_and_marks_close_in_reverse_order() {
    let document = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"paragraph-one"},"content":[
        {"type":"text","text":"Raised","marks":[{"type":"strong"},{"type":"superscript"}]}, {"type":"hard_break"}
    ]}]});
    let output = Fixture::default().record(&document, &mut ReferenceScope::default());
    assert_eq!(
        output.events,
        [
            "+Document",
            "+Paragraph",
            "+mark:Strong",
            "+mark:Superscript",
            "+Text(\"Raised\")",
            "-Text(\"Raised\")",
            "-mark:Superscript",
            "-mark:Strong",
            "+HardBreak",
            "-HardBreak",
            "-Paragraph",
            "-Document"
        ]
    );
}

#[test]
fn links_wrap_semantic_marks_independently_of_source_mark_order() {
    let fixture = Fixture {
        links: HashMap::from([(
            "https://example.com".into(),
            Destination::Url("https://example.com".into()),
        )]),
        ..Fixture::default()
    };
    for marks in [
        json!([{"type":"strong"},{"type":"link","attrs":{"href":"https://example.com"}}]),
        json!([{"type":"link","attrs":{"href":"https://example.com"}},{"type":"strong"}]),
    ] {
        let document = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Bold link","marks":marks}]}]});
        let result = fixture.record(&document, &mut ReferenceScope::default());
        assert_eq!(result.events[2], "+mark:Link(Url(\"https://example.com\"))");
        assert_eq!(result.events[3], "+mark:Strong");
    }
}

fn complete_corpus() -> Recording {
    let document: Value = serde_json::from_str(include_str!(
        "../../../../test-fixtures/publication/full-export.json"
    ))
    .unwrap();
    let fixture = Fixture {
        links: HashMap::from([
            (
                "https://example.com/?a=1&b=2".into(),
                Destination::Url("https://example.com/?a=1&b=2".into()),
            ),
            (
                "https://example.com/source".into(),
                Destination::Url("https://example.com/source".into()),
            ),
            (
                "/?note=01a0e2a0-0000-7000-8000-000000000001".into(),
                Destination::Anchor("note-two".into()),
            ),
            (
                "#heading-one".into(),
                Destination::Anchor("block-heading-one".into()),
            ),
        ]),
        assets: HashMap::from([("/assets/test/pixel.png".into(), "assets/pixel.png".into())]),
        fields: HashMap::from([("author".into(), "Writer & editor".into())]),
        snippets: HashMap::from([(
            "source_one".into(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Shared source","marks":[{"type":"strong"}]}]}),
        )]),
        exclusions: HashSet::from(["excluded-paragraph".into()]),
    };
    fixture.record(&document, &mut ReferenceScope::default())
}

#[test]
fn complete_corpus_prepares_all_headings_boxes_fields_and_references() {
    let output = complete_corpus();
    assert!(
        !output
            .events
            .iter()
            .any(|event| event.contains("Excluded text"))
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| event.contains("Writer & editor"))
    );
    let headings = output
        .nodes
        .iter()
        .filter_map(|node| {
            if let Kind::Heading { level } = node.kind {
                Some(level)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(headings, [1, 2, 3, 4, 5, 6]);
    let boxes = output
        .nodes
        .iter()
        .filter_map(|node| {
            if let Kind::Box(kind) = node.kind {
                Some(kind.name())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        boxes,
        [
            "note",
            "plain",
            "note",
            "tip",
            "tip",
            "important",
            "warning",
            "caution",
            "note",
            "plain"
        ]
    );
    // The footnote keeps its own number; the inline and shared citations
    // beside it share one marker, and the shared source cited again adds
    // nothing new.
    let footnotes = output
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            Kind::Footnote(footnote) => Some(footnote.number),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(footnotes, [1]);
    let markers = output
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            Kind::Citations(citations) => Some(
                citations
                    .iter()
                    .map(|citation| citation.number)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(markers, [vec![1, 2]]);
}

#[test]
fn complete_corpus_normalizes_image_dimensions_and_table_spans_and_keeps_code_verbatim() {
    let output = complete_corpus();
    let cells = output
        .nodes
        .iter()
        .filter_map(|node| {
            if let Kind::TableCell(cell) = node.kind {
                Some(cell)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cells,
        [
            Cell {
                row: 0,
                column: 0,
                colspan: 1,
                rowspan: 2,
                header: true
            },
            Cell {
                row: 0,
                column: 1,
                colspan: 2,
                rowspan: 1,
                header: true
            },
            Cell {
                row: 1,
                column: 1,
                colspan: 1,
                rowspan: 1,
                header: false
            },
            Cell {
                row: 1,
                column: 2,
                colspan: 1,
                rowspan: 1,
                header: false
            },
        ]
    );
    let image = output
        .nodes
        .iter()
        .find_map(|node| {
            if let Kind::Image(image) = &node.kind {
                Some(image)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(image.width_pt, Some(48.0));
    assert_eq!(image.height_pt, Some(36.0));
    assert!(output.nodes.iter().any(|node| matches!(&node.kind, Kind::CodeBlock { text, .. } if text.contains("println!(\"# <literal>\");"))));
}

fn citations() -> Value {
    json!({"type":"doc","content":[{"type":"paragraph","content":[
        {"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}},
        {"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}}
    ]}]})
}

fn citation_fixture() -> Fixture {
    Fixture {
        snippets: HashMap::from([(
            "source_one".into(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Reference","marks":[{"type":"emphasis"}]}]}),
        )]),
        ..Fixture::default()
    }
}

/// The numbers of each citation marker a page prepared.
fn markers(recording: &Recording) -> Vec<Vec<usize>> {
    recording
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            Kind::Citations(citations) => {
                Some(citations.iter().map(|citation| citation.number).collect())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_shared_source_keeps_its_number_and_is_listed_once_per_scope() {
    let fixture = citation_fixture();
    let mut scope = ReferenceScope::default();
    // Cited twice side by side: one marker, one number.
    let first = fixture.record(&citations(), &mut scope);
    assert_eq!(markers(&first), [vec![1]]);
    // The source's text waits for the list instead of entering the page.
    assert!(
        !first
            .nodes
            .iter()
            .any(|node| matches!(&node.kind, Kind::Text(text) if text == "Reference"))
    );
    let second = fixture.record(&citations(), &mut scope);
    assert_eq!(markers(&second), [vec![1]]);
    assert_eq!(scope.items.len(), 1);
    // A fresh scope, as a site page or a new chapter has, numbers from one.
    let fresh = fixture.record(&citations(), &mut ReferenceScope::default());
    assert_eq!(markers(&fresh), [vec![1]]);

    let list = scope.take_list("page-one", Some(("Sources", 2))).unwrap();
    let rendered = render(&list, Recording::default()).unwrap();
    assert!(matches!(rendered.nodes[0].kind, Kind::ReferenceList));
    assert!(matches!(rendered.nodes[1].kind, Kind::Heading { level: 2 }));
    assert!(
        rendered
            .nodes
            .iter()
            .any(|node| matches!(&node.kind, Kind::Text(text) if text == "Sources"))
    );
    assert_eq!(
        rendered
            .nodes
            .iter()
            .filter(
                |node| matches!(&node.kind, Kind::ReferenceItem(citation) if citation.number == 1)
            )
            .count(),
        1
    );
    assert!(
        rendered
            .nodes
            .iter()
            .any(|node| matches!(&node.kind, Kind::Text(text) if text == "Reference"))
    );
    // Taking the list empties it.
    assert!(scope.take_list("page-one", None).is_none());
}

#[test]
fn footnotes_and_citations_number_separately_and_named_scopes_namespace_targets() {
    let fixture = Fixture::default();
    let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
        {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Aside"}]}}},
        {"type":"text","text":" text "},
        {"type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{"type":"text","text":"First"}]}}},
        {"type":"text","text":" more "},
        {"type":"citation","attrs":{"source":"inline","paragraph":{"type":"paragraph","content":[{"type":"text","text":"Second"}]}}},
        {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Another"}]}}}
    ]}]});
    let mut scope = ReferenceScope::named("chapter");
    let recording = fixture.record(&document, &mut scope);
    let footnotes = recording
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            Kind::Footnote(footnote) => Some(footnote.number),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(footnotes, [1, 2]);
    // Text between citations keeps them as separate markers.
    assert_eq!(markers(&recording), [vec![1], vec![2]]);
    assert!(recording.nodes.iter().any(|node| matches!(
        &node.kind,
        Kind::Citations(citations) if citations[0].target == "ref-chapter-2"
    )));
    // A book's scope carries on numbering across its chapters.
    let next = fixture.record(&document, &mut scope);
    assert_eq!(markers(&next), [vec![3], vec![4]]);
    assert_eq!(scope.items.len(), 4);
}

#[test]
fn failed_preparation_leaves_the_scope_untouched() {
    let fixture = citation_fixture();
    let mut document = citations();
    document["content"][0]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"field","attrs":{"key":"missing"}}));
    let mut scope = ReferenceScope::default();
    let error =
        prepare_page("page-one", &document, &fixture.resources(), 0, &mut scope).unwrap_err();
    assert!(error.to_string().contains("Field missing"));
    assert_eq!((scope.footnotes, scope.citations), (0, 0));
    assert!(scope.sources.is_empty() && scope.items.is_empty());
    let result = fixture.record(&citations(), &mut scope);
    assert_eq!(markers(&result), [vec![1]]);
}

#[test]
fn exclusions_prune_resources_and_references_without_allocating_numbers() {
    let document = json!({"type":"doc","content":[
        {"type":"paragraph","attrs":{"id":"omit"},"content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"missing"}},{"type":"field","attrs":{"key":"missing"}}]},
        {"type":"paragraph","content":[{"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Included"}]}}}]}
    ]});
    let fixture = Fixture {
        exclusions: HashSet::from(["omit".into()]),
        ..Fixture::default()
    };
    let mut scope = ReferenceScope::default();
    let result = fixture.record(&document, &mut scope);
    assert!(
        result
            .nodes
            .iter()
            .any(|node| matches!(node.kind, Kind::Footnote(Footnote { number: 1 })))
    );
    assert_eq!(scope.citations, 0);
    assert!(!result.events.iter().any(|event| event.contains("missing")));
}

#[test]
fn missing_and_unsafe_resources_fail_with_page_and_nearest_block() {
    let document = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"block-one"},"content":[{"type":"text","text":"Link","marks":[{"type":"link","attrs":{"href":"key"}}]}]}]});
    for destination in [
        None,
        Some(Destination::Url("javascript:alert(1)".into())),
        Some(Destination::Anchor(String::new())),
    ] {
        let mut fixture = Fixture::default();
        if let Some(destination) = destination {
            fixture.links.insert("key".into(), destination);
        }
        let error = prepare_page(
            "page-one",
            &document,
            &fixture.resources(),
            0,
            &mut ReferenceScope::default(),
        )
        .unwrap_err();
        assert_eq!(error.page_id.as_deref(), Some("page-one"));
        assert_eq!(error.block_id.as_deref(), Some("block-one"));
    }
    let fixture = Fixture {
        links: HashMap::from([("key".into(), Destination::PlainText)]),
        ..Fixture::default()
    };
    let result = fixture.record(&document, &mut ReferenceScope::default());
    assert!(
        !result
            .events
            .iter()
            .any(|event| event.contains("mark:Link"))
    );
}

#[test]
fn snippets_are_terminal_and_keep_rich_marks_and_planned_links() {
    let document = json!({"type":"doc","content":[{"type":"embed","attrs":{"id":"embed-one","snippetId":"source_one"}}]});
    let mut fixture = Fixture {
        snippets: HashMap::from([(
            "source_one".into(),
            json!({"type":"paragraph","content":[{"type":"text","text":"Rich source","marks":[{"type":"strong"},{"type":"link","attrs":{"href":"https://example.com"}}]}]}),
        )]),
        links: HashMap::from([(
            "https://example.com".into(),
            Destination::Url("https://example.com".into()),
        )]),
        ..Fixture::default()
    };
    let result = fixture.record(&document, &mut ReferenceScope::default());
    assert!(
        result
            .events
            .iter()
            .any(|event| event.contains("mark:Strong"))
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| event.contains("mark:Link"))
    );
    fixture.snippets.insert("source_one".into(), json!({"type":"paragraph","content":[{"type":"citation","attrs":{"source":"snippet","snippetId":"source_one"}}]}));
    let error = prepare_page(
        "page-one",
        &document,
        &fixture.resources(),
        0,
        &mut ReferenceScope::default(),
    )
    .unwrap_err();
    assert!(error.message.contains("text only"));
    assert_eq!(error.block_id.as_deref(), Some("embed-one"));
}

#[test]
fn table_exclusions_leave_empty_geometry_instead_of_shifting_surviving_cells() {
    let document = json!({"type":"doc","content":[{"type":"table","attrs":{"columnWidthsPercent":[40,60]},"content":[{"type":"table_row","content":[
        {"type":"table_cell","attrs":{"id":"omit"},"content":[{"type":"paragraph","content":[{"type":"field","attrs":{"key":"missing"}}]}]},
        {"type":"table_cell","content":[{"type":"paragraph","content":[{"type":"text","text":"Survivor"}]}]}
    ]}]}]});
    let fixture = Fixture {
        exclusions: HashSet::from(["omit".into()]),
        ..Fixture::default()
    };
    let output = fixture.record(&document, &mut ReferenceScope::default());
    let cells = output
        .nodes
        .iter()
        .filter_map(|node| {
            if let Kind::TableCell(cell) = node.kind {
                Some(cell.column)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(cells, [0, 1]);
    assert!(!output.events.iter().any(|event| event.contains("missing")));
    assert!(output.events.iter().any(|event| event.contains("Survivor")));
}

#[test]
fn heading_nesting_preserves_levels_above_six_and_rejects_underflow() {
    let document =
        json!({"type":"doc","content":[{"type":"heading","attrs":{"id":"heading","level":6}}]});
    let fixture = Fixture::default();
    let prepared = prepare_page(
        "page",
        &document,
        &fixture.resources(),
        2,
        &mut ReferenceScope::default(),
    )
    .unwrap();
    let output = render(&prepared, Recording::default()).unwrap();
    assert!(
        output
            .nodes
            .iter()
            .any(|node| matches!(node.kind, Kind::Heading { level: 8 }))
    );
    assert!(
        prepare_page(
            "page",
            &document,
            &fixture.resources(),
            -6,
            &mut ReferenceScope::default()
        )
        .unwrap_err()
        .message
        .contains("below 1")
    );
}

#[test]
fn formatter_failure_stops_traversal_and_keeps_source_context() {
    struct Failing {
        enters: usize,
    }
    impl Formatter for Failing {
        type Output = ();
        fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
            if let RenderEvent::Enter(node) = event {
                self.enters += 1;
                if matches!(node.kind, Kind::Text(_)) {
                    return Err(ExportError::new("Serializer failure"));
                }
            }
            Ok(())
        }
        fn finish(self) -> Result<(), ExportError> {
            panic!("finish must not run after an event failure")
        }
    }
    let document = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"block-one"},"content":[{"type":"text","text":"First"},{"type":"text","text":"Second"}]}]});
    let fixture = Fixture::default();
    let prepared = prepare_page(
        "page-one",
        &document,
        &fixture.resources(),
        0,
        &mut ReferenceScope::default(),
    )
    .unwrap();
    let mut formatter = Failing { enters: 0 };
    let error = walk(&prepared, &mut formatter).unwrap_err();
    assert_eq!(formatter.enters, 3);
    assert_eq!(
        error.to_string(),
        "Page page-one, block block-one: Serializer failure"
    );
    assert!(render(&prepared, Failing { enters: 0 }).is_err());
}

#[test]
fn a_code_mark_renders_only_on_text() {
    let code = json!([{"type":"code"}]);
    let document = json!({"type":"doc","content":[{"type":"paragraph","content":[
        {"type":"text","text":"call()","marks":code},
        {"type":"hard_break","marks":code},
        {"type":"footnote","marks":code,"attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Note"}]}}}
    ]}]});
    let events = Fixture::default()
        .record(&document, &mut ReferenceScope::default())
        .events;
    let code_marks = events.iter().filter(|event| *event == "+mark:Code").count();
    assert_eq!(code_marks, 1, "{events:?}");
}
