// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod activity;
pub(crate) mod assemble;
pub(crate) mod asset;
pub(crate) mod catalogues;
pub(crate) mod config;
pub(crate) mod envelope;
pub(crate) mod facets;
pub(crate) mod goal;
pub(crate) mod home_widgets;
pub(crate) mod identity;
pub(crate) mod io;
pub(crate) mod item;
pub(crate) mod item_mutations;
pub(crate) mod named_items;
pub(crate) mod properties;
pub(crate) mod query;
pub(crate) mod reports;
pub(crate) mod setup;
pub(crate) mod snippets;
pub(crate) mod storage_error;
pub(crate) mod task;
pub(crate) mod template;
pub(crate) mod transaction;
pub(crate) mod usage;
pub(crate) mod workspace;
pub(crate) mod write;

#[cfg(test)]
mod audit;
#[cfg(test)]
mod contract_tests;

pub(crate) mod types;
