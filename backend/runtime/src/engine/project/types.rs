// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed portable values shared by canonical validation and browser DTOs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// JSON owned and narrowed by an editor or feature, rather than the transport.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
#[cfg_attr(test, derive(ts_rs::TS), ts(type = "unknown"))]
pub(crate) struct OpaqueJson(pub(crate) Value);

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
        #[cfg_attr(test, derive(ts_rs::TS))]
        pub(crate) enum $name { $(#[serde(rename = $value)] $variant),+ }
        #[allow(dead_code, reason = "Wire vocabularies also supply generated browser constants.")]
        impl $name {
            pub(crate) const fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            #[cfg(test)]
            pub(crate) const VALUES: &'static [(&'static str, &'static str)] = &[$((stringify!($variant), $value)),+];
        }
    };
}
vocabulary!(ItemType { Page => "page", Workspace => "workspace", Goal => "goal", Widget => "widget", Query => "query", Setting => "setting", Publication => "publication" });
vocabulary!(ItemState { Active => "active", Archived => "archived" });
vocabulary!(FacetKind { Tag => "tag", Label => "label" });
vocabulary!(MetadataFieldValueType { Text => "text", Url => "url", Email => "email", Tel => "tel", Number => "number", Checkbox => "checkbox", Date => "date", DateTime => "dateTime", Timestamp => "timestamp", Select => "select", MultiSelect => "multiSelect", Language => "language", Color => "color", Image => "image", Rating => "rating" });
vocabulary!(FieldSource { Registered => "registered", Inferred => "inferred", Builtin => "builtin" });
vocabulary!(ViewPresentationMode { Cards => "cards", Table => "table", Kanban => "kanban", Dashboard => "dashboard" });
vocabulary!(CalendarZoom { Month => "month", Week => "week" });
vocabulary!(CardFieldLayout { Inline => "inline", Stacked => "stacked", Body => "body" });
vocabulary!(ImageFit { Cover => "cover", Fit => "fit" });
vocabulary!(TaskStatus { Open => "open", Completed => "completed" });
vocabulary!(CalendarGranularity { Day => "day", Month => "month", Year => "year" });
vocabulary!(GoalScope { AllNotes => "all_notes", View => "view" });

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<ItemState>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) item_type: Option<ItemType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) property_keys: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lang: Option<String>,
    #[serde(flatten)]
    pub(crate) properties: BTreeMap<String, OpaqueJson>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct NoteTask {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) completed_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum TaskChange {
    Create {
        text: String,
    },
    Edit {
        #[serde(rename = "taskId")]
        task_id: String,
        text: String,
    },
    Complete {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Reopen {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Delete {
        #[serde(rename = "taskId")]
        task_id: String,
    },
}

vocabulary!(ParagraphKind { Paragraph => "paragraph" });
vocabulary!(TextKind { Text => "text" });
vocabulary!(ReferenceMarkKind { Strong => "strong", Emphasis => "emphasis", Underline => "underline", Strike => "strike", Code => "code", Link => "link", Superscript => "superscript", Subscript => "subscript" });

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceParagraph {
    #[serde(rename = "type")]
    pub(crate) kind: ParagraphKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<Vec<ReferenceText>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceText {
    #[serde(rename = "type")]
    pub(crate) kind: TextKind,
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) marks: Option<Vec<ReferenceMark>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceMark {
    #[serde(rename = "type")]
    pub(crate) kind: ReferenceMarkKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) attrs: Option<LinkAttributes>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkAttributes {
    pub(crate) href: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mode: Option<ViewPresentationMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cards: Option<ViewCardPresentation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) table: Option<ViewTablePresentation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kanban: Option<ViewKanbanPresentation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) calendar: Option<ViewCalendarPresentation>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, OpaqueJson>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewCalendarPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) initial_zoom: Option<CalendarZoom>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewTableColumn {
    pub(crate) field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<f64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewCardField {
    pub(crate) field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_lines: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) layout: Option<CardFieldLayout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_items: Option<u8>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewKanbanColumn {
    pub(crate) id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hidden: Option<bool>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewTablePresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) columns: Option<Vec<ViewTableColumn>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewCardPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fields: Option<Vec<ViewCardField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) show_media: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) media_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image_fit: Option<ImageFit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title_lines: Option<u8>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewKanbanPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) columns: Option<Vec<ViewKanbanColumn>>,
}

vocabulary!(WritingGoalKind { WritingChallenge => "writing_challenge" });
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct WritingGoalContent {
    pub(crate) kind: WritingGoalKind,
    pub(crate) scope: GoalScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) view_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    pub(crate) start_date: String,
    pub(crate) end_date: String,
    pub(crate) daily_word_target: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_word_target: Option<u64>,
}
