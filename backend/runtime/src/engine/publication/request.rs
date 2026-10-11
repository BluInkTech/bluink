// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{OutsideLinks, PdfProfile, plan::PublicationFormat};
use crate::engine::project::item::is_item_id;
use serde::{Deserialize, Deserializer, de::Error};
const INVALID_SOURCE: &str = "Choose a saved source and output type.";
const INVALID_PAGE_SETTINGS: &str = "Choose valid page export settings.";
#[derive(Debug)]
pub(crate) enum SourceKind {
    Publication,
    Note,
    LinkedPages,
}
#[derive(Debug)]
pub(crate) struct PageSettings {
    pub(crate) theme_id: String,
    pub(crate) outside_links: OutsideLinks,
    pub(crate) profile: PdfProfile,
}
#[derive(Debug, Deserialize)]
#[serde(try_from = "ExportInput")]
#[cfg_attr(test, derive(ts_rs::TS), ts(as = "ExportInput"))]
pub(crate) struct ExportRequest {
    pub(crate) source: SourceKind,
    pub(crate) id: String,
    pub(crate) format: PublicationFormat,
    pub(crate) start: bool,
    pub(crate) page: Option<PageSettings>,
}
/// Dispatch by the nested source discriminator without losing field-specific errors.
#[derive(Debug)]
#[cfg_attr(test, derive(ts_rs::TS), ts(untagged))]
pub(crate) enum ExportInput {
    Note(PageExportInput<NoteSource, NoteFormat>),
    LinkedPages(PageExportInput<LinkedSource, LinkedFormat>),
    Publication(PublicationExportInput),
}
impl<'de> Deserialize<'de> for ExportInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Keep the required source at the boundary so the shared decoder can
        // report missing fields consistently across every RPC.
        #[derive(Deserialize)]
        struct Envelope {
            source: serde_json::Value,
            #[serde(flatten)]
            fields: serde_json::Map<String, serde_json::Value>,
        }
        let envelope = Envelope::deserialize(deserializer)?;
        let kind = envelope.source["kind"].as_str().map(str::to_owned);
        let mut fields = envelope.fields;
        fields.insert("source".to_owned(), envelope.source);
        let input = serde_json::Value::Object(fields);
        match kind.as_deref() {
            Some("note") => serde_json::from_value(input).map(Self::Note),
            Some("linked-pages") => serde_json::from_value(input).map(Self::LinkedPages),
            Some("publication") => serde_json::from_value(input).map(Self::Publication),
            _ => return Err(D::Error::custom(INVALID_SOURCE)),
        }
        .map_err(D::Error::custom)
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PageExportInput<S, F> {
    source: S,
    format: F,
    start: bool,
    theme_id: String,
    outside_links: OutsideLinks,
    profile: PdfProfile,
}
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PublicationExportInput {
    source: PublicationSource,
    format: PublicationFormat,
    start: bool,
}
macro_rules! source {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Deserialize)]
        #[serde(tag = "kind")]
        #[cfg_attr(test, derive(ts_rs::TS))]
        pub(crate) enum $name {
            #[serde(rename = $kind)]
            Source { id: String },
        }
    };
}
source!(NoteSource, "note");
source!(LinkedSource, "linked-pages");
source!(PublicationSource, "publication");
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum NoteFormat {
    Pdf,
    Html,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum LinkedFormat {
    Pdf,
    Site,
}
impl TryFrom<ExportInput> for ExportRequest {
    type Error = &'static str;
    fn try_from(input: ExportInput) -> Result<Self, Self::Error> {
        let (source, id, format, start, page) = match input {
            ExportInput::Publication(request) => {
                let PublicationSource::Source { id } = request.source;
                (
                    SourceKind::Publication,
                    id,
                    request.format,
                    request.start,
                    None,
                )
            }
            ExportInput::Note(request) => {
                let NoteSource::Source { id } = request.source;
                let format = match request.format {
                    NoteFormat::Pdf => PublicationFormat::Pdf,
                    NoteFormat::Html => PublicationFormat::Html,
                };
                (
                    SourceKind::Note,
                    id,
                    format,
                    request.start,
                    Some(PageSettings {
                        theme_id: request.theme_id,
                        outside_links: request.outside_links,
                        profile: request.profile,
                    }),
                )
            }
            ExportInput::LinkedPages(request) => {
                let LinkedSource::Source { id } = request.source;
                let format = match request.format {
                    LinkedFormat::Pdf => PublicationFormat::Pdf,
                    LinkedFormat::Site => PublicationFormat::Site,
                };
                (
                    SourceKind::LinkedPages,
                    id,
                    format,
                    request.start,
                    Some(PageSettings {
                        theme_id: request.theme_id,
                        outside_links: request.outside_links,
                        profile: request.profile,
                    }),
                )
            }
        };
        if !is_item_id(&id) {
            return Err(INVALID_SOURCE);
        }
        if page
            .as_ref()
            .is_some_and(|settings| settings.theme_id.trim().is_empty())
        {
            return Err(INVALID_PAGE_SETTINGS);
        }
        Ok(Self {
            source,
            id,
            format,
            start,
            page,
        })
    }
}
#[cfg(test)]
fn validate_export_request(request: &serde_json::Value) -> Result<(), String> {
    serde_json::from_value::<ExportRequest>(request.clone())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::validate_export_request;

    const ID: &str = "e26690c4-8a2e-569a-8d9b-212a2772127e";

    fn request(kind: &str, format: &str) -> Value {
        json!({
            "source": { "kind": kind, "id": ID },
            "format": format,
            "start": false,
            "themeId": "classic",
            "outsideLinks": "error",
            "profile": "default"
        })
    }

    #[test]
    fn accepts_only_supported_source_and_format_pairs() {
        for (kind, format) in [
            ("note", "pdf"),
            ("note", "html"),
            ("linked-pages", "pdf"),
            ("linked-pages", "site"),
            ("publication", "pdf"),
            ("publication", "html"),
            ("publication", "site"),
        ] {
            assert_eq!(validate_export_request(&request(kind, format)), Ok(()));
        }
        assert_eq!(
            validate_export_request(&json!({
                "source": { "kind": "publication", "id": ID },
                "format": "site",
                "start": true
            })),
            Ok(())
        );
        for (kind, format) in [("note", "site"), ("linked-pages", "html"), ("other", "pdf")] {
            assert!(validate_export_request(&request(kind, format)).is_err());
        }
    }

    #[test]
    fn rejects_missing_source_fields_and_invalid_page_settings() {
        for invalid in [
            json!(null),
            json!({ "source": { "kind": "note", "id": "bad" }, "format": "pdf", "start": false }),
            json!({ "source": { "kind": "note", "id": ID }, "format": "docx", "start": false }),
            json!({ "source": { "kind": "note", "id": ID }, "format": "pdf", "start": "yes" }),
        ] {
            assert!(validate_export_request(&invalid).is_err());
        }
        for (field, value) in [
            ("themeId", json!("  ")),
            ("outsideLinks", json!("ignore")),
            ("profile", json!("draft")),
        ] {
            let mut invalid = request("note", "pdf");
            invalid[field] = value;
            assert!(validate_export_request(&invalid).is_err());
        }
    }
}
