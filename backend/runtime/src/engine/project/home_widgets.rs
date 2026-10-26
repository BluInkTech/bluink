// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Persisted Home configuration constraints. Gallery presentation belongs to the browser.
use super::types::{ItemType, OpaqueJson};
use super::workspace::{WidgetHeight, WidgetSpan, WorkspaceWidget};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct WidgetConstraint {
    pub(crate) kind: String,
    pub(crate) widths: Vec<WidgetSpan>,
    pub(crate) heights: Vec<WidgetHeight>,
    pub(crate) settings: Vec<WidgetSettingConstraint>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct WidgetSettingConstraint {
    pub(crate) key: String,
    pub(crate) schema: SettingSchema,
}

/// What a setting accepts. Clients use it to validate and present settings.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum SettingSchema {
    Integer {
        min: u16,
        max: u16,
    },
    Choice {
        options: Vec<OpaqueJson>,
    },
    /// One or more distinct values from `options`, in the order chosen.
    Choices {
        options: Vec<OpaqueJson>,
    },
    Text {
        #[serde(rename = "maxLength")]
        max_length: u32,
    },
    /// One item identifier, or `null` when `optional`.
    Item {
        #[serde(rename = "itemType")]
        item_type: ItemType,
        optional: bool,
    },
    /// An ordered list of distinct item identifiers.
    Items {
        #[serde(rename = "itemType")]
        item_type: ItemType,
    },
    /// A version-1 structured query document.
    Query,
}

fn setting(key: &str, schema: SettingSchema) -> WidgetSettingConstraint {
    WidgetSettingConstraint {
        key: key.into(),
        schema,
    }
}
fn choices(values: &[Value]) -> SettingSchema {
    SettingSchema::Choice {
        options: values.iter().cloned().map(OpaqueJson).collect(),
    }
}
fn definition(kind: &str, settings: Vec<WidgetSettingConstraint>) -> WidgetConstraint {
    use WidgetSpan::{Full, Half, Third, TwoThirds};
    WidgetConstraint {
        kind: kind.into(),
        widths: if matches!(kind, "layout" | "writing_rhythm") {
            vec![Half, TwoThirds, Full]
        } else {
            vec![Third, Half, TwoThirds, Full]
        },
        heights: vec![
            WidgetHeight::Compact,
            WidgetHeight::Regular,
            WidgetHeight::Tall,
        ],
        settings,
    }
}

/// The figures the Writing stats widget can show, chosen in order.
fn writing_stats() -> WidgetSettingConstraint {
    setting(
        "stats",
        SettingSchema::Choices {
            options: [
                "words_today",
                "streak",
                "active_days",
                "words_added",
                "net_change",
                "tasks_completed",
            ]
            .map(|value| OpaqueJson(json!(value)))
            .to_vec(),
        },
    )
}

pub(crate) fn definitions() -> Vec<WidgetConstraint> {
    let count = || setting("count", SettingSchema::Integer { min: 1, max: 50 });
    let period = || {
        setting(
            "days",
            choices(&[json!(7), json!(30), json!(90), json!(365)]),
        )
    };
    let notes = || {
        vec![
            count(),
            setting("presentation", choices(&[json!("cards"), json!("list")])),
        ]
    };
    vec![
        definition(
            "continue_writing",
            vec![setting(
                "noteId",
                SettingSchema::Item {
                    item_type: ItemType::Page,
                    optional: true,
                },
            )],
        ),
        definition("recent_notes", notes()),
        definition(
            "workspace_shortcuts",
            vec![setting(
                "workspaceIds",
                SettingSchema::Items {
                    item_type: ItemType::Workspace,
                },
            )],
        ),
        definition("pinned_notes", notes()),
        definition(
            "layout",
            vec![
                setting("query", SettingSchema::Query),
                setting(
                    "mode",
                    choices(&[
                        json!("cards"),
                        json!("table"),
                        json!("kanban"),
                        json!("list"),
                    ]),
                ),
            ],
        ),
        definition(
            "tasks",
            vec![
                setting("status", choices(&[json!("open"), json!("completed")])),
                count(),
            ],
        ),
        definition("writing_activity", vec![period()]),
        definition(
            "publications",
            vec![
                setting(
                    "publicationIds",
                    SettingSchema::Items {
                        item_type: ItemType::Publication,
                    },
                ),
                count(),
            ],
        ),
        definition("writing_stats", vec![writing_stats(), period()]),
        definition("writing_rhythm", vec![period()]),
        definition("writing_goal", Vec::new()),
        definition(
            "create_buttons",
            vec![setting(
                "templateIds",
                SettingSchema::Items {
                    item_type: ItemType::Template,
                },
            )],
        ),
        definition("count", vec![setting("query", SettingSchema::Query)]),
        definition(
            "introduction",
            vec![
                setting("title", SettingSchema::Text { max_length: 160 }),
                setting("description", SettingSchema::Text { max_length: 10_000 }),
            ],
        ),
    ]
}

fn accepts(schema: &SettingSchema, value: &Value) -> bool {
    let item_id = |value: &Value| value.as_str().is_some_and(super::identity::is_item_id);
    match schema {
        SettingSchema::Integer { min, max } => value
            .as_u64()
            .is_some_and(|number| (u64::from(*min)..=u64::from(*max)).contains(&number)),
        SettingSchema::Choice { options } => options.iter().any(|option| option.0 == *value),
        SettingSchema::Choices { options } => value.as_array().is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| options.iter().any(|option| option.0 == *value))
                && values
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == values.len()
        }),
        SettingSchema::Text { max_length, .. } => value
            .as_str()
            .is_some_and(|text| text.chars().count() <= *max_length as usize),
        SettingSchema::Item { optional, .. } => (*optional && value.is_null()) || item_id(value),
        SettingSchema::Items { .. } => value.as_array().is_some_and(|ids| {
            ids.iter().all(item_id)
                && ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len()
        }),
        SettingSchema::Query => value["version"] == 1,
    }
}

/// The longest heading a widget may replace its type's name with.
pub(crate) const TITLE_LENGTH: u32 = 80;

/// Check one saved widget against its type's definition.
pub(crate) fn validate(
    widget: &WorkspaceWidget,
    definitions: &[WidgetConstraint],
) -> Result<(), String> {
    let config = serde_json::to_value(&widget.config).map_err(|error| error.to_string())?;
    let definition = definitions
        .iter()
        .find(|definition| definition.kind == widget.config.kind())
        .ok_or_else(|| format!("Widget {} has an unknown type.", widget.id))?;
    if let Some(title) = &widget.title
        && (title.trim().is_empty()
            || title.trim() != title
            || title.chars().count() > TITLE_LENGTH as usize
            || title.contains(char::is_control))
    {
        return Err(format!(
            "Give widget {} a title of 1 to {TITLE_LENGTH} characters on one line, or none.",
            widget.id
        ));
    }
    if let Some(home) = widget.home {
        if let Some(height) = home.height
            && !definition.heights.contains(&height)
        {
            return Err(format!(
                "Choose a height {} allows for widget {}.",
                definition.kind, widget.id
            ));
        }
        if !definition.widths.contains(&home.span) {
            return Err(format!(
                "Choose a width {} allows for widget {}.",
                definition.kind, widget.id
            ));
        }
    }
    for setting in &definition.settings {
        if !accepts(&setting.schema, &config[&setting.key]) {
            return Err(format!(
                "Widget {} has an invalid {}.",
                widget.id, setting.key
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::workspace::{HomePlacement, WidgetConfig};
    use super::*;
    #[test]
    fn every_type_has_constraints_and_initial_home_is_valid() {
        let definitions = definitions();
        let kinds: std::collections::HashSet<_> =
            definitions.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(kinds.len(), definitions.len());
        assert_eq!(kinds, WidgetConfig::KINDS.into_iter().collect());
        let initial: Vec<WorkspaceWidget> =
            serde_json::from_value(super::super::workspace::initial_content()["widgets"].clone())
                .unwrap();
        for widget in initial {
            validate(&widget, &definitions).unwrap();
        }
    }
    #[test]
    fn settings_reject_values_outside_their_schema() {
        let definitions = definitions();
        let widget = |span, config: Value| WorkspaceWidget {
            id: "01a15a12-b26a-71a4-bfd6-7bfa6551f71d".into(),
            title: None,
            home: Some(HomePlacement {
                span,
                height: None,
                hidden: None,
            }),
            top_bar: None,
            config: serde_json::from_value(config).unwrap(),
        };
        for (span, config) in [
            (
                WidgetSpan::Third,
                json!({"kind":"recent_notes","count":0,"presentation":"list"}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"recent_notes","count":51,"presentation":"list"}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"writing_activity","days":14}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"writing_stats","stats":[],"days":30}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"writing_stats","stats":["streak","streak"],"days":30}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"writing_stats","stats":["streak"],"days":14}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"continue_writing","noteId":"not-an-id"}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"workspace_shortcuts","workspaceIds":["01a15a12-b26a-71a4-bfd6-7bfa6551f71d","01a15a12-b26a-71a4-bfd6-7bfa6551f71d"]}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"introduction","title":"x".repeat(161),"description":""}),
            ),
            (
                WidgetSpan::Third,
                json!({"kind":"layout","query":{"version":1,"clauses":[],"sort":[]},"mode":"cards"}),
            ),
        ] {
            assert!(
                validate(&widget(span, config.clone()), &definitions).is_err(),
                "{config}"
            );
        }
        let notes = json!({"kind":"recent_notes","count":5,"presentation":"list"});
        for title in [
            "",
            "  ",
            " Drafts",
            "Two
lines",
            &"x".repeat(TITLE_LENGTH as usize + 1),
        ] {
            let mut titled = widget(WidgetSpan::Third, notes.clone());
            titled.title = Some(title.into());
            assert!(validate(&titled, &definitions).is_err(), "{title:?}");
        }
    }
}
