// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Rebuildable index storage, service lifetime and background projection work.

pub(crate) mod coordinator;
pub(crate) mod fast;
pub(crate) mod index;
mod lifecycle;
pub(crate) mod service;
pub(crate) mod source;
pub(crate) mod state;
mod worker;
