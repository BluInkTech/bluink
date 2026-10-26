// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::indexing::index::write_document;
use crate::indexing::source::SourceDocument;
use tantivy::schema::{Field, Schema};
use tantivy::{Index, IndexWriter, Term};

/// All records derived from a source share an owner and one commit boundary.
pub(super) struct Projections {
    pub(super) main: Schema,
    owner: Field,
}
impl Projections {
    pub(super) fn new(index: &Index) -> Result<Self, String> {
        let main = index.schema();
        Ok(Self {
            owner: main.get_field("id").map_err(|error| error.to_string())?,
            main,
        })
    }
    pub(super) fn write(
        &self,
        writer: &mut IndexWriter,
        source: SourceDocument,
    ) -> Result<(), String> {
        self.delete(writer, &source.id);
        write_document(writer, &self.main, source).map_err(|error| error.to_string())
    }
    pub(super) fn delete(&self, writer: &IndexWriter, id: &str) {
        writer.delete_term(Term::from_field_text(self.owner, id));
    }
}
