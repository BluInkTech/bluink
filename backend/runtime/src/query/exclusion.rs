// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Negation membership must tolerate probes behind a scorer's current hit.
use tantivy::query::{ConstScorer, EnableScoring, Explanation, Occur, Query, Scorer, Weight};
use tantivy::{DocId, Score, SegmentReader, Term};

#[derive(Debug)]
struct ExclusionQuery(Box<dyn Query>);

pub(super) fn guard(occur: Occur, query: Box<dyn Query>) -> Box<dyn Query> {
    // ExclusionSet may probe behind a phrase scorer's current hit. Negation
    // never contributes scores, so use a scorer with DocSet's safe seek path.
    if occur == Occur::MustNot {
        Box::new(ExclusionQuery(query))
    } else {
        query
    }
}

impl Clone for ExclusionQuery {
    fn clone(&self) -> Self {
        Self(self.0.box_clone())
    }
}

impl Query for ExclusionQuery {
    fn weight(&self, scoring: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
        Ok(Box::new(ExclusionWeight(self.0.weight(scoring)?)))
    }

    fn query_terms<'a>(&'a self, visitor: &mut dyn FnMut(&'a Term, bool)) {
        self.0.query_terms(visitor);
    }
}

struct ExclusionWeight(Box<dyn Weight>);

impl Weight for ExclusionWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> tantivy::Result<Box<dyn Scorer>> {
        // ConstScorer deliberately uses DocSet's default seek_danger, which
        // checks the current hit before calling the wrapped scorer's seek.
        // Unlike ConstScoreQuery, this also wraps when scoring is disabled,
        // as it is for BooleanQuery's MustNot children and membership reports.
        Ok(Box::new(ConstScorer::new(
            self.0.scorer(reader, boost)?,
            0.0,
        )))
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> tantivy::Result<Explanation> {
        self.0.explain(reader, doc)
    }
}
