// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{DocxDocument, styles};
use crate::engine::publication::{render::Cell, theme_format::Role};

#[derive(Clone)]
struct Span {
    column: usize,
    colspan: usize,
    last_row: usize,
    header: bool,
}
pub(super) struct Table {
    columns: Vec<f64>,
    spans: Vec<Span>,
    row: usize,
    rows: usize,
    column: usize,
    joins_next_row: bool,
    pub(super) cell_content: bool,
}

impl DocxDocument<'_> {
    pub(super) fn start_table(
        &mut self,
        widths: &[f64],
        rows: usize,
        id: Option<&str>,
    ) -> Result<(), String> {
        if self.story.table.is_some() {
            return Err("DOCX does not allow a source table inside another table cell".into());
        }
        self.pending_anchor(id);
        let style = &self.theme.styles[&Role::Table];
        let width = self.story.width() - style.indent_pt;
        if width <= 0.0 {
            return Err("DOCX table indent leaves no content width".into());
        }
        self.spacer(style.space_before_pt, true)?;
        let xml = &mut self.story.xml;
        xml.start("w:tbl", &[])?;
        xml.start("w:tblPr", &[])?;
        xml.value("w:tblStyle", styles::id(Role::Table))?;
        xml.empty(
            "w:tblW",
            &[("w:w", styles::twips(width)?), ("w:type", "dxa".into())],
        )?;
        xml.empty("w:tblLayout", &[("w:type", "fixed".into())])?;
        xml.end()?;
        xml.start("w:tblGrid", &[])?;
        let total: f64 = widths.iter().sum();
        let mut cumulative = 0.0;
        let mut previous = 0.0;
        let mut columns = Vec::new();
        for track in widths {
            cumulative += track;
            let edge = (width * 20.0 * cumulative / total).round() / 20.0;
            let column = edge - previous;
            if column <= 0.0 {
                return Err("DOCX table column is smaller than one twip".into());
            }
            columns.push(column);
            previous = edge;
            xml.empty("w:gridCol", &[("w:w", styles::twips(column)?)])?;
        }
        xml.end()?;
        self.story.table = Some(Table {
            columns,
            spans: Vec::new(),
            row: 0,
            rows,
            column: 0,
            joins_next_row: false,
            cell_content: false,
        });
        self.story.last_table = false;
        Ok(())
    }

    pub(super) fn start_row(
        &mut self,
        index: usize,
        repeat: bool,
        joins_next_row: bool,
        id: Option<&str>,
    ) -> Result<(), String> {
        let table = self
            .story
            .table
            .as_mut()
            .ok_or("DOCX table row has no table")?;
        table.row = index;
        table.column = 0;
        table.joins_next_row = joins_next_row;
        table.spans.retain(|span| span.last_row >= index);
        self.pending_anchor(id);
        self.story.xml.start("w:tr", &[])?;
        let together = self.theme.styles[&Role::Table].keep_together;
        if repeat || together || joins_next_row {
            self.story.xml.start("w:trPr", &[])?;
            if together || joins_next_row {
                self.story.xml.empty("w:cantSplit", &[])?;
            }
            if repeat {
                self.story.xml.empty("w:tblHeader", &[])?;
            }
            self.story.xml.end()?;
        }
        Ok(())
    }

    fn continue_until(&mut self, limit: usize) -> Result<(), String> {
        loop {
            let table = self
                .story
                .table
                .as_ref()
                .ok_or("DOCX table continuation has no table")?;
            if table.column >= limit {
                break;
            }
            let span = table
                .spans
                .iter()
                .find(|span| span.column == table.column)
                .cloned();
            let column = table.column;
            let row = table.row;
            let (colspan, header) = span
                .as_ref()
                .map_or((1, false), |span| (span.colspan, span.header));
            self.open_cell(
                Cell {
                    row,
                    column,
                    colspan,
                    rowspan: 1,
                    header,
                },
                span.is_some(),
            )?;
            self.begin_paragraph(
                if header {
                    Role::TableHeader
                } else {
                    Role::TableCell
                },
                None,
                None,
            )?;
            self.end_paragraph()?;
            self.close_cell()?;
        }
        Ok(())
    }

    pub(super) fn start_cell(&mut self, cell: Cell, id: Option<&str>) -> Result<(), String> {
        self.continue_until(cell.column)?;
        self.pending_anchor(id);
        self.open_cell(cell, false)?;
        if cell.rowspan > 1 {
            self.story
                .table
                .as_mut()
                .ok_or("DOCX cell has no table")?
                .spans
                .push(Span {
                    column: cell.column,
                    colspan: cell.colspan,
                    last_row: cell.row + cell.rowspan - 1,
                    header: cell.header,
                });
        }
        Ok(())
    }

    fn open_cell(&mut self, cell: Cell, continuation: bool) -> Result<(), String> {
        let table = self.story.table.as_ref().ok_or("DOCX cell has no table")?;
        let width = table
            .columns
            .get(cell.column..cell.column + cell.colspan)
            .ok_or("DOCX cell exceeds the table grid")?
            .iter()
            .sum::<f64>();
        let role = if cell.header {
            Role::TableHeader
        } else {
            Role::TableCell
        };
        let style = &self.theme.styles[&role];
        if width <= 2.0 * style.padding_pt {
            return Err("DOCX table cell padding leaves no content width".into());
        }
        let xml = &mut self.story.xml;
        xml.start("w:tc", &[])?;
        xml.start("w:tcPr", &[])?;
        xml.empty(
            "w:tcW",
            &[("w:w", styles::twips(width)?), ("w:type", "dxa".into())],
        )?;
        if cell.colspan > 1 {
            xml.value("w:gridSpan", cell.colspan)?;
        }
        if continuation {
            xml.value("w:vMerge", "continue")?;
        } else if cell.rowspan > 1 {
            xml.value("w:vMerge", "restart")?;
        }
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
        xml.value("w:vAlign", "top")?;
        xml.end()?;
        self.story.roles.push(role);
        self.story
            .widths
            .push((-2.0_f64).mul_add(style.padding_pt, width));
        self.story
            .table
            .as_mut()
            .ok_or("DOCX cell has no table")?
            .column = cell.column + cell.colspan;
        self.story
            .table
            .as_mut()
            .ok_or("DOCX cell has no table")?
            .cell_content = false;
        self.story.last_table = false;
        Ok(())
    }

    pub(super) fn close_cell(&mut self) -> Result<(), String> {
        if !self.story.pending.is_empty()
            || self.story.last_table
            || !self
                .story
                .table
                .as_ref()
                .ok_or("DOCX cell has no table")?
                .cell_content
        {
            self.begin_paragraph(self.story.role(), None, None)?;
            self.end_paragraph()?;
        }
        self.story.xml.end()?;
        self.story.roles.pop();
        self.story.widths.pop();
        Ok(())
    }

    pub(super) fn end_row(&mut self) -> Result<(), String> {
        let columns = self
            .story
            .table
            .as_ref()
            .ok_or("DOCX row has no table")?
            .columns
            .len();
        self.continue_until(columns)?;
        self.story.xml.end()
    }

    pub(super) fn end_table(&mut self) -> Result<(), String> {
        self.story.xml.end()?;
        self.story.table = None;
        self.story.last_table = true;
        self.spacer(
            self.theme.styles[&Role::Table].space_after_pt,
            self.caption_follows() || self.theme.styles[&Role::Table].keep_with_next,
        )
    }
}

impl Table {
    pub(super) const fn joins_next_row(&self) -> bool {
        self.joins_next_row
    }

    pub(super) const fn last_row(&self) -> bool {
        self.row + 1 == self.rows
    }
}
