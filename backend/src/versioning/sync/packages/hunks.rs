// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
//! Let libgit2 find conflicts, then replace each marker with a provisional
//! fragment and a separate decision. Marker text never enters canonical files.
use super::*;
use decisions::file::{FileScope, propose_scoped};
use serde_json::json;

enum Piece {
    Common(String),
    Conflict {
        base: String,
        left: String,
        right: String,
    },
}

pub(super) fn merge(
    context: &Context,
    path: &str,
    base: &str,
    left: &str,
    right: &str,
) -> Result<(String, Vec<Proposal>)> {
    let size = marker_size([base, left, right])?;
    let mut options = git2::MergeFileOptions::new();
    options
        .style_diff3(true)
        .marker_size(size)
        .ancestor_label("base")
        .our_label("left")
        .their_label("right");
    let merged = git2::merge_file(
        git2::MergeFileInput::new()
            .content(base.as_bytes())
            .path(std::path::Path::new(path)),
        git2::MergeFileInput::new()
            .content(left.as_bytes())
            .path(std::path::Path::new(path)),
        git2::MergeFileInput::new()
            .content(right.as_bytes())
            .path(std::path::Path::new(path)),
        Some(&mut options),
    )?;
    let marked =
        std::str::from_utf8(merged.content()).map_err(|error| failure(error.to_string()))?;
    if merged.is_automergeable() {
        return Ok((marked.into(), Vec::new()));
    }
    let pieces = pieces(marked, size)?;
    let mut output = String::new();
    let mut proposals = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Common(text) => output.push_str(text),
            Piece::Conflict {
                base: b,
                left: l,
                right: r,
            } => {
                let before = common(&pieces[index - 1])?;
                let after = common(&pieces[index + 1])?;
                let finish = index + 2 == pieces.len();
                let prior: Vec<_> = before.split_inclusive('\n').collect();
                let next: Vec<_> = after.split_inclusive('\n').collect();
                let scope = FileScope::Text {
                    before: prior[prior.len().saturating_sub(3)..].concat(),
                    after: next[..next.len().min(3)].concat(),
                    start: index == 1 && prior.len() <= 3,
                    end: finish && next.len() <= 3,
                };
                let fragment = |text: &str, original: &str| {
                    // diff3 must put the following marker on a fresh line.
                    let raw = if finish && after.is_empty() && !original.ends_with('\n') {
                        text.strip_suffix('\n').unwrap_or(text)
                    } else {
                        text
                    };
                    if raw.is_empty() {
                        Fragment::Missing
                    } else {
                        Fragment::Present(json!(raw))
                    }
                };
                let proposal = propose_scoped(
                    context,
                    path,
                    scope,
                    fragment(b, base),
                    fragment(l, left),
                    fragment(r, right),
                )?;
                if let Fragment::Present(value) = proposal.selected_file(Selection::Provisional)? {
                    output.push_str(value.as_str().ok_or_else(|| failure("Invalid text hunk"))?);
                }
                proposals.push(proposal);
            }
        }
    }
    for proposal in &proposals {
        proposal
            .file_decision()
            .ok_or_else(|| failure("Missing file decision"))?
            .current(Some(&output))?;
    }
    Ok((output, proposals))
}

fn common(piece: &Piece) -> Result<&str> {
    match piece {
        Piece::Common(text) => Ok(text),
        Piece::Conflict { .. } => Err(failure("Adjacent merge hunks have no independent anchor")),
    }
}

fn marker_size(inputs: [&str; 3]) -> Result<u16> {
    let mut size = 16usize;
    for line in inputs.into_iter().flat_map(str::lines) {
        if let Some(ch) = line.bytes().next()
            && matches!(ch, b'<' | b'|' | b'=' | b'>')
        {
            size = size.max(line.bytes().take_while(|byte| *byte == ch).count() + 1);
        }
    }
    u16::try_from(size).map_err(|_| failure("Theme text contains an unrepresentable marker"))
}

fn pieces(text: &str, size: u16) -> Result<Vec<Piece>> {
    let n = usize::from(size);
    let start = format!("{} left", "<".repeat(n));
    let base = format!("{} base", "|".repeat(n));
    let middle = "=".repeat(n);
    let end = format!("{} right", ">".repeat(n));
    let mut lines = text.split_inclusive('\n');
    let mut pieces = Vec::new();
    let mut common = String::new();
    while let Some(line) = lines.next() {
        if line.trim_end_matches(['\r', '\n']) != start {
            common.push_str(line);
            continue;
        }
        pieces.push(Piece::Common(std::mem::take(&mut common)));
        let mut until = |marker: &str| -> Result<String> {
            let mut body = String::new();
            for line in lines.by_ref() {
                if line.trim_end_matches(['\r', '\n']) == marker {
                    return Ok(body);
                }
                body.push_str(line);
            }
            Err(failure("Malformed libgit2 conflict region"))
        };
        pieces.push(Piece::Conflict {
            left: until(&base)?,
            base: until(&middle)?,
            right: until(&end)?,
        });
    }
    if pieces.is_empty() {
        return Err(failure("Unrecognized libgit2 conflict region"));
    }
    pieces.push(Piece::Common(common));
    Ok(pieces)
}
