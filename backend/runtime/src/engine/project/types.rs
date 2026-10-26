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
            pub(crate) const VALUES: &'static [(&'static str, &'static str)] = &[$((stringify!($variant), $value)),+];
        }
    };
}
vocabulary!(ItemType { Page => "page", Workspace => "workspace", Goal => "goal", Widget => "widget", Query => "query", Setting => "setting", Publication => "publication", Task => "task", Tag => "tag", Label => "label", Property => "property", Snippet => "snippet", Template => "template" });
vocabulary!(ItemState { Active => "active", Archived => "archived" });
vocabulary!(FacetKind { Tag => "tag", Label => "label" });
vocabulary!(ChoiceColor { Neutral => "neutral", Blue => "blue", Green => "green", Amber => "amber", Purple => "purple", Rose => "rose", Cyan => "cyan" });
vocabulary!(MetadataFieldValueType { Text => "text", Url => "url", Email => "email", Tel => "tel", Number => "number", Checkbox => "checkbox", Date => "date", DateTime => "dateTime", Timestamp => "timestamp", Select => "select", MultiSelect => "multiSelect", Language => "language", Color => "color", Image => "image", Rating => "rating" });
vocabulary!(FieldSource { Registered => "registered", Inferred => "inferred", Builtin => "builtin" });
vocabulary!(ViewPresentationMode { Cards => "cards", Table => "table", Kanban => "kanban", List => "list" });
vocabulary!(CalendarZoom { Month => "month", Week => "week" });
vocabulary!(TaskStatus { Open => "open", Completed => "completed" });
vocabulary!(CalendarGranularity { Day => "day", Month => "month", Year => "year" });
vocabulary!(GoalScope { AllNotes => "all_notes", View => "view" });
vocabulary!(CoverGradient { Dawn => "dawn", Sea => "sea", Meadow => "meadow", Dusk => "dusk", Bloom => "bloom", Grove => "grove" });

/// An item's icon: a bundled symbol tinted by the item's colour, an emoji, or
/// an image uploaded to the item's own assets.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ItemIcon {
    Symbol { name: String },
    Emoji { value: String },
    Image { url: String },
}

/// An item's cover. Without one, the cover is a wash of the item's colour.
/// An image's `position` is its vertical focal point, as a percentage.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum ItemCover {
    Gradient { name: CoverGradient },
    Image { url: String, position: u8 },
}

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
    pub(crate) icon: Option<ItemIcon>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cover: Option<ItemCover>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) property_keys: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lang: Option<String>,
    #[serde(flatten)]
    pub(crate) properties: BTreeMap<String, OpaqueJson>,
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
        revision: String,
        text: String,
    },
    Complete {
        #[serde(rename = "taskId")]
        task_id: String,
        revision: String,
    },
    Reopen {
        #[serde(rename = "taskId")]
        task_id: String,
        revision: String,
    },
    Delete {
        #[serde(rename = "taskId")]
        task_id: String,
        revision: String,
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

/// How one choice of a select property looks wherever it shows. Without an
/// entry, the browser picks its colour.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ChoiceAppearance {
    pub(crate) color: ChoiceColor,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS), ts(optional_fields))]
pub(crate) struct ViewContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<crate::engine::project::query::QueryDocument>,
    /// The workspace's widgets, each with its own settings, in Home's order.
    /// Those pinned to the top bar are also shortcuts there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) widgets: Option<Vec<super::workspace::WorkspaceWidget>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) calendar: Option<ViewCalendarPresentation>,
    /// The note templates the workspace's New menu offers, in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) templates: Option<WorkspaceTemplates>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, OpaqueJson>,
}
/// References are soft: a template archived later stays listed until the
/// person removes it, and the browser skips it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct WorkspaceTemplates {
    pub(crate) offered: Vec<String>,
    /// The template the main New action uses; none creates a plain note.
    pub(crate) default: Option<String>,
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
    pub(crate) query: Option<crate::engine::project::query::QueryDocument>,
    pub(crate) start_date: String,
    pub(crate) end_date: String,
    pub(crate) daily_word_target: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_word_target: Option<u64>,
}
