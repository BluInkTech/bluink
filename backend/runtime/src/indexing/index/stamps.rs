// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;

use crate::indexing::source::SourceId;
use crate::projection::FileStamp;
use tantivy::IndexReader;

/// Read only reconciliation metadata, reusing fast-field decoding buffers.
pub(crate) fn indexed_file_stamps(
    reader: &IndexReader,
    progress: impl Fn(usize, usize) -> Result<(), String>,
) -> Result<HashMap<SourceId, Option<FileStamp>>, String> {
    let searcher = reader.searcher();
    let total = usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX);
    let mut stamps = HashMap::new();
    let (mut id, mut stamp) = (String::new(), String::new());
    let mut processed = 0usize;
    for segment in searcher.segment_readers() {
        let fields = segment.fast_fields();
        let ids = fields
            .str("id")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Missing id fast field".to_owned())?;
        let values = fields
            .str("index_stamp")
            .map_err(|error| error.to_string())?;
        for doc_id in segment.doc_ids_alive() {
            processed += 1;
            if processed == total || processed.is_multiple_of(256) {
                progress(processed, total)?;
            }
            let Some(values) = &values else {
                continue;
            };
            let (Some(id_ord), Some(stamp_ord)) = (
                ids.term_ords(doc_id).next(),
                values.term_ords(doc_id).next(),
            ) else {
                continue;
            };
            ids.ord_to_str(id_ord, &mut id)
                .map_err(|error| error.to_string())?;
            values
                .ord_to_str(stamp_ord, &mut stamp)
                .map_err(|error| error.to_string())?;
            // An invalid stamp is a cache miss, so the canonical item is rebuilt.
            if let Ok(id) = SourceId::try_from(id.as_str()) {
                stamps.insert(id, FileStamp::parse_index(&stamp));
            }
        }
    }
    Ok(stamps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexing::index::{build_schema, open_reader, open_writer};
    use tantivy::Index;

    #[test]
    fn progress_failure_stops_reconciliation_instead_of_returning_partial_stamps() {
        let schema = build_schema();
        let id = schema.get_field("id").unwrap();
        let stamp = schema.get_field("index_stamp").unwrap();
        let index = Index::create_in_ram(schema);
        let mut writer = open_writer(&index).unwrap();
        writer
            .add_document(tantivy::doc!(
                id => "00000000-0000-0000-0000-000000000001",
                stamp => "invalid cache stamp"
            ))
            .unwrap();
        writer.commit().unwrap();
        writer.wait_merging_threads().unwrap();
        let reader = open_reader(&index).unwrap();
        let result = indexed_file_stamps(&reader, |_, _| Err("Coordinator unavailable".to_owned()));
        assert_eq!(result.err().as_deref(), Some("Coordinator unavailable"));
    }
}
