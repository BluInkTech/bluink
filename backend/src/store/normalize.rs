// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Versioned search normalization shared by projection and query compilation.
//! Token rules match the former default analyzer; authored values stay unchanged.

pub(crate) const VERSION: i64 = 1;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) position: usize,
    pub(crate) text: String,
}

pub(crate) fn tokens(value: &str) -> Vec<Token> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .enumerate()
        .filter(|(_, part)| part.len() < 40)
        .map(|(position, part)| Token {
            position,
            text: part.chars().flat_map(char::to_lowercase).collect(),
        })
        .collect()
}

/// Title ordering retains the existing case-insensitive title comparator.
pub(crate) fn sort_key(value: &str) -> String {
    value.to_lowercase()
}

/// ASCII tokens keep FTS5's Unicode version and diacritic folding from changing
/// membership. Every user token has the `t` prefix; `gap` cannot be user text.
pub(crate) fn fts_token(value: &str) -> String {
    let mut output = String::with_capacity(value.len() * 2 + 1);
    append_fts_token(&mut output, value);
    output
}

fn append_fts_token(output: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push('t');
    for byte in value.bytes() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
}

pub(crate) fn fts_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len() * 2);
    for part in value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        if !output.is_empty() {
            output.push(' ');
        }
        if part.len() >= 40 {
            output.push_str("gap");
        } else {
            let normalized: String = part.chars().flat_map(char::to_lowercase).collect();
            append_fts_token(&mut output, &normalized);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_tokens_match_the_frozen_analyzer() {
        let vectors: Vec<(String, Vec<(usize, String)>)> =
            serde_json::from_str(include_str!("query/fixtures/tantivy-tokens.json")).unwrap();
        for (input, expected) in vectors {
            let actual = tokens(&input)
                .into_iter()
                .map(|token| (token.position, token.text))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{input}");
        }
    }
    #[test]
    fn tokens_preserve_unicode_punctuation_and_removed_word_positions() {
        let input = format!("Café CAFÉ ΣΟΣ İ alpha_beta {} end", "x".repeat(40));
        let tokens = tokens(&input);
        assert_eq!(
            tokens
                .iter()
                .map(|token| (token.position, token.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, "café"),
                (1, "café"),
                (2, "σοσ"),
                (3, "i\u{307}"),
                (4, "alpha"),
                (5, "beta"),
                (7, "end")
            ]
        );
        assert_eq!(fts_text("one two"), "t6f6e65 t74776f");
        assert_eq!(
            fts_text(&format!("one {} two", "x".repeat(40))),
            "t6f6e65 gap t74776f"
        );
    }
}
