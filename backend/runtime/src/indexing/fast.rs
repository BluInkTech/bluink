// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Open string columns once per segment, then reuse them across matching rows.
pub(crate) struct Strings(tantivy::columnar::StrColumn);

impl From<tantivy::columnar::StrColumn> for Strings {
    fn from(column: tantivy::columnar::StrColumn) -> Self {
        Self(column)
    }
}

impl Strings {
    pub(crate) fn open(segment: &tantivy::SegmentReader, name: &str) -> Result<Self, String> {
        segment
            .fast_fields()
            .str(name)
            .map_err(|error| error.to_string())?
            .map(Self)
            .ok_or_else(|| format!("Missing {name} fast field"))
    }

    /// Decode only requested ordinals, in dictionary order. Repeated random
    /// `ord_to_str` calls would reopen and rescan the same compressed blocks.
    pub(crate) fn rows(&self, docs: impl Iterator<Item = u32>) -> Result<Vec<Vec<String>>, String> {
        let mut rows = Vec::new();
        let mut ordinals = Vec::new();
        for (row, doc) in docs.enumerate() {
            rows.push(Vec::new());
            ordinals.extend(self.0.term_ords(doc).map(|ordinal| (ordinal, row)));
        }
        ordinals.sort_unstable();
        let mut positions = ordinals.iter();
        let found = self
            .0
            .dictionary()
            .sorted_ords_to_term_cb(ordinals.iter().map(|(ordinal, _)| *ordinal), |bytes| {
                let (_, row) = positions
                    .next()
                    .ok_or_else(|| std::io::Error::other("Unexpected dictionary term"))?;
                let text = std::str::from_utf8(bytes).map_err(std::io::Error::other)?;
                rows[*row].push(text.to_owned());
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        if !found {
            return Err("Indexed string ordinal is missing from its dictionary".into());
        }
        Ok(rows)
    }
}

pub(crate) fn matching_ids(
    searcher: &tantivy::Searcher,
    query: &dyn tantivy::query::Query,
) -> Result<Vec<String>, String> {
    let mut hits: Vec<_> = searcher
        .search(query, &tantivy::collector::DocSetCollector)
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect();
    hits.sort_unstable();
    let mut result = Vec::with_capacity(hits.len());
    for group in hits.chunk_by(|left, right| left.segment_ord == right.segment_ord) {
        let column = Strings::open(searcher.segment_reader(group[0].segment_ord), "id")?;
        for mut row in column.rows(group.iter().map(|hit| hit.doc_id))? {
            result.push(row.pop().ok_or("Indexed candidate has no ID")?);
        }
    }
    result.sort_unstable();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::schema::{FAST, STORED, STRING, Schema, TantivyDocument, Value};

    #[test]
    fn bulk_strings_preserve_rows_missing_values_and_repeated_ordinals() {
        let mut schema = Schema::builder();
        let id = schema.add_text_field("id", STRING | FAST);
        let labels = schema.add_text_field("labels", STRING | FAST);
        let index = tantivy::Index::create_in_ram(schema.build());
        let mut writer = super::super::index::open_writer(&index).unwrap();
        writer
            .add_document(tantivy::doc!(id => "first", labels => "café", labels => "日本語"))
            .unwrap();
        writer
            .add_document(tantivy::doc!(id => "second", labels => "日本語"))
            .unwrap();
        writer.add_document(tantivy::doc!(id => "missing")).unwrap();
        writer.commit().unwrap();
        let reader = super::super::index::open_reader(&index).unwrap();
        let searcher = reader.searcher();
        let column = Strings::open(searcher.segment_reader(0), "labels").unwrap();
        assert_eq!(
            column.rows([2, 0, 1, 0].into_iter()).unwrap(),
            vec![
                Vec::<String>::new(),
                vec!["café".into(), "日本語".into()],
                vec!["日本語".into()],
                vec!["café".into(), "日本語".into()],
            ]
        );
    }

    #[test]
    #[ignore = "Explicit comparative benchmark of stored-document and fast-column ID reads."]
    fn compare_candidate_reads() {
        let mut schema = Schema::builder();
        let id = schema.add_text_field("id", STRING | FAST | STORED);
        let metadata = schema.add_text_field("metadata", STORED);
        let index = tantivy::Index::create_in_ram(schema.build());
        let mut writer = super::super::index::open_writer(&index).unwrap();
        for number in 0..50_000 {
            writer.add_document(tantivy::doc!(id => format!("item-{number:06}"), metadata => format!("Note {number}: {}", "A page with stored metadata and preview. ".repeat(16)))).unwrap();
        }
        writer.commit().unwrap();
        let reader = super::super::index::open_reader(&index).unwrap();
        let searcher = reader.searcher();
        for run in 1..=3 {
            let started = std::time::Instant::now();
            let mut stored: Vec<_> = searcher
                .search(
                    &tantivy::query::AllQuery,
                    &tantivy::collector::DocSetCollector,
                )
                .unwrap()
                .into_iter()
                .map(|address| {
                    let document: TantivyDocument = searcher.doc(address).unwrap();
                    document.get_first(id).unwrap().as_str().unwrap().to_owned()
                })
                .collect();
            stored.sort_unstable();
            let stored_time = started.elapsed();
            let started = std::time::Instant::now();
            let fast = matching_ids(&searcher, &tantivy::query::AllQuery).unwrap();
            let fast_time = started.elapsed();
            assert_eq!(stored, fast);
            eprintln!(
                "50,000 candidate IDs, run {run}: stored {stored_time:?}, fast {fast_time:?}"
            );
        }
    }
}
