// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{DocxDocument, styles};
use crate::engine::publication::theme_format::Role;

pub(super) const fn container_role(role: Role) -> bool {
    matches!(
        role,
        Role::Quote
            | Role::Embed
            | Role::Code
            | Role::Box
            | Role::BoxInfo
            | Role::BoxAbstract
            | Role::BoxNote
            | Role::BoxTip
            | Role::BoxSuccess
            | Role::BoxImportant
            | Role::BoxWarning
            | Role::BoxError
            | Role::BoxQuestion
            | Role::BoxExample
    )
}

impl DocxDocument<'_> {
    pub(super) fn spacer(&mut self, points: f64, keep: bool) -> Result<(), String> {
        if points <= 0.0 {
            return Ok(());
        }
        let xml = &mut self.story.xml;
        xml.start("w:p", &[])?;
        xml.start("w:pPr", &[])?;
        if keep {
            xml.value("w:keepNext", 1)?;
        }
        xml.empty(
            "w:spacing",
            &[
                ("w:before", "0".into()),
                ("w:after", "0".into()),
                ("w:line", styles::twips(points)?),
                ("w:lineRule", "exact".into()),
            ],
        )?;
        xml.start("w:rPr", &[])?;
        xml.value("w:sz", 2)?;
        xml.end()?;
        xml.end()?;
        xml.end()?;
        self.story.last_table = false;
        if let Some(content) = self.story.frame_content.last_mut() {
            *content = true;
        }
        Ok(())
    }

    pub(super) fn start_frame(&mut self, role: Role, id: Option<&str>) -> Result<(), String> {
        let style = &self.theme.styles[&role];
        let width = self.story.width() - style.indent_pt;
        let inner = (-2.0_f64).mul_add(style.padding_pt, width);
        if inner <= 0.0 {
            return Err(format!(
                "DOCX {} padding leaves no content width",
                role.name()
            ));
        }
        self.spacer(style.space_before_pt, true)?;
        self.pending_anchor(id);
        let xml = &mut self.story.xml;
        xml.start("w:tbl", &[])?;
        xml.start("w:tblPr", &[])?;
        xml.empty(
            "w:tblW",
            &[("w:w", styles::twips(width)?), ("w:type", "dxa".into())],
        )?;
        xml.empty(
            "w:tblInd",
            &[
                ("w:w", styles::twips(style.indent_pt)?),
                ("w:type", "dxa".into()),
            ],
        )?;
        xml.start("w:tblBorders", &[])?;
        for side in ["top", "left", "bottom", "right"] {
            styles::border(xml, side, style, false)?;
        }
        xml.end()?;
        styles::shading(xml, style)?;
        xml.empty("w:tblLayout", &[("w:type", "fixed".into())])?;
        xml.value("w:tblCaption", format!("Bluink {}", role.name()))?;
        xml.end()?;
        xml.start("w:tblGrid", &[])?;
        xml.empty("w:gridCol", &[("w:w", styles::twips(width)?)])?;
        xml.end()?;
        xml.start("w:tr", &[])?;
        if style.keep_together {
            xml.start("w:trPr", &[])?;
            xml.empty("w:cantSplit", &[])?;
            xml.end()?;
        }
        xml.start("w:tc", &[])?;
        xml.start("w:tcPr", &[])?;
        xml.empty(
            "w:tcW",
            &[("w:w", styles::twips(width)?), ("w:type", "dxa".into())],
        )?;
        xml.start("w:tcBorders", &[])?;
        for side in ["top", "left", "bottom", "right"] {
            styles::border(xml, side, style, false)?;
        }
        xml.end()?;
        styles::shading(xml, style)?;
        xml.start("w:tcMar", &[])?;
        for side in ["top", "left", "bottom", "right"] {
            xml.empty(
                &format!("w:{side}"),
                &[
                    ("w:w", styles::twips(style.padding_pt)?),
                    ("w:type", "dxa".into()),
                ],
            )?;
        }
        xml.end()?;
        xml.end()?;
        self.story.roles.push(role);
        self.story.frame_content.push(false);
        self.story.widths.push(inner);
        self.story.last_table = false;
        Ok(())
    }

    pub(super) fn end_frame(&mut self) -> Result<(), String> {
        let role = self.story.role();
        if self.story.last_table {
            self.spacer(0.05, false)?;
        }
        if !self.story.pending.is_empty() || self.story.frame_content.last() == Some(&false) {
            self.begin_paragraph(role, None, None)?;
            self.end_paragraph()?;
        }
        self.story.xml.end()?;
        self.story.xml.end()?;
        self.story.xml.end()?;
        self.story.roles.pop();
        self.story.frame_content.pop();
        if let Some(content) = self.story.frame_content.last_mut() {
            *content = true;
        }
        self.story.widths.pop();
        self.story.last_table = true;
        self.spacer(
            self.theme.styles[&role].space_after_pt,
            self.theme.styles[&role].keep_with_next || self.caption_follows(),
        )
    }
}
