// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use crate::domain::publication::preflight::PlannedAsset;
use std::path::Path;

const ASSET_LIMIT: u64 = 64 * 1024 * 1024;

fn asset_object(asset: &PlannedAsset) -> Result<crate::store::assets::Object, String> {
    if asset.bytes > ASSET_LIMIT {
        return Err(format!("Asset {} exceeds the 64 MiB limit", asset.src));
    }
    Ok(crate::store::assets::Object {
        oid: asset.sha256.clone(),
        size: asset.bytes,
    })
}

/// Bound the read and verify captured bytes immediately before serialization.
pub(crate) fn read_asset(root: &Path, asset: &PlannedAsset) -> Result<Vec<u8>, String> {
    asset_object(asset)?
        .read_verified(root, ASSET_LIMIT)
        .map_err(|error| format!("Cannot read asset {}: {error}", asset.src))
}

/// Stream verified captured bytes into an output file without holding them.
pub(crate) fn copy_asset(
    root: &Path,
    asset: &PlannedAsset,
    output: &mut impl std::io::Write,
) -> Result<(), String> {
    asset_object(asset)?
        .copy_verified(root, output)
        .map_err(|error| format!("Cannot read asset {}: {error}", asset.src))
}
