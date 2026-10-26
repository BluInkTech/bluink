// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::{DocxDocument, styles};
pub(super) use crate::engine::publication::media::Asset;
use crate::engine::{
    publication::render::{ExportError, Image},
    xml::Xml,
};
use std::path::Path;

const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
pub(super) const ASVG: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";

pub(super) fn namespaces() -> [(&'static str, String); 4] {
    [
        ("xmlns:wp", WP.into()),
        ("xmlns:a", A.into()),
        ("xmlns:pic", PIC.into()),
        ("xmlns:asvg", ASVG.into()),
    ]
}

impl DocxDocument<'_> {
    pub(crate) fn add_image_resource(
        &mut self,
        key: &str,
        bytes: Vec<u8>,
        fonts: Option<&Path>,
    ) -> Result<(), ExportError> {
        if self.assets.contains_key(key) {
            return Err(ExportError::new(
                "DOCX image resource key was registered twice",
            ));
        }
        self.assets.insert(key.into(), {
            let mut asset = Asset::parse_with_fonts(bytes, fonts, &mut self.svg_fonts)
                .map_err(ExportError::new)?;
            asset.primary.path = format!("word/{}", asset.primary.path);
            if let Some(vector) = &mut asset.vector {
                vector.path = format!("word/{}", vector.path);
            }
            asset
        });
        Ok(())
    }

    pub(super) fn image(&mut self, image: &Image, id: Option<&str>) -> Result<(), String> {
        let asset = self
            .assets
            .get_mut(&image.asset)
            .ok_or_else(|| format!("DOCX image resource was not loaded: {}", image.source))?;
        let (width, height) = match (image.width_pt, image.height_pt) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width * asset.height / asset.width),
            (None, Some(height)) => (height * asset.width / asset.height, height),
            (None, None) => (asset.width, asset.height),
        };
        let scale = 1.0_f64.min(self.story.width() / width).min(
            (self.theme.page.height_pt
                - self.theme.page.margin_top_pt
                - self.theme.page.margin_bottom_pt)
                / height,
        );
        let width = width * scale;
        let height = height * scale;
        for part in std::iter::once(&mut asset.primary).chain(asset.vector.iter_mut()) {
            if self.media_parts.insert(part.path.clone()) {
                self.package
                    .add(&part.path, &part.kind, std::mem::take(&mut part.bytes))?;
            }
        }
        let primary = self
            .package
            .relate(&self.story.owner, "image", &asset.primary.path)?;
        let vector = asset
            .vector
            .as_ref()
            .map(|part| self.package.relate(&self.story.owner, "image", &part.path))
            .transpose()?;
        self.story.drawing = true;
        self.begin_paragraph(self.story.role(), id, None)?;
        self.story.drawing = false;
        self.begin_run()?;
        self.drawing_id += 1;
        drawing(
            &mut self.story.xml,
            self.drawing_id,
            &image.alt,
            width,
            height,
            &primary,
            vector.as_deref(),
        )?;
        self.story.xml.end()?;
        self.end_paragraph()
    }
}

fn drawing(
    xml: &mut Xml,
    id: usize,
    alt: &str,
    width: f64,
    height: f64,
    primary: &str,
    vector: Option<&str>,
) -> Result<(), String> {
    let cx = styles::integer(width, 12700.0, "image width")?;
    let cy = styles::integer(height, 12700.0, "image height")?;
    xml.start("w:drawing", &[])?;
    xml.start(
        "wp:inline",
        &[
            ("distT", "0".into()),
            ("distB", "0".into()),
            ("distL", "0".into()),
            ("distR", "0".into()),
        ],
    )?;
    xml.empty("wp:extent", &[("cx", cx.clone()), ("cy", cy.clone())])?;
    xml.empty(
        "wp:effectExtent",
        &[
            ("l", "0".into()),
            ("t", "0".into()),
            ("r", "0".into()),
            ("b", "0".into()),
        ],
    )?;
    xml.empty(
        "wp:docPr",
        &[
            ("id", id.to_string()),
            ("name", format!("Bluink image {id}")),
            ("descr", alt.into()),
        ],
    )?;
    xml.start("wp:cNvGraphicFramePr", &[])?;
    xml.empty("a:graphicFrameLocks", &[("noChangeAspect", "1".into())])?;
    xml.end()?;
    xml.start("a:graphic", &[])?;
    xml.start("a:graphicData", &[("uri", PIC.into())])?;
    xml.start("pic:pic", &[])?;
    xml.start("pic:nvPicPr", &[])?;
    xml.empty(
        "pic:cNvPr",
        &[
            ("id", "0".into()),
            ("name", format!("Image {id}")),
            ("descr", alt.into()),
        ],
    )?;
    xml.empty("pic:cNvPicPr", &[])?;
    xml.end()?;
    xml.start("pic:blipFill", &[])?;
    xml.start("a:blip", &[("r:embed", primary.into())])?;
    if let Some(vector) = vector {
        xml.start("a:extLst", &[])?;
        xml.start(
            "a:ext",
            &[("uri", "{96DAC541-7B7A-43D3-8B79-37D633B846F1}".into())],
        )?;
        xml.empty("asvg:svgBlip", &[("r:embed", vector.into())])?;
        xml.end()?;
        xml.end()?;
    }
    xml.end()?;
    xml.start("a:stretch", &[])?;
    xml.empty("a:fillRect", &[])?;
    xml.end()?;
    xml.end()?;
    xml.start("pic:spPr", &[])?;
    xml.start("a:xfrm", &[])?;
    xml.empty("a:off", &[("x", "0".into()), ("y", "0".into())])?;
    xml.empty("a:ext", &[("cx", cx), ("cy", cy)])?;
    xml.end()?;
    xml.start("a:prstGeom", &[("prst", "rect".into())])?;
    xml.empty("a:avLst", &[])?;
    xml.end()?;
    xml.end()?;
    xml.end()?;
    xml.end()?;
    xml.end()?;
    xml.end()?;
    xml.end()
}
