// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Portable semantic query values, independent of runtime search and UI state.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum Polarity {
    Required,
    Excluded,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QueryDocument {
    pub(crate) version: u8,
    pub(crate) clauses: Vec<Condition>,
    pub(crate) sort: Vec<QuerySort>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct Condition {
    pub(crate) polarity: Polarity,
    pub(crate) predicate: Predicate,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum Predicate {
    Text {
        value: String,
        mode: TextMode,
    },
    Tag {
        key: String,
    },
    Label {
        key: String,
    },
    Field {
        field: String,
        operator: Comparison,
        value: String,
    },
    Has {
        field: String,
    },
    Is {
        value: StatePredicate,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum TextMode {
    Terms,
    Phrase,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum Comparison {
    Match,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum StatePredicate {
    Archived,
    Pinned,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct QuerySort {
    pub(crate) field: String,
    pub(crate) direction: SortDirection,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ConditionDiagnostic {
    pub(crate) path: String,
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub(crate) end: Option<usize>,
}

impl QueryDocument {
    pub(crate) const fn empty() -> Self {
        Self {
            version: 1,
            clauses: Vec::new(),
            sort: Vec::new(),
        }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.version == 1 && self.clauses.is_empty() && self.sort.is_empty()
    }

    /// Conjoin scopes without erasing either one's criteria. Only an explicit
    /// temporary sort replaces the saved scope's explicit ordering.
    pub(crate) fn compose(&self, temporary: &Self) -> Result<Self, Vec<ConditionDiagnostic>> {
        if self.version != 1 || temporary.version != 1 {
            return Err(vec![ConditionDiagnostic {
                path: "/version".into(),
                code: "semantic".into(),
                message: "Choose query document version 1.".into(),
                start: None,
                end: None,
            }]);
        }
        Ok(Self {
            version: 1,
            clauses: self
                .clauses
                .iter()
                .chain(&temporary.clauses)
                .cloned()
                .collect(),
            sort: if temporary.sort.is_empty() {
                self.sort.clone()
            } else {
                temporary.sort.clone()
            },
        })
    }

    pub(crate) fn active_pages() -> Self {
        Self {
            version: 1,
            clauses: [("type", "page"), ("state", "active")]
                .into_iter()
                .map(|(field, value)| Condition {
                    polarity: Polarity::Required,
                    predicate: Predicate::Field {
                        field: field.into(),
                        operator: Comparison::Match,
                        value: value.into(),
                    },
                })
                .collect(),
            sort: Vec::new(),
        }
    }
}

impl Default for QueryDocument {
    fn default() -> Self {
        Self::empty()
    }
}
