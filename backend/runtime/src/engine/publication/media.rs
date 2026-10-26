// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded, offline image normalization shared by packaged document exporters.
use image::{ImageFormat, ImageReader};
use quick_xml::{Reader, XmlVersion, events::Event};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::Path};

pub(crate) struct Part {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: Vec<u8>,
}
pub(crate) struct Asset {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) primary: Part,
    pub(crate) vector: Option<Part>,
}

/// Per-export cache; fonts cannot leak across exports or outlive their work.
#[derive(Default)]
pub(crate) struct SvgFonts(
    std::collections::HashMap<std::path::PathBuf, std::sync::Arc<resvg::usvg::fontdb::Database>>,
);

impl SvgFonts {
    fn load(
        &mut self,
        folder: &Path,
    ) -> Result<std::sync::Arc<resvg::usvg::fontdb::Database>, String> {
        if !folder.is_dir() {
            return Err("Packaged SVG font folder is missing".into());
        }
        if let Some(database) = self.0.get(folder) {
            return Ok(std::sync::Arc::clone(database));
        }
        let mut files = std::fs::read_dir(folder)
            .map_err(|error| error.to_string())?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        files.sort();
        let mut database = resvg::usvg::fontdb::Database::new();
        for file in files {
            if file.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("ttf") || extension.eq_ignore_ascii_case("otf")
            }) {
                database.load_font_data(std::fs::read(file).map_err(|error| error.to_string())?);
            }
        }
        let database = std::sync::Arc::new(database);
        self.0
            .insert(folder.to_owned(), std::sync::Arc::clone(&database));
        Ok(database)
    }
}

impl Asset {
    #[cfg(test)]
    pub(crate) fn parse(bytes: Vec<u8>, fonts: Option<&Path>) -> Result<Self, String> {
        Self::parse_with_fonts(bytes, fonts, &mut SvgFonts::default())
    }

    pub(crate) fn parse_with_fonts(
        bytes: Vec<u8>,
        fonts: Option<&Path>,
        cache: &mut SvgFonts,
    ) -> Result<Self, String> {
        if bytes.len() > 64 * 1024 * 1024 {
            return Err("Publication image exceeds the 64 MiB limit".into());
        }
        if let Ok(format) = image::guess_format(&bytes) {
            let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
                .into_dimensions()
                .map_err(|error| format!("Invalid Publication image dimensions: {error}"))?;
            dimensions(width, height)?;
            let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(256 * 1024 * 1024);
            reader.limits(limits);
            let decoded = reader
                .decode()
                .map_err(|error| format!("Cannot decode Publication image: {error}"))?;
            let (extension, kind, bytes) = match format {
                ImageFormat::Png => ("png", "image/png", bytes),
                ImageFormat::Jpeg => ("jpg", "image/jpeg", bytes),
                ImageFormat::Gif | ImageFormat::WebP => {
                    let mut png = Cursor::new(Vec::new());
                    decoded
                        .write_to(&mut png, ImageFormat::Png)
                        .map_err(|error| error.to_string())?;
                    ("png", "image/png", png.into_inner())
                }
                _ => return Err("Unsupported Publication image format".into()),
            };
            Ok(Self {
                width: f64::from(width) * 0.75,
                height: f64::from(height) * 0.75,
                primary: part(extension, kind, bytes),
                vector: None,
            })
        } else {
            let has_text = check_svg(&bytes)?;
            let mut options = resvg::usvg::Options::default();
            options.image_href_resolver.resolve_string = Box::new(|_, _| None);
            options.font_family = "Noto Sans".into();
            if let Some(folder) = fonts {
                options.fontdb = cache.load(folder)?;
                if has_text && options.fontdb.faces().next().is_none() {
                    return Err("Packaged SVG font folder contains no usable faces".into());
                }
            } else if has_text {
                return Err("SVG text requires the packaged offline font folder".into());
            }
            let tree = resvg::usvg::Tree::from_data(&bytes, &options)
                .map_err(|error| format!("Cannot decode Publication SVG: {error}"))?;
            let size = tree.size();
            let scale = 3.125_f32.min(4096.0 / size.width().max(size.height()));
            let width = format!("{:.0}", (size.width() * scale).ceil())
                .parse::<u32>()
                .map_err(|error| error.to_string())?;
            let height = format!("{:.0}", (size.height() * scale).ceil())
                .parse::<u32>()
                .map_err(|error| error.to_string())?;
            dimensions(width, height)?;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
                .ok_or("Cannot allocate Publication SVG fallback")?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
            let png = pixmap.encode_png().map_err(|error| error.to_string())?;
            Ok(Self {
                width: f64::from(size.width()) * 0.75,
                height: f64::from(size.height()) * 0.75,
                primary: part("png", "image/png", png),
                // Serialize the resolved tree so Word receives self-contained
                // paths, including offline text outlines, rather than source
                // CSS, external references or fonts it could resolve differently.
                vector: Some(part(
                    "svg",
                    "image/svg+xml",
                    tree.to_string(&resvg::usvg::WriteOptions::default())
                        .into_bytes(),
                )),
            })
        }
    }
}

fn dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 32 * 1024 * 1024 {
        return Err("Publication image exceeds the 32 megapixel decoding limit".into());
    }
    Ok(())
}

fn part(extension: &str, kind: &str, bytes: Vec<u8>) -> Part {
    let digest = super::digest_hex(&Sha256::digest(&bytes));
    Part {
        path: format!("media/image-{}.{extension}", &digest[..32]),
        kind: kind.into(),
        bytes,
    }
}

fn check_svg(bytes: &[u8]) -> Result<bool, String> {
    let mut reader = Reader::from_reader(bytes);
    let mut root = false;
    let mut text = false;
    loop {
        match reader
            .read_event()
            .map_err(|error| format!("Invalid SVG XML: {error}"))?
        {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = tag.local_name();
                if !root {
                    if name.as_ref() != "svg" {
                        return Err(
                            "Publication asset is neither a supported raster image nor SVG".into(),
                        );
                    }
                    root = true;
                }
                if name.as_ref() == "text" {
                    text = true;
                }
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|error| error.to_string())?;
                    if attribute.key.local_name().as_ref() == "href" && name.as_ref() != "a" {
                        let value = attribute
                            .normalized_value(XmlVersion::Explicit1_0)
                            .map_err(|error| error.to_string())?;
                        if !value.starts_with('#') && !value.starts_with("data:") {
                            return Err("Publication SVG resources must be embedded; external file and network references are unsupported".into());
                        }
                    }
                }
            }
            Event::DocType(_) => {
                return Err(
                    "Publication SVG cannot contain a document type or external entities".into(),
                );
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root {
        return Err("Publication SVG has no root element".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_formats_preserve_geometry_and_convert_gif_and_webp_to_native_png() {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            4,
            image::Rgb([40, 100, 180]),
        ));
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            let mut encoded = Cursor::new(Vec::new());
            image.write_to(&mut encoded, format).unwrap();
            let bytes = encoded.into_inner();
            let asset = Asset::parse(bytes.clone(), None).unwrap();
            assert_eq!((asset.width, asset.height), (6.0, 3.0));
            assert!(asset.vector.is_none());
            if matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
                assert_eq!(asset.primary.bytes, bytes);
            } else {
                assert_eq!(asset.primary.kind, "image/png");
                let decoded = image::load_from_memory(&asset.primary.bytes).unwrap();
                assert_eq!((decoded.width(), decoded.height()), (8, 4));
                assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [40, 100, 180]);
            }
        }
    }

    #[test]
    fn vector_output_is_resolved_self_contained_and_has_a_decodable_raster_fallback() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40"><style>rect { fill: #2468ab }</style><rect width="80" height="40"/></svg>"#;
        let asset = Asset::parse(svg.to_vec(), None).unwrap();
        assert_eq!((asset.width, asset.height), (60.0, 30.0));
        let vector = String::from_utf8(asset.vector.unwrap().bytes).unwrap();
        assert!(!vector.contains("<style") && vector.contains("#2468ab"));
        let decoded = image::load_from_memory(&asset.primary.bytes).unwrap();
        assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [36, 104, 171]);
        assert!(
            Asset::parse(
                br#"<svg xmlns="http://www.w3.org/2000/svg"><text>Offline text</text></svg>"#
                    .to_vec(),
                None
            )
            .err()
            .unwrap()
            .contains("offline font")
        );
        assert!(
            Asset::parse(
                br#"<!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec(),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn svg_text_is_outlined_using_only_explicit_packaged_fonts() {
        let fonts = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("fonts/assets/pdf/noto-sans");
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30"><text x="2" y="20" font-family="Noto Sans">Bluink</text></svg>"#;
        let asset = Asset::parse(svg.to_vec(), Some(&fonts)).unwrap();
        let vector = String::from_utf8(asset.vector.unwrap().bytes).unwrap();
        assert!(vector.contains("<path") && !vector.contains("<text"));
        let raster = image::load_from_memory(&asset.primary.bytes)
            .unwrap()
            .to_rgba8();
        assert!(raster.pixels().any(|pixel| pixel.0[3] > 0));
    }
}
