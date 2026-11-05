// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Portable JSON encoding shared by the runtime and demo generator.
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::Value;

const ENVELOPE: &[&str] = &[
    "format",
    "formatVersion",
    "id",
    "revision",
    "createdAt",
    "modifiedAt",
    "meta",
    "tags",
    "pins",
    "content",
    "analytics",
];

struct Ordered<'a> {
    value: &'a Value,
    envelope: bool,
}

fn entries(object: &serde_json::Map<String, Value>, envelope: bool) -> Vec<(&str, &Value)> {
    let mut entries: Vec<_> = object
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    let envelope = envelope && object.get("format").and_then(Value::as_str) == Some("bluink.item");
    entries.sort_unstable_by_key(|(key, _)| {
        (
            if envelope {
                ENVELOPE
                    .iter()
                    .position(|known| known == key)
                    .unwrap_or(ENVELOPE.len())
            } else {
                0
            },
            *key,
        )
    });
    entries
}

impl Serialize for Ordered<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(values) = self.value.as_array() {
            let mut sequence = serializer.serialize_seq(Some(values.len()))?;
            for value in values {
                sequence.serialize_element(&Self {
                    value,
                    envelope: false,
                })?;
            }
            return sequence.end();
        }
        let Some(object) = self.value.as_object() else {
            return self.value.serialize(serializer);
        };
        let mut map = serializer.serialize_map(Some(object.len()))?;
        // Explicit ordering also works when a consumer enables preserve_order.
        for (key, value) in entries(object, self.envelope) {
            map.serialize_entry(
                key,
                &Self {
                    value,
                    envelope: false,
                },
            )?;
        }
        map.end()
    }
}

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    let mut bytes = Vec::new();
    pretty(&mut bytes, &value, 0, true)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Deterministic compact bytes for immutable database blocks and their digests.
#[allow(dead_code, reason = "Shared with the portable-only demo generator.")]
pub fn compact(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&Ordered {
        value,
        envelope: false,
    })
}

fn newline(bytes: &mut Vec<u8>, depth: usize) {
    bytes.push(b'\n');
    bytes.resize(bytes.len() + depth * 2, b' ');
}

/// Let serde own all escaping and primitive encoding. Only container whitespace
/// is customized: one complete authored block per line, with readable metadata.
fn pretty(
    bytes: &mut Vec<u8>,
    value: &Value,
    depth: usize,
    envelope: bool,
) -> Result<(), serde_json::Error> {
    match value {
        Value::Object(object) => {
            bytes.push(b'{');
            for (index, (key, child)) in entries(object, envelope).into_iter().enumerate() {
                if index > 0 {
                    bytes.push(b',');
                }
                newline(bytes, depth + 1);
                serde_json::to_writer(&mut *bytes, key)?;
                bytes.extend_from_slice(b": ");
                if key == "content"
                    && object.get("type").and_then(Value::as_str) == Some("doc")
                    && let Some(blocks) = child.as_array()
                {
                    array(bytes, blocks, depth + 1, true)?;
                } else if key == "paragraph"
                    && child.get("type").and_then(Value::as_str) == Some("paragraph")
                {
                    serde_json::to_writer(
                        &mut *bytes,
                        &Ordered {
                            value: child,
                            envelope: false,
                        },
                    )?;
                } else {
                    pretty(bytes, child, depth + 1, false)?;
                }
            }
            if !object.is_empty() {
                newline(bytes, depth);
            }
            bytes.push(b'}');
        }
        Value::Array(values) => array(bytes, values, depth, false)?,
        _ => serde_json::to_writer(bytes, value)?,
    }
    Ok(())
}

fn array(
    bytes: &mut Vec<u8>,
    values: &[Value],
    depth: usize,
    blocks: bool,
) -> Result<(), serde_json::Error> {
    bytes.push(b'[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            bytes.push(b',');
        }
        newline(bytes, depth + 1);
        if blocks {
            serde_json::to_writer(
                &mut *bytes,
                &Ordered {
                    value,
                    envelope: false,
                },
            )?;
        } else {
            pretty(bytes, value, depth + 1, false)?;
        }
    }
    if !values.is_empty() {
        newline(bytes, depth);
    }
    bytes.push(b']');
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn orders_envelope_and_nested_keys_with_lf_termination() {
        let bytes = super::to_vec(
            &serde_json::json!({"content":{"z":1,"a":2},"id":"fixture","format":"bluink.item"}),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\n  \"format\": \"bluink.item\",\n  \"id\": \"fixture\",\n  \"content\": {\n    \"a\": 2,\n    \"z\": 1\n  }\n}\n"
        );
    }

    #[test]
    fn authored_blocks_are_single_lines_and_one_edit_changes_one_line() {
        let mut value = json!({
            "format":"bluink.item", "id":"fixture", "tags":["history"],
            "meta":{"type":"page","z":2,"a":1},
            "content":{"document":{"type":"doc","content":[
                {"type":"paragraph","attrs":{"id":"first"},"content":[{"type":"text","text":"Line one\nquoted \"two\" \\ 😀"}]},
                {"type":"list","attrs":{"id":"second","ordered":false},"content":[{"type":"listItem","attrs":{"id":"nested"},"content":[{"type":"paragraph","attrs":{"id":"inside"},"content":[{"type":"text","text":"Nested text"}]}]}]}
            ]}}
        });
        let bytes = super::to_vec(&value).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            value
        );
        let encoded = String::from_utf8(bytes).unwrap();
        let blocks: Vec<_> = encoded
            .lines()
            .filter(|line| line.trim_start().starts_with("{\"attrs\""))
            .collect();
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("Line one\\nquoted \\\"two\\\" \\\\ 😀"));
        assert!(blocks[1].contains("Nested text"));
        assert!(blocks[1].contains("\"listItem\""));
        assert!(encoded.contains("\"tags\": [\n    \"history\"\n  ]"));
        assert!(encoded.contains("\"meta\": {\n    \"a\": 1,"));
        assert!(!encoded.contains('\r'));
        value["content"]["document"]["content"][0]["content"][0]["text"] =
            json!("Changed paragraph");
        let changed = String::from_utf8(super::to_vec(&value).unwrap()).unwrap();
        assert_eq!(encoded.lines().count(), changed.lines().count());
        assert_eq!(
            encoded
                .lines()
                .zip(changed.lines())
                .filter(|(before, after)| before != after)
                .count(),
            1
        );
    }

    #[test]
    fn snippets_compact_their_paragraph_but_keep_other_properties_readable() {
        let value = json!({"paragraph":{"type":"paragraph","content":[{"type":"text","text":"Reference"}]},"source":{"url":"example","title":"Source"}});
        let encoded = String::from_utf8(super::to_vec(&value).unwrap()).unwrap();
        assert!(encoded.contains("  \"paragraph\": {\"content\":[{\"text\":\"Reference\",\"type\":\"text\"}],\"type\":\"paragraph\"},\n"));
        assert!(encoded.contains("  \"source\": {\n    \"title\": \"Source\","));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            value
        );
    }
}
