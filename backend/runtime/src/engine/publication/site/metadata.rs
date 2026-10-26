// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::fmt::Write;

use crate::engine::publication::{
    html::escape_html,
    render::{self, ExportError, Formatter, Kind, PreparedPage, RenderEvent},
};

pub(super) struct PageMetadata {
    pub(super) text: String,
    pub(super) contents: String,
}

pub(super) fn page_metadata(page: &PreparedPage) -> Result<PageMetadata, String> {
    render::render(page, MetadataFormatter::default()).map_err(|error| error.to_string())
}

struct Heading {
    id: String,
    level: u16,
    title: String,
}

#[derive(Default)]
struct MetadataFormatter {
    text: String,
    notes: String,
    reference: Option<String>,
    heading: Option<Heading>,
    headings: Vec<Heading>,
}

impl MetadataFormatter {
    fn append(&mut self, text: &str) {
        if let Some(reference) = &mut self.reference {
            reference.push_str(text);
        } else {
            self.text.push_str(text);
        }
    }

    fn inline(&mut self, text: &str) {
        self.append(text);
        if self.reference.is_none()
            && let Some(heading) = &mut self.heading
        {
            heading.title.push_str(text);
        }
    }
}

impl Formatter for MetadataFormatter {
    type Output = PageMetadata;

    fn emit(&mut self, event: RenderEvent<'_>) -> Result<(), ExportError> {
        match event {
            RenderEvent::Enter(node) => match &node.kind {
                Kind::Reference(_) => self.reference = Some(String::new()),
                Kind::Heading { level } => {
                    self.append(" ");
                    self.heading = node.id.as_ref().map(|id| Heading {
                        id: id.clone(),
                        level: (*level).min(6),
                        title: String::new(),
                    });
                }
                Kind::Text(text) => self.inline(text),
                Kind::HardBreak => self.inline(" "),
                Kind::CodeBlock { text, .. } => {
                    self.append(" ");
                    self.append(text);
                    self.append(" ");
                }
                Kind::Image(image) => {
                    self.append(" ");
                    self.append(&image.alt);
                    self.append(" ");
                }
                Kind::Paragraph | Kind::BoxHeading | Kind::Rule => self.append(" "),
                _ => {}
            },
            RenderEvent::Exit(node) => {
                match &node.kind {
                    Kind::Reference(_) => {
                        self.notes.push(' ');
                        self.notes.push_str(&self.reference.take().ok_or_else(|| {
                            ExportError::new("Unbalanced site metadata reference")
                        })?);
                    }
                    Kind::Heading { .. } => {
                        if let Some(mut heading) = self.heading.take() {
                            heading.title = normalize(&heading.title);
                            if !heading.title.is_empty() {
                                self.headings.push(heading);
                            }
                        }
                        self.append(" ");
                    }
                    Kind::Figure { caption } => {
                        self.append(" ");
                        self.append(caption);
                        self.append(" ");
                    }
                    Kind::Paragraph | Kind::BoxHeading => self.append(" "),
                    _ => {}
                }
            }
            RenderEvent::EnterMark(_) | RenderEvent::ExitMark(_) => {}
        }
        Ok(())
    }

    #[expect(
        clippy::expect_used,
        reason = "Formatting only strings and primitive values into String is infallible."
    )]
    fn finish(mut self) -> Result<PageMetadata, ExportError> {
        if self.reference.is_some() || self.heading.is_some() {
            return Err(ExportError::new("Unclosed site metadata events"));
        }
        self.text.push_str(&self.notes);
        let mut contents = String::new();
        if !self.headings.is_empty() {
            contents.push_str(
                "<nav class=\"publication-page-contents\" aria-label=\"On this page\"><h2>On this page</h2><ol>",
            );
            for heading in self.headings {
                write!(contents,
                    "<li class=\"publication-page-contents__level-{}\"><a href=\"#block-{}\">{}</a></li>",
                    heading.level, escape_html(&heading.id), escape_html(&heading.title)
                ).expect("writing to a String cannot fail");
            }
            contents.push_str("</ol></nav>\n");
        }
        Ok(PageMetadata {
            text: normalize(&self.text),
            contents,
        })
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use serde_json::{Value, json};

    use super::{PageMetadata, page_metadata};
    use crate::engine::publication::{
        html::render_prepared_html_document,
        render::{self, ReferenceScope, Resources, prepare_page},
    };

    fn metadata(
        document: &Value,
        fields: &HashMap<String, String>,
        snippets: &HashMap<String, Value>,
        exclusions: &HashSet<String>,
        offset: i16,
    ) -> PageMetadata {
        let page = prepare_page(
            "site-test",
            document,
            &Resources {
                links: &HashMap::new(),
                assets: &HashMap::new(),
                fields,
                snippets,
                excluded_block_ids: exclusions,
            },
            offset,
            &mut ReferenceScope::default(),
        )
        .unwrap();
        page_metadata(&page).unwrap()
    }

    #[test]
    fn contents_uses_prepared_levels_fields_and_exclusions() {
        let document = json!({"type":"doc","content":[
            {"type":"heading","attrs":{"id":"included","level":2},"content":[
                {"type":"text","text":"A <title> & "},
                {"type":"field","attrs":{"key":"edition"}}]},
            {"type":"heading","attrs":{"id":"excluded","level":2},"content":[
                {"type":"text","text":"Excluded"}]}
        ]});
        let result = metadata(
            &document,
            &HashMap::from([("edition".into(), "2026".into())]),
            &HashMap::new(),
            &HashSet::from(["excluded".into()]),
            6,
        );
        assert!(result.contents.contains("href=\"#block-included\""));
        assert!(result.contents.contains("A &lt;title&gt; &amp; 2026"));
        assert!(
            result
                .contents
                .contains("publication-page-contents__level-6")
        );
        assert_eq!(result.text, "A <title> & 2026");
        assert!(!result.contents.contains("Excluded"));
    }

    #[test]
    fn search_preserves_inline_words_and_separates_blocks() {
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[
                {"type":"text","text":"un"},
                {"type":"text","text":"broken","marks":[{"type":"strong"}]},
                {"type":"text","text":" "},
                {"type":"field","attrs":{"key":"year"}}]},
            {"type":"paragraph","attrs":{"id":"excluded"},"content":[{"type":"text","text":"Hidden"}]},
            {"type":"paragraph","content":[{"type":"text","text":"Next"},{"type":"hard_break"},{"type":"text","text":"line"}]}
        ]});
        let result = metadata(
            &document,
            &HashMap::from([("year".into(), "2026".into())]),
            &HashMap::new(),
            &HashSet::from(["excluded".into()]),
            0,
        );
        assert_eq!(result.text, "unbroken 2026 Next line");
        assert_eq!(result.contents, "");
    }

    #[test]
    fn search_includes_expanded_snippets_notes_code_and_captions() {
        let document = json!({"type":"doc","content":[
            {"type":"paragraph","content":[
                {"type":"text","text":"Read "},
                {"type":"footnote","attrs":{"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Rich note"}]}}},
                {"type":"text","text":"this sentence."}]},
            {"type":"embed","attrs":{"snippetId":"source"}},
            {"type":"paragraph","content":[
                {"type":"citation","attrs":{"source":"snippet","snippetId":"source"}},
                {"type":"citation","attrs":{"source":"snippet","snippetId":"source"}}]},
            {"type":"figure","content":[
                {"type":"blockTitle","content":[{"type":"text","text":"Code caption"}]},
                {"type":"code_block","attrs":{"language":"plain"},"content":[{"type":"text","text":"let answer = 42;"}]}]}
        ]});
        let result = metadata(
            &document,
            &HashMap::new(),
            &HashMap::from([(
                "source".into(),
                json!({"type":"paragraph","content":[{"type":"text","text":"Shared source"}]}),
            )]),
            &HashSet::new(),
            0,
        );
        assert_eq!(
            result.text,
            "Read this sentence. Shared source let answer = 42; Code caption Rich note Shared source"
        );
        assert_eq!(result.contents, "");
    }

    #[test]
    fn corpus_metadata_matches_the_same_prepared_html() {
        let document = serde_json::from_str(include_str!(
            "../../../../../test-fixtures/publication/full-export.json"
        ))
        .unwrap();
        let links = HashMap::from([
            (
                "https://example.com/?a=1&b=2".into(),
                render::Destination::Url("https://example.com/?a=1&b=2".into()),
            ),
            (
                "https://example.com/source".into(),
                render::Destination::Url("https://example.com/source".into()),
            ),
            (
                "/?note=01a0e2a0-0000-7000-8000-000000000001".into(),
                render::Destination::Url("chapter.html".into()),
            ),
            (
                "#heading-one".into(),
                render::Destination::Url("#block-heading-one".into()),
            ),
        ]);
        let page = prepare_page(
            "corpus",
            &document,
            &Resources {
                links: &links,
                assets: &HashMap::from([(
                    "/assets/test/pixel.png".into(),
                    "assets/pixel.png".into(),
                )]),
                fields: &HashMap::from([("author".into(), "Writer & editor".into())]),
                snippets: &HashMap::from([(
                    "source_one".into(),
                    json!({"type":"paragraph","content":[{"type":"text","text":"Shared source"}]}),
                )]),
                excluded_block_ids: &HashSet::from(["excluded-paragraph".into()]),
            },
            0,
            &mut ReferenceScope::default(),
        )
        .unwrap();
        let result = page_metadata(&page).unwrap();
        let html = render_prepared_html_document(&page).unwrap();
        assert_eq!(result.contents.matches("<li ").count(), 6);
        assert!(!result.text.contains("must not appear"));
        for text in [
            "Shared source",
            "Writer & editor",
            "Rich footnote source",
            "Inline citation",
            "Table caption",
            "Image caption",
            "Code caption",
            "fn main()",
            "One transparent pixel",
        ] {
            assert!(result.text.contains(text), "{text}");
        }
        for level in 1..=6 {
            assert!(result.contents.contains(&format!("__level-{level}")));
        }
        for id in [
            "heading-one",
            "heading-two",
            "heading-three",
            "heading-four",
            "heading-five",
            "heading-six",
        ] {
            assert!(result.contents.contains(&format!("href=\"#block-{id}\"")));
            assert!(html.contains(&format!("id=\"block-{id}\"")));
        }
    }
}
