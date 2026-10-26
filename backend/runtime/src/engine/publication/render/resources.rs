// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::Destination;
use crate::engine::publication::preflight::{LinkResolution, PlannedAsset, PlannedLink};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Read, path::Path};
use url::Url;

/// Native document links share one closed set of anchors and external URLs.
pub(crate) fn native_links(
    links: &[PlannedLink],
    source: &str,
) -> Result<HashMap<String, Destination>, String> {
    let mut output = HashMap::new();
    for link in links.iter().filter(|link| link.page_id == source) {
        let destination = match link.resolution {
            LinkResolution::Included => Destination::Anchor(format!(
                "note-{}",
                link.target_id
                    .as_deref()
                    .ok_or("Included link has no target ID")?
            )),
            LinkResolution::Anchor => Destination::Anchor(
                link.destination
                    .as_deref()
                    .and_then(|value| value.strip_prefix('#'))
                    .ok_or("Anchor has no fragment")?
                    .into(),
            ),
            LinkResolution::External => Destination::Url(
                link.destination
                    .clone()
                    .ok_or("External link has no destination")?,
            ),
            LinkResolution::TextOnly => Destination::PlainText,
        };
        if output
            .insert(link.href.clone(), destination.clone())
            .is_some_and(|previous| previous != destination)
        {
            return Err(format!(
                "Source page {source} has conflicting link targets."
            ));
        }
    }
    Ok(output)
}

/// Bound the read and verify captured bytes immediately before serialization.
pub(crate) fn read_asset(root: &Path, asset: &PlannedAsset) -> Result<Vec<u8>, String> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    if asset.bytes > LIMIT {
        return Err(format!("Asset {} exceeds the 64 MiB limit", asset.src));
    }
    let path = crate::engine::project::identity::ItemId::try_from(asset.owner_id.as_str())?
        .directory(root)
        .join("assets")
        .join(&asset.filename);
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Cannot read asset {}: {error}", asset.src))?;
    if bytes.len() as u64 != asset.bytes
        || crate::engine::publication::digest_hex(&Sha256::digest(&bytes)) != asset.sha256
    {
        return Err(format!("Asset {} changed during rendering.", asset.src));
    }
    Ok(bytes)
}

/// Output destinations are validated independently of their source spelling.
/// Relative site routes and embedded image data are legitimate planned resources.
pub(crate) fn safe_output_url(value: &str, image: bool) -> bool {
    if value.is_empty()
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    if let Ok(url) = Url::parse(value) {
        return if image {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || [
                    "data:image/png;base64,",
                    "data:image/jpeg;base64,",
                    "data:image/webp;base64,",
                    "data:image/gif;base64,",
                    "data:image/svg+xml;base64,",
                ]
                .iter()
                .any(|prefix| {
                    value.strip_prefix(prefix).is_some_and(|data| {
                        !data.is_empty()
                            && data.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                            })
                    })
                })
        } else {
            (matches!(url.scheme(), "http" | "https") && url.has_host())
                || (url.scheme() == "mailto" && !url.path().is_empty())
        };
    }
    !value
        .split('/')
        .next()
        .is_some_and(|part| part.contains(':'))
}
