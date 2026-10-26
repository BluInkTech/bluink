// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Every searchable record is one canonical item, irrespective of its type.
pub(crate) use crate::engine::project::identity::ItemId as SourceId;
use crate::projection::FileStamp;
pub(crate) use crate::projection::SearchDocument as SourceDocument;
use rayon::prelude::*;
use std::path::Path;

impl SourceId {
    pub(crate) fn read(&self, root: &Path, stamp: FileStamp) -> Result<SourceDocument, String> {
        crate::projection::read_document(&self.path(root), stamp.indexed())
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn list(root: &Path) -> Result<Vec<SourceId>, String> {
    crate::engine::project::item::list_project_item_ids(root)?
        .iter()
        .map(|id| SourceId::try_from(id.as_str()))
        .collect()
}

/// Probe canonical files in bounded parallel batches. Results keep input order
/// so callers can compare them with indexed stamps without shared mutation.
pub(crate) fn file_stamps(
    root: &Path,
    ids: &[SourceId],
    mut progress: impl FnMut(usize, usize) -> Result<(), String> + Send,
) -> Result<Vec<Option<FileStamp>>, String> {
    const BATCH_SIZE: usize = 4_096;
    crate::parallel::metadata(|| {
        let mut stamps = Vec::with_capacity(ids.len());
        for batch in ids.chunks(BATCH_SIZE) {
            let checked: Vec<_> = batch
                .par_iter()
                .map(|id| FileStamp::read(&id.path(root)))
                .collect::<Result<_, _>>()?;
            stamps.extend(checked);
            progress(stamps.len(), ids.len())?;
        }
        Ok(stamps)
    })
}

#[cfg(test)]
mod tests {
    use super::{SourceId, file_stamps};

    #[test]
    fn stamp_scan_preserves_missing_files_and_reports_file_errors() {
        let root = tempfile::tempdir().unwrap();
        let ids: Vec<_> = [
            "00000000-0000-7000-8000-000000000001",
            "00000000-0000-7000-8000-000000000002",
            "00000000-0000-7000-8000-000000000003",
        ]
        .into_iter()
        .map(SourceId::try_from)
        .collect::<Result<_, _>>()
        .unwrap();
        for id in [&ids[0], &ids[2]] {
            let path = id.path(root.path());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        }
        let mut progress = Vec::new();
        let stamps = file_stamps(root.path(), &ids, |checked, total| {
            progress.push((checked, total));
            Ok(())
        })
        .unwrap();
        assert!(stamps[0].is_some());
        assert!(stamps[1].is_none());
        assert!(stamps[2].is_some());
        assert_eq!(progress, vec![(3, 3)]);

        let invalid = ids[1].path(root.path());
        std::fs::create_dir_all(&invalid).unwrap();
        assert!(file_stamps(root.path(), &ids, |_, _| Ok(())).is_err());
    }
}
