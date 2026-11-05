// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

pub(crate) mod activity;
pub(crate) use bluink_format::{canonical_json, identity};
pub(crate) mod assemble;
pub(crate) mod asset;
pub(crate) mod candidate;
pub(crate) mod config;
pub(crate) mod envelope;
pub(crate) mod facets;
pub(crate) mod goal;
pub(crate) mod home_widgets;
pub(crate) mod item;
pub(crate) mod named_items;
pub(crate) mod properties;
pub(crate) mod query;
pub(crate) mod reports;
pub(crate) mod setup;
pub(crate) mod snippets;
pub(crate) mod submissions;
pub(crate) mod task;
pub(crate) mod template;
pub(crate) mod workspace;

#[cfg(test)]
mod audit;
#[cfg(test)]
mod contract_tests;

pub(crate) mod types;
