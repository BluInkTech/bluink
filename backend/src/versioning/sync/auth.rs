// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
use crate::versioning::{
    History, Result,
    transport::{self, model::*},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct SyncOrigin {
    pub(crate) origin: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct ConfigureSyncAuth {
    pub(crate) origin: String,
    pub(crate) auth: AuthInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct TrustSyncHost {
    pub(crate) origin: String,
    pub(crate) trust: TrustInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct GenerateSyncKey {
    pub(crate) origin: String,
    pub(crate) key: GenerateKeyInput,
}

impl History {
    pub(crate) fn cancel_sync(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.owners <= 1 {
            self.sync_cancel
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    pub(in crate::versioning) fn auth(
        &self,
        remote: &transport::Remote,
    ) -> Result<transport::Auth> {
        let mut auth = transport::Auth::load(&self.user_directory, remote)?;
        auth.cancel = Some(self.cancellation()?);
        auth.check_cancelled()?;
        Ok(auth)
    }
    pub(in crate::versioning) fn cancellation(
        &self,
    ) -> Result<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        let token = self.sync_cancel.lock().map_err(|_| {
            crate::versioning::failure("Synchronization cancellation lock was poisoned")
        })?;
        Ok(std::sync::Arc::clone(&token))
    }
    pub(crate) fn generate_sync_key(&self, request: &GenerateSyncKey) -> Result<GeneratedKey> {
        transport::keys::generate_key(
            &self.user_directory,
            &transport::Remote::parse(&request.origin)?,
            &request.key,
        )
    }
    pub(crate) fn read_sync_auth(&self, origin: &str) -> Result<AuthStatus> {
        Ok(
            transport::Auth::load(&self.user_directory, &transport::Remote::parse(origin)?)?
                .status(),
        )
    }
    pub(crate) fn configure_sync_auth(&self, request: ConfigureSyncAuth) -> Result<AuthStatus> {
        transport::credentials::configure_auth(
            &self.user_directory,
            &transport::Remote::parse(&request.origin)?,
            request.auth,
        )
    }
    pub(crate) fn trust_sync_host(&self, request: &TrustSyncHost) -> Result<AuthStatus> {
        let remote = transport::Remote::parse(&request.origin)?;
        transport::credentials::trust_host(&self.user_directory, &remote, &request.trust)?;
        self.read_sync_auth(&request.origin)
    }
}
