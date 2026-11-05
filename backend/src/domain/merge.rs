// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Candidate construction and decision preparation. Item merging stays pure;
//! decision preparation uses the shared envelope assembler for fresh revisions.
//! This module never writes files, publishes an index or applies a candidate. The
//! accepting coordinator must replay tombstones and validate complete packages.
//!
//! Save and sync coordinators validate captured revisions or the database
//! generation before publishing a complete plan in one transaction.

pub(crate) mod capture;
pub(crate) mod decisions;
mod document;
pub(crate) mod groups;
pub(crate) mod item;
pub(crate) mod model;
mod sequence;
pub(crate) mod validation;
mod value;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
