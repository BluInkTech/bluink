// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Every workspace, including the initial All notes, uses one canonical content shape.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::HashSet;

/// One of a workspace's widgets, with its own settings. It shows on Home at
/// its placement, and any widget can also be pinned to the top bar as a
/// shortcut. Which combinations are offered is the browser's concern.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct WorkspaceWidget {
    pub(crate) id: String,
    /// Replaces the type's name as the widget's heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) title: Option<String>,
    /// Where it sits on Home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) home: Option<HomePlacement>,
    /// Pinned to the top bar as a shortcut to the widget.
    #[serde(default, rename = "topBar", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) top_bar: Option<bool>,
    pub(crate) config: WidgetConfig,
}

/// A widget's card on Home, in the order of the workspace's widgets.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HomePlacement {
    pub(crate) span: WidgetSpan,
    /// The card's height; without one, the type's default height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) height: Option<WidgetHeight>,
    /// Kept off Home while keeping its place, for a widget reached from the top bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) hidden: Option<bool>,
}

/// How tall a widget's card is. Content that does not fit scrolls inside it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum WidgetHeight {
    Compact,
    Regular,
    Tall,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "u8", into = "u8")]
#[cfg_attr(test, derive(ts_rs::TS), ts(type = "4 | 6 | 8 | 12"))]
pub(crate) enum WidgetSpan {
    Third,
    Half,
    TwoThirds,
    Full,
}
impl TryFrom<u8> for WidgetSpan {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            4 => Ok(Self::Third),
            6 => Ok(Self::Half),
            8 => Ok(Self::TwoThirds),
            12 => Ok(Self::Full),
            _ => Err("Choose widget width 4, 6, 8 or 12.".into()),
        }
    }
}
impl From<WidgetSpan> for u8 {
    fn from(value: WidgetSpan) -> Self {
        match value {
            WidgetSpan::Third => 4,
            WidgetSpan::Half => 6,
            WidgetSpan::TwoThirds => 8,
            WidgetSpan::Full => 12,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum WidgetConfig {
    ContinueWriting {
        #[serde(rename = "noteId")]
        note_id: Option<String>,
    },
    RecentNotes {
        count: u8,
        presentation: NotePresentation,
    },
    PinnedNotes {
        count: u8,
        presentation: NotePresentation,
    },
    WorkspaceShortcuts {
        #[serde(rename = "workspaceIds")]
        workspace_ids: Vec<String>,
    },
    /// The workspace's notes, narrowed by `query`, as cards, a table, a board or a list.
    Layout {
        query: super::query::QueryDocument,
        mode: super::types::ViewPresentationMode,
        /// The card design, owned and checked by the browser so it can change
        /// without a backend release.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        cards: Option<super::types::OpaqueJson>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        table: Option<super::types::ViewTablePresentation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        kanban: Option<super::types::ViewKanbanPresentation>,
    },
    Tasks {
        status: super::types::TaskStatus,
        count: u8,
    },
    WritingActivity {
        days: u16,
    },
    Publications {
        #[serde(rename = "publicationIds")]
        publication_ids: Vec<String>,
        count: u8,
    },
    Introduction {
        title: String,
        description: String,
    },
    /// Chosen writing figures for a period, such as words today and the streak.
    WritingStats {
        stats: Vec<WritingStat>,
        days: u16,
    },
    /// The workspace's writing goal: its progress, or a way to set one.
    WritingGoal {},
    /// Buttons that each start a note from one of these templates, in order.
    CreateButtons {
        #[serde(rename = "templateIds")]
        template_ids: Vec<String>,
    },
    /// How many of the workspace's notes match `query`.
    Count {
        query: super::query::QueryDocument,
    },
    /// Words added and deleted each day of a period, with the net change.
    WritingRhythm {
        days: u16,
    },
}

/// A figure the Writing stats widget can show.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum WritingStat {
    WordsToday,
    Streak,
    ActiveDays,
    WordsAdded,
    NetChange,
    TasksCompleted,
}

impl WidgetConfig {
    #[cfg(test)]
    pub(crate) const KINDS: [&'static str; 14] = [
        "continue_writing",
        "recent_notes",
        "pinned_notes",
        "workspace_shortcuts",
        "layout",
        "tasks",
        "writing_activity",
        "publications",
        "introduction",
        "writing_stats",
        "writing_goal",
        "writing_rhythm",
        "create_buttons",
        "count",
    ];
    /// The widget type, as written in `kind`.
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::ContinueWriting { .. } => "continue_writing",
            Self::RecentNotes { .. } => "recent_notes",
            Self::PinnedNotes { .. } => "pinned_notes",
            Self::WorkspaceShortcuts { .. } => "workspace_shortcuts",
            Self::Layout { .. } => "layout",
            Self::Tasks { .. } => "tasks",
            Self::WritingActivity { .. } => "writing_activity",
            Self::Publications { .. } => "publications",
            Self::Introduction { .. } => "introduction",
            Self::WritingStats { .. } => "writing_stats",
            Self::WritingGoal {} => "writing_goal",
            Self::WritingRhythm { .. } => "writing_rhythm",
            Self::CreateButtons { .. } => "create_buttons",
            Self::Count { .. } => "count",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum NotePresentation {
    List,
    Cards,
}

/// Instantiate caller-owned presentation data with fresh record-local identities.
/// Canonical validation remains independent of where a preset was authored.
pub(crate) fn instantiate(template: &Value) -> Result<Value, String> {
    validate_content(template)?;
    let mut content: super::types::ViewContent =
        serde::Deserialize::deserialize(template).map_err(|error| error.to_string())?;
    for widget in content.widgets.iter_mut().flatten() {
        widget.id = uuid::Uuid::now_v7().to_string();
    }
    serde_json::to_value(content).map_err(|error| error.to_string())
}

/// Tests consume the same asset as the browser and packaged CLI.
#[cfg(test)]
pub(crate) fn initial_content() -> Value {
    instantiate(
        &serde_json::from_str(include_str!("../../../../vendor/workspace-starter.json")).unwrap(),
    )
    .unwrap()
}

/// The largest card design a view stores, as JSON text.
const MAX_CARD_DESIGN_BYTES: usize = 16 * 1024;

pub(crate) fn validate_content(value: &Value) -> Result<(), String> {
    let content: super::types::ViewContent = serde::Deserialize::deserialize(value)
        .map_err(|error| format!("Invalid workspace content: {error}"))?;
    let query = content
        .query
        .ok_or("A workspace needs a structured query.")?;
    if query.version != 1 {
        return Err("Choose query document version 1.".into());
    }
    // Presentation and Home live in the widgets; these would be silently ignored.
    if let Some(key) = ["mode", "cards", "table", "kanban", "home", "layouts"]
        .into_iter()
        .find(|key| content.extra.contains_key(*key))
    {
        return Err(format!(
            "Workspace {key} belongs to its widgets, not to the workspace."
        ));
    }
    let widgets = content.widgets.unwrap_or_default();
    let definitions = super::home_widgets::definitions();
    let mut ids = HashSet::new();
    for widget in &widgets {
        if !super::identity::is_item_id(&widget.id) || !ids.insert(widget.id.as_str()) {
            return Err("Workspace widgets need distinct UUIDs.".into());
        }
        super::home_widgets::validate(widget, &definitions)?;
        if let WidgetConfig::CreateButtons { template_ids } = &widget.config
            && template_ids.len() > MAX_CREATE_BUTTONS
        {
            return Err(format!(
                "Widget {} can start notes from at most {MAX_CREATE_BUTTONS} templates.",
                widget.id
            ));
        }
        // A card design's shape belongs to the browser; storage only bounds it.
        if let WidgetConfig::Layout {
            cards: Some(cards), ..
        } = &widget.config
            && (!cards.0.is_object() || cards.0.to_string().len() > MAX_CARD_DESIGN_BYTES)
        {
            return Err(format!(
                "Widget {} needs a card design object of at most {MAX_CARD_DESIGN_BYTES} bytes.",
                widget.id
            ));
        }
    }
    if let Some(templates) = &content.templates {
        validate_template_offers(templates)?;
    }
    // The collection view always has a way to show the workspace's notes.
    if !widgets
        .iter()
        .any(|widget| matches!(widget.config, WidgetConfig::Layout { .. }))
    {
        return Err("A workspace needs at least one table, cards or board widget.".into());
    }
    Ok(())
}

/// At most this many templates appear in a workspace's New menu.
pub(crate) const MAX_TEMPLATE_OFFERS: usize = 12;
/// At most this many buttons appear in one Create buttons widget.
const MAX_CREATE_BUTTONS: usize = 6;

fn validate_template_offers(templates: &super::types::WorkspaceTemplates) -> Result<(), String> {
    if templates.offered.len() > MAX_TEMPLATE_OFFERS {
        return Err(format!(
            "A workspace can offer at most {MAX_TEMPLATE_OFFERS} templates."
        ));
    }
    let mut seen = HashSet::new();
    for id in &templates.offered {
        if !super::identity::is_item_id(id) || !seen.insert(id.as_str()) {
            return Err("Offered templates need distinct item IDs.".into());
        }
    }
    if let Some(default) = &templates.default
        && !seen.contains(default.as_str())
    {
        return Err("The default template must be one the workspace offers.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_offers_are_distinct_bounded_and_include_the_default() {
        let mut content = initial_content();
        let a = "01a0e2a0-0000-7000-8000-0000000000a1";
        let b = "01a0e2a0-0000-7000-8000-0000000000b2";
        content["templates"] = serde_json::json!({ "offered": [a, b], "default": b });
        validate_content(&content).unwrap();
        content["templates"] = serde_json::json!({ "offered": [a], "default": null });
        validate_content(&content).unwrap();
        content["templates"] = serde_json::json!({ "offered": [a, a], "default": null });
        assert!(validate_content(&content).is_err());
        content["templates"] = serde_json::json!({ "offered": [a], "default": b });
        assert!(validate_content(&content).is_err());
        content["templates"] = serde_json::json!({ "offered": ["not-an-id"], "default": null });
        assert!(validate_content(&content).is_err());
        content["templates"] = serde_json::json!({ "offered": [a], "default": a, "extra": 1 });
        assert!(validate_content(&content).is_err());
    }

    #[test]
    fn preset_instantiation_gives_fresh_widget_ids() {
        let template = initial_content();
        let first = instantiate(&template).unwrap();
        let second = instantiate(&template).unwrap();
        assert_ne!(first["widgets"][0]["id"], template["widgets"][0]["id"]);
        assert_ne!(first["widgets"][0]["id"], second["widgets"][0]["id"]);
        validate_content(&first).unwrap();
    }

    #[test]
    fn widgets_carry_their_own_settings_and_placements() {
        let initial = initial_content();
        validate_content(&initial).unwrap();
        let widgets = initial["widgets"].as_array().unwrap();
        // The starter pins its views to the top bar, hidden on Home.
        assert!(widgets.iter().any(|widget| {
            widget["topBar"] == true
                && widget["home"]["hidden"] == true
                && widget["config"]["kind"] == "layout"
        }));
        assert!(
            widgets
                .iter()
                .all(|widget| widget["home"]["span"].is_number())
        );
        let layout = widgets
            .iter()
            .position(|widget| widget["config"]["kind"] == "layout")
            .unwrap();
        let other = widgets
            .iter()
            .position(|widget| widget["config"]["kind"] != "layout")
            .unwrap();
        for change in [
            "text_query",
            "bad_width",
            "duplicate_id",
            "unknown_widget",
            "no_views",
            "layout_query",
            "layout_mode",
            "card_design_not_object",
            "card_design_too_large",
            "obsolete_home",
            "obsolete_layouts",
            "top_level_mode",
        ] {
            let mut content = initial.clone();
            match change {
                "text_query" => content["query"] = json!("#history"),
                "bad_width" => content["widgets"][other]["home"]["span"] = json!(5),
                "duplicate_id" => {
                    content["widgets"][1]["id"] = content["widgets"][0]["id"].clone();
                }
                "unknown_widget" => {
                    content["widgets"][other]["config"]["kind"] = json!("external_script");
                }
                "no_views" => {
                    content["widgets"] = json!(
                        widgets
                            .iter()
                            .filter(|widget| widget["config"]["kind"] != "layout")
                            .collect::<Vec<_>>()
                    );
                }
                "layout_query" => content["widgets"][layout]["config"]["query"] = json!("#a"),
                "layout_mode" => content["widgets"][layout]["config"]["mode"] = json!("dashboard"),
                "card_design_not_object" => {
                    content["widgets"][layout]["config"]["cards"] = json!(["fields"]);
                }
                "card_design_too_large" => {
                    content["widgets"][layout]["config"]["cards"] =
                        json!({"note": "x".repeat(MAX_CARD_DESIGN_BYTES)});
                }
                "obsolete_home" => content["home"] = json!({"version": 1, "widgets": []}),
                "obsolete_layouts" => content["layouts"] = json!([]),
                "top_level_mode" => content["mode"] = json!("cards"),
                _ => unreachable!(),
            }
            assert!(validate_content(&content).is_err(), "{change}");
        }
        // A card design is the browser's own JSON; storage keeps it as it is.
        let mut designed = initial.clone();
        let design = json!({"version": 1, "future": {"region": "anything"}});
        designed["widgets"][layout]["config"]["cards"] = design.clone();
        validate_content(&designed).unwrap();
        let config: WidgetConfig =
            serde_json::from_value(designed["widgets"][layout]["config"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(config).unwrap()["cards"],
            design,
            "the design round-trips unchanged"
        );
        // Any widget can be pinned to the top bar, and shown on Home as well.
        let mut pinned = initial.clone();
        pinned["widgets"][other]["topBar"] = json!(true);
        pinned["widgets"][layout]["home"] = json!({"span": 12, "height": "tall"});
        validate_content(&pinned).unwrap();
    }
}
