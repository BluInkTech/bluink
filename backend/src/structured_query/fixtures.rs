// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typed query builders for tests. Values are literal data, never expressions.
use super::document::{Comparison, Predicate, QueryDocument, QuerySort, SortDirection, TextMode};
use crate::domain::project::query::{Condition, Polarity, StatePredicate};

pub(crate) fn criteria<const N: usize>(clauses: [Condition; N]) -> QueryDocument {
    QueryDocument {
        version: 1,
        clauses: clauses.into(),
        sort: Vec::new(),
    }
}

fn required(predicate: Predicate) -> Condition {
    Condition {
        polarity: Polarity::Required,
        predicate,
    }
}

pub(crate) fn exclude(mut condition: Condition) -> Condition {
    condition.polarity = Polarity::Excluded;
    condition
}

pub(crate) fn term(value: &str) -> Condition {
    required(Predicate::Text {
        value: value.into(),
        mode: TextMode::Terms,
    })
}

pub(crate) fn phrase(value: &str) -> Condition {
    required(Predicate::Text {
        value: value.into(),
        mode: TextMode::Phrase,
    })
}

pub(crate) fn tag(key: &str) -> Condition {
    required(Predicate::Tag { key: key.into() })
}

pub(crate) fn label(key: &str) -> Condition {
    required(Predicate::Label { key: key.into() })
}

pub(crate) fn field(key: &str, value: &str) -> Condition {
    comparison(key, Comparison::Match, value)
}

pub(crate) fn comparison(key: &str, operator: Comparison, value: &str) -> Condition {
    required(Predicate::Field {
        field: key.into(),
        operator,
        value: value.into(),
    })
}

pub(crate) fn greater_than(key: &str, value: &str) -> Condition {
    comparison(key, Comparison::Gt, value)
}

pub(crate) fn at_least(key: &str, value: &str) -> Condition {
    comparison(key, Comparison::Gte, value)
}

pub(crate) fn less_than(key: &str, value: &str) -> Condition {
    comparison(key, Comparison::Lt, value)
}

pub(crate) fn at_most(key: &str, value: &str) -> Condition {
    comparison(key, Comparison::Lte, value)
}

pub(crate) fn has(key: &str) -> Condition {
    required(Predicate::Has { field: key.into() })
}

pub(crate) fn pinned() -> Condition {
    required(Predicate::Is {
        value: StatePredicate::Pinned,
    })
}

pub(crate) fn archived() -> Condition {
    required(Predicate::Is {
        value: StatePredicate::Archived,
    })
}

pub(crate) fn asc(key: &str) -> QuerySort {
    QuerySort {
        field: key.into(),
        direction: SortDirection::Asc,
    }
}

pub(crate) fn desc(key: &str) -> QuerySort {
    QuerySort {
        field: key.into(),
        direction: SortDirection::Desc,
    }
}

impl QueryDocument {
    pub(crate) fn sorted<const N: usize>(mut self, sort: [QuerySort; N]) -> Self {
        self.sort = sort.into();
        self
    }
}
