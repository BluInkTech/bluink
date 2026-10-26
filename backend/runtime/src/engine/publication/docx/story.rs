// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::tables::Table;
use crate::engine::{
    publication::{render::Mark, theme_format::Role},
    xml::Xml,
};

#[expect(
    clippy::struct_excessive_bools,
    reason = "Paragraph, list, drawing, note marker and trailing table states are independent story facts, not mutually exclusive modes."
)]
pub(super) struct Story {
    pub(super) xml: Xml,
    pub(super) owner: String,
    pub(super) marks: Vec<Mark>,
    pub(super) paragraph: bool,
    pub(super) paragraph_role: Role,
    pub(super) drawing: bool,
    pub(super) paragraph_bookmarks: Vec<usize>,
    pub(super) pending: Vec<String>,
    pub(super) roles: Vec<Role>,
    pub(super) frame_content: Vec<bool>,
    pub(super) widths: Vec<f64>,
    pub(super) lists: Vec<usize>,
    pub(super) list_item: bool,
    pub(super) table: Option<Table>,
    pub(super) note_marker: bool,
    pub(super) last_table: bool,
}

impl Story {
    pub(super) fn new(xml: Xml, owner: &str, width: f64, role: Role) -> Self {
        Self {
            xml,
            owner: owner.into(),
            marks: Vec::new(),
            paragraph: false,
            paragraph_role: role,
            drawing: false,
            paragraph_bookmarks: Vec::new(),
            pending: Vec::new(),
            roles: vec![role],
            frame_content: Vec::new(),
            widths: vec![width],
            lists: Vec::new(),
            list_item: false,
            table: None,
            note_marker: false,
            last_table: false,
        }
    }
    #[expect(
        clippy::expect_used,
        reason = "Story construction seeds the base width and role; balanced render events only pop nested frames."
    )]
    pub(super) fn width(&self) -> f64 {
        *self.widths.last().expect("story has a content width")
    }
    #[expect(
        clippy::expect_used,
        reason = "Story construction seeds the base width and role; balanced render events only pop nested frames."
    )]
    pub(super) fn role(&self) -> Role {
        *self.roles.last().expect("story has a text role")
    }
    pub(super) const fn balanced(&self) -> bool {
        !self.paragraph
            && self.pending.is_empty()
            && self.marks.is_empty()
            && self.lists.is_empty()
            && self.table.is_none()
            && self.roles.len() == 1
            && self.frame_content.is_empty()
            && self.widths.len() == 1
    }
}
