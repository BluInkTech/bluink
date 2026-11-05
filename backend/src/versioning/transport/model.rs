// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use serde::{Deserialize, Serialize};

/// Never derive Debug: inbound secrets must not appear in diagnostics.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct AuthInput {
    pub(crate) account: String,
    pub(crate) token: Option<String>,
    pub(crate) private_key: Option<String>,
    pub(crate) passphrase: Option<String>,
    pub(crate) forget: bool,
    pub(crate) use_agent: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct AuthStatus {
    pub(crate) account: String,
    pub(crate) has_token: bool,
    pub(crate) private_key: Option<String>,
    pub(crate) host_fingerprint: Option<String>,
    pub(crate) use_agent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HostIdentity {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) fingerprint: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TrustInput {
    pub(crate) fingerprint: String,
    /// Must match the prior stored trust when explicitly replacing a key.
    pub(crate) previous_fingerprint: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct GenerateKeyInput {
    pub(crate) name: String,
    pub(crate) account: String,
    pub(crate) passphrase: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct GeneratedKey {
    pub(crate) private_key: String,
    pub(crate) public_key: String,
    pub(crate) fingerprint: String,
}
