// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use std::collections::HashMap;

use crate::project::{FileStamp, ItemId};
use tantivy::IndexReader;

/// Read only reconciliation metadata, reusing fast-field decoding buffers.
pub(crate) fn indexed_file_stamps(
    reader: &IndexReader,
    id_field: &str,
    markers_only: bool,
    progress: impl Fn(usize, usize),
) -> Result<HashMap<ItemId, Option<FileStamp>>, String> {
    let searcher = reader.searcher();
    let total = usize::try_from(searcher.num_docs()).unwrap_or(usize::MAX);
    let mut stamps = HashMap::new();
    let (mut id, mut stamp, mut kind) = (String::new(), String::new(), String::new());
    let mut processed = 0usize;
    for segment in searcher.segment_readers() {
        let fields = segment.fast_fields();
        let ids = fields
            .str(id_field)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Missing {id_field} fast field"))?;
        let values = fields
            .str("index_stamp")
            .map_err(|error| error.to_string())?
            .ok_or("Missing index_stamp fast field")?;
        let kinds = if markers_only {
            Some(
                fields
                    .str("record_kind")
                    .map_err(|error| error.to_string())?
                    .ok_or("Missing record_kind fast field")?,
            )
        } else {
            None
        };
        for doc_id in segment.doc_ids_alive() {
            processed += 1;
            if processed == total || processed.is_multiple_of(256) {
                progress(processed, total);
            }
            if let Some(kinds) = &kinds {
                let Some(ordinal) = kinds.term_ords(doc_id).next() else {
                    continue;
                };
                kinds
                    .ord_to_str(ordinal, &mut kind)
                    .map_err(|error| error.to_string())?;
                if kind != "marker" {
                    continue;
                }
            }
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
            if let Some(id) = ItemId::parse(&id) {
                stamps.insert(id, FileStamp::parse_index(&stamp));
            }
        }
    }
    Ok(stamps)
}
