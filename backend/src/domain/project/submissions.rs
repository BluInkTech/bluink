// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Durable draft acknowledgements, committed with their canonical item group.
use serde::Serialize;
use sha2::{Digest, Sha256};
pub(crate) fn hash(value: &impl Serialize) -> Result<String, String> {
    let bytes = super::canonical_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .flat_map(|byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect())
}
