// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::de::DeserializeOwned;

use super::error::{Result, invalid_request};

/// The HTTP and host adapters decode into the same request types. Dynamic
/// document/property values remain fields inside those types, not a second
/// untyped request envelope with duplicated validation.
pub(crate) fn decode<T: DeserializeOwned>(source: &[u8]) -> Result<T> {
    if source.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        // Object requests take the typed path below. Validate rejected scalar
        // and array payloads too, so malformed JSON keeps its transport error.
        serde_json::from_slice::<serde_json::Value>(source)
            .map_err(|_| invalid_request("ERR_INVALID_JSON"))?;
        return Err(invalid_request("Request body must be an object"));
    }
    serde_json::from_slice(source).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            return invalid_request("ERR_INVALID_JSON");
        }
        let message = error.to_string();
        let message = message.split(" at line ").next().unwrap_or(&message);
        message
            .strip_prefix("missing field `")
            .and_then(|field| field.strip_suffix('`'))
            .map_or_else(
                || invalid_request(message),
                |field| invalid_request(format!("Missing required field: {field}")),
            )
    })
}

#[cfg(test)]
mod tests {
    use super::decode;
    use crate::application::error::ApplicationError;
    use crate::application::items::{QueryItemsRequest, ReadItemRequest};

    #[test]
    fn decoding_preserves_required_fields_and_rejects_invalid_payloads() {
        for (source, expected) in [
            ("{}", "Missing required field: id"),
            ("[]", "Request body must be an object"),
            ("null", "Request body must be an object"),
            ("\"text\"", "Request body must be an object"),
            ("", "ERR_INVALID_JSON"),
            ("not-json", "ERR_INVALID_JSON"),
            ("[", "ERR_INVALID_JSON"),
            ("{", "ERR_INVALID_JSON"),
            (r#"{"id":"\uD800"}"#, "ERR_INVALID_JSON"),
            (r#""\uD800""#, "ERR_INVALID_JSON"),
        ] {
            let Err(ApplicationError::InvalidRequest(message)) =
                decode::<ReadItemRequest>(source.as_bytes())
            else {
                panic!("Expected a request validation error");
            };
            assert_eq!(message, expected);
        }
        assert!(decode::<QueryItemsRequest>(br#"{"query":"", "limit":-1}"#).is_err());
        assert!(decode::<QueryItemsRequest>(br#"{"query":"", "limit":1.5}"#).is_err());
        let decoded = decode::<ReadItemRequest>(r#"{"id":"文学 🌍"}"#.as_bytes()).unwrap();
        assert_eq!(decoded.id, "文学 🌍");
        assert_eq!(decoded.options.content, None);
    }
}
