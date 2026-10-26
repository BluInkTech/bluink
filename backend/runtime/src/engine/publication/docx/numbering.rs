// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::W;
use crate::engine::xml::Xml;

#[derive(Default)]
pub(super) struct Numbering {
    instances: Vec<(bool, u64, u8)>,
}

impl Numbering {
    pub(super) fn list(&mut self, ordered: bool, start: u64, level: u8) -> Result<usize, String> {
        if start > i32::MAX.unsigned_abs().into() || level > 8 {
            return Err("DOCX numbering start or level is outside the native range".into());
        }
        self.instances.push((ordered, start, level));
        Ok(self.instances.len())
    }

    pub(super) fn compile(
        &self,
        theme: &crate::engine::publication::theme_format::ResolvedTheme,
        fonts: &super::fonts::Fonts,
        language: &str,
    ) -> Result<Vec<u8>, String> {
        let mut xml = Xml::new();
        xml.start("w:numbering", &[("xmlns:w", W.into())])?;
        for (abstract_id, ordered) in [(0, false), (1, true)] {
            xml.start(
                "w:abstractNum",
                &[("w:abstractNumId", abstract_id.to_string())],
            )?;
            xml.value("w:multiLevelType", "multilevel")?;
            for level in 0..9 {
                xml.start("w:lvl", &[("w:ilvl", level.to_string())])?;
                xml.value("w:start", 1)?;
                xml.value("w:numFmt", if ordered { "decimal" } else { "bullet" })?;
                xml.value(
                    "w:lvlText",
                    if ordered {
                        format!("%{}.", level + 1)
                    } else {
                        "•".into()
                    },
                )?;
                xml.value("w:lvlJc", "left")?;
                xml.start("w:pPr", &[])?;
                xml.start("w:tabs", &[])?;
                xml.empty(
                    "w:tab",
                    &[
                        ("w:val", "num".into()),
                        ("w:pos", ((level + 1) * 360).to_string()),
                    ],
                )?;
                xml.end()?;
                xml.empty(
                    "w:ind",
                    &[
                        ("w:left", ((level + 1) * 360).to_string()),
                        ("w:hanging", "360".into()),
                    ],
                )?;
                xml.end()?;
                let style = &theme.styles[&crate::engine::publication::theme_format::Role::List];
                super::styles::run_properties(
                    &mut xml,
                    style,
                    fonts.family(&style.font_family)?,
                    language,
                    false,
                    false,
                )?;
                xml.end()?;
            }
            xml.end()?;
        }
        for (index, (ordered, start, level)) in self.instances.iter().enumerate() {
            xml.start("w:num", &[("w:numId", (index + 1).to_string())])?;
            xml.value("w:abstractNumId", u8::from(*ordered))?;
            xml.start("w:lvlOverride", &[("w:ilvl", level.to_string())])?;
            xml.value("w:startOverride", start)?;
            xml.end()?;
            xml.end()?;
        }
        xml.end()?;
        xml.finish()
    }
}
