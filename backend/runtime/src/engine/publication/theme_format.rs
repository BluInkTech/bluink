// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Authored portable theme values, separate from target-specific serialization.
//! Package loading, publication overrides, and every formatter share this contract.

#[cfg(test)]
mod compile_tests;
pub(crate) mod css;
mod resolve;
#[cfg(test)]
mod tests;
pub(crate) mod typst;

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub(crate) use resolve::{ResolvedStyle, ResolvedTheme, resolve};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Rem,
    Px,
    Pt,
    Mm,
    In,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(type = "string"))]
#[cfg_attr(test, ts(rename = "PublicationThemeLength"))]
pub(crate) struct Length {
    pub(crate) value: f64,
    pub(crate) unit: Unit,
}

impl FromStr for Length {
    type Err = String;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        for (suffix, unit) in [
            ("rem", Unit::Rem),
            ("px", Unit::Px),
            ("pt", Unit::Pt),
            ("mm", Unit::Mm),
            ("in", Unit::In),
        ] {
            if let Some(number) = source.strip_suffix(suffix) {
                let value = number
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid length: {source}"))?;
                if value.is_finite() && value.abs() <= 100_000.0 {
                    return Ok(Self { value, unit });
                }
                return Err(format!("Length must be finite and bounded: {source}"));
            }
        }
        Err(format!("Expected a rem, px, pt, mm or in length: {source}"))
    }
}

impl TryFrom<String> for Length {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let suffix = match self.unit {
            Unit::Rem => "rem",
            Unit::Px => "px",
            Unit::Pt => "pt",
            Unit::Mm => "mm",
            Unit::In => "in",
        };
        write!(f, "{}{suffix}", self.value)
    }
}

impl From<Length> for String {
    fn from(value: Length) -> Self {
        value.to_string()
    }
}

impl Length {
    pub(crate) fn points(self, root: f64) -> f64 {
        self.value
            * match self.unit {
                Unit::Rem => root,
                Unit::Px => 0.75,
                Unit::Pt => 1.0,
                Unit::Mm => 72.0 / 25.4,
                Unit::In => 72.0,
            }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeReference"))]
pub(crate) struct Reference {
    #[serde(rename = "ref")]
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeValue"))]
pub(crate) enum Value<T> {
    Literal(T),
    Reference(Reference),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeToken"))]
pub(crate) enum Token {
    Color(Value<String>),
    Length(Value<Length>),
    Font(Value<String>),
    Number(Value<f64>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeRole"))]
pub(crate) enum Role {
    Body,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Link,
    Quote,
    Code,
    InlineCode,
    List,
    Table,
    TableHeader,
    TableCell,
    Caption,
    Footnote,
    Embed,
    Rule,
    Box,
    BoxHeading,
    BoxInfo,
    BoxAbstract,
    BoxNote,
    BoxTip,
    BoxSuccess,
    BoxImportant,
    BoxWarning,
    BoxError,
    BoxQuestion,
    BoxExample,
}

impl Role {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Heading1 => "heading1",
            Self::Heading2 => "heading2",
            Self::Heading3 => "heading3",
            Self::Heading4 => "heading4",
            Self::Heading5 => "heading5",
            Self::Heading6 => "heading6",
            Self::Link => "link",
            Self::Quote => "quote",
            Self::Code => "code",
            Self::InlineCode => "inlineCode",
            Self::List => "list",
            Self::Table => "table",
            Self::TableHeader => "tableHeader",
            Self::TableCell => "tableCell",
            Self::Caption => "caption",
            Self::Footnote => "footnote",
            Self::Embed => "embed",
            Self::Rule => "rule",
            Self::Box => "box",
            Self::BoxHeading => "boxHeading",
            Self::BoxInfo => "boxInfo",
            Self::BoxAbstract => "boxAbstract",
            Self::BoxNote => "boxNote",
            Self::BoxTip => "boxTip",
            Self::BoxSuccess => "boxSuccess",
            Self::BoxImportant => "boxImportant",
            Self::BoxWarning => "boxWarning",
            Self::BoxError => "boxError",
            Self::BoxQuestion => "boxQuestion",
            Self::BoxExample => "boxExample",
        }
    }

    pub(crate) const ALL: [Self; 31] = [
        Self::Body,
        Self::Heading1,
        Self::Heading2,
        Self::Heading3,
        Self::Heading4,
        Self::Heading5,
        Self::Heading6,
        Self::Link,
        Self::Quote,
        Self::Code,
        Self::InlineCode,
        Self::List,
        Self::Table,
        Self::TableHeader,
        Self::TableCell,
        Self::Caption,
        Self::Footnote,
        Self::Embed,
        Self::Rule,
        Self::Box,
        Self::BoxHeading,
        Self::BoxInfo,
        Self::BoxAbstract,
        Self::BoxNote,
        Self::BoxTip,
        Self::BoxSuccess,
        Self::BoxImportant,
        Self::BoxWarning,
        Self::BoxError,
        Self::BoxQuestion,
        Self::BoxExample,
    ];

    pub(crate) const fn parent(self) -> Option<Self> {
        match self {
            Self::Body => None,
            Self::BoxInfo
            | Self::BoxAbstract
            | Self::BoxNote
            | Self::BoxTip
            | Self::BoxSuccess
            | Self::BoxImportant
            | Self::BoxWarning
            | Self::BoxError
            | Self::BoxQuestion
            | Self::BoxExample => Some(Self::Box),
            Self::TableCell | Self::TableHeader => Some(Self::Table),
            _ => Some(Self::Body),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeWeight"))]
pub(crate) enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeAlignment"))]
pub(crate) enum Alignment {
    Start,
    Center,
    End,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeStroke"))]
pub(crate) enum Stroke {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeCaptionPosition"))]
pub(crate) enum CaptionPosition {
    Before,
    After,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
#[cfg_attr(test, ts(rename = "PublicationThemeStyle"))]
pub(crate) struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) font_family: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) font_size: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) font_weight: Option<Weight>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) italic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line_height: Option<Value<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) background: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) space_before: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) space_after: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) indent: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) padding: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border_width: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) border_color: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[expect(
        clippy::struct_field_names,
        reason = "Matches the authored borderStyle property and generated browser contract."
    )]
    pub(crate) border_style: Option<Stroke>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) alignment: Option<Alignment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keep_with_next: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keep_together: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) caption_position: Option<CaptionPosition>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeRoot"))]
pub(crate) struct Root {
    pub(crate) font_size: Value<Length>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeColors"))]
pub(crate) struct Colors {
    pub(crate) ink: Value<String>,
    pub(crate) muted: Value<String>,
    pub(crate) link: Value<String>,
    pub(crate) accent: Value<String>,
    pub(crate) surface: Value<String>,
    pub(crate) paper: Value<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeOrientation"))]
pub(crate) enum Orientation {
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeChapterStart"))]
pub(crate) enum ChapterStart {
    Continuous,
    NextPage,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "text",
    rename_all = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeRunningPart"))]
pub(crate) enum RunningPart {
    Text(String),
    DocumentTitle,
    ChapterTitle,
    PageNumber,
    PageCount,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeRunningLine"))]
pub(crate) struct RunningLine {
    pub(crate) left: Vec<RunningPart>,
    pub(crate) center: Vec<RunningPart>,
    pub(crate) right: Vec<RunningPart>,
    pub(crate) show_on_first_page: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemePage"))]
pub(crate) struct Page {
    pub(crate) width: Value<Length>,
    pub(crate) height: Value<Length>,
    pub(crate) orientation: Orientation,
    pub(crate) margin_top: Value<Length>,
    pub(crate) margin_right: Value<Length>,
    pub(crate) margin_bottom: Value<Length>,
    pub(crate) margin_left: Value<Length>,
    pub(crate) header_distance: Value<Length>,
    pub(crate) footer_distance: Value<Length>,
    pub(crate) chapter_start: ChapterStart,
    pub(crate) header: RunningLine,
    pub(crate) footer: RunningLine,
    pub(crate) measure_characters: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
#[cfg_attr(test, ts(rename = "PublicationThemeRunningLinePatch"))]
pub(crate) struct RunningLinePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) left: Option<Vec<RunningPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) center: Option<Vec<RunningPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) right: Option<Vec<RunningPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) show_on_first_page: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
#[cfg_attr(test, ts(rename = "PublicationThemePagePatch"))]
pub(crate) struct PagePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) height: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) orientation: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) margin_top: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) margin_right: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) margin_bottom: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) margin_left: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) header_distance: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) footer_distance: Option<Value<Length>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chapter_start: Option<ChapterStart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) header: Option<RunningLinePatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) footer: Option<RunningLinePatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) measure_characters: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
#[cfg_attr(test, ts(rename = "PublicationThemeColorsPatch"))]
pub(crate) struct ColorsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ink: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) muted: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) link: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) accent: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) surface: Option<Value<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) paper: Option<Value<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemePatch"))]
pub(crate) struct Patch {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tokens: BTreeMap<String, Token>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) root: Option<Root>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) colors: Option<ColorsPatch>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) styles: BTreeMap<Role, Style>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) page: Option<PagePatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeOverrides"))]
pub(crate) struct Overrides {
    pub(crate) shared: Patch,
    pub(crate) targets: BTreeMap<String, Patch>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeWeb"))]
pub(crate) struct Web {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) site: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) site_roles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) controls: Vec<WebControl>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeWebControl"))]
pub(crate) enum WebControl {
    Toggle {
        id: String,
        label: String,
        default: bool,
    },
    Choice {
        id: String,
        label: String,
        default: String,
        choices: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(rename = "PublicationThemeDefinition"))]
pub(crate) struct Definition {
    pub(crate) schema_version: u8,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) tokens: BTreeMap<String, Token>,
    pub(crate) root: Root,
    pub(crate) colors: Colors,
    pub(crate) styles: BTreeMap<Role, Style>,
    pub(crate) page: Page,
    pub(crate) targets: BTreeMap<String, Patch>,
    pub(crate) web: Web,
}
