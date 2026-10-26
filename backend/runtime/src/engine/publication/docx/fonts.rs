// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Consume complete build-time static faces; installed export only reads bytes.

use super::{
    R, W,
    package::{MAIN, Package},
    styles,
};
use crate::engine::{
    publication::theme_format::{ResolvedStyle, ResolvedTheme, Weight},
    xml::Xml,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};

const PART: &str = "word/fontTable.xml";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    generator: Generator,
    fonts: Vec<Typeface>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generator {
    name: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Typeface {
    id: String,
    family: String,
    docx_family: String,
    faces: Vec<Face>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Face {
    weight: u16,
    italic: bool,
    path: String,
    sha256: String,
    fs_type: u16,
    glyph_count: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    id: String,
    bold: bool,
    italic: bool,
}

pub(super) struct Fonts {
    folder: PathBuf,
    families: BTreeMap<String, Typeface>,
    used: BTreeSet<Key>,
}

impl Fonts {
    pub(super) fn load(folder: &Path, theme: &ResolvedTheme) -> Result<Self, String> {
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(folder.join("manifest.json"))
                .map_err(|error| format!("Cannot read packaged Word font manifest: {error}"))?,
        )
        .map_err(|error| format!("Invalid Word font manifest: {error}"))?;
        if manifest.schema_version != 1
            || manifest.generator.name.is_empty()
            || manifest.generator.version.is_empty()
        {
            return Err("Unsupported Word font manifest".into());
        }
        let mut families = BTreeMap::new();
        let mut names = BTreeSet::new();
        for family in manifest.fonts {
            if crate::engine::publication::fonts::family(&family.id)? != family.family
                || family.docx_family.len() > 31
                || !family.docx_family.is_ascii()
                || family.docx_family.is_empty()
                || !names.insert(family.docx_family.clone())
            {
                return Err("Invalid Word font family identity".into());
            }
            let mut faces = BTreeSet::new();
            for face in &family.faces {
                let style = if face.italic { "italic" } else { "normal" };
                if !matches!(face.weight, 400 | 700)
                    || !faces.insert((face.weight, face.italic))
                    || face.path
                        != format!("{}/{}-{}-{style}.ttf", family.id, family.id, face.weight)
                    || face.sha256.len() != 64
                    || !face.sha256.bytes().all(|value| value.is_ascii_hexdigit())
                {
                    return Err("Invalid Word font face identity".into());
                }
            }
            if faces.len() != 4 || families.insert(family.id.clone(), family).is_some() {
                return Err("Word fonts require four unique style-linked faces per family".into());
            }
        }
        for style in theme.styles.values() {
            if !families.contains_key(&style.font_family) {
                return Err(format!("Missing packaged Word font: {}", style.font_family));
            }
        }
        Ok(Self {
            folder: folder.to_owned(),
            families,
            used: BTreeSet::new(),
        })
    }

    pub(super) fn family(&self, id: &str) -> Result<&str, String> {
        self.families
            .get(id)
            .map(|family| family.docx_family.as_str())
            .ok_or_else(|| format!("Unknown Word font: {id}"))
    }

    pub(super) fn use_style(&mut self, style: &ResolvedStyle, strong: bool, emphasis: bool) {
        self.used.insert(Key {
            id: style.font_family.clone(),
            bold: strong || style.font_weight == Weight::Bold,
            italic: emphasis || style.italic,
        });
    }

    pub(super) fn install(self, package: &mut Package) -> Result<(), String> {
        let mut xml = Xml::new();
        xml.start("w:fonts", &[("xmlns:w", W.into()), ("xmlns:r", R.into())])?;
        for (id, family) in &self.families {
            let mut used: Vec<_> = self.used.iter().filter(|key| key.id == *id).collect();
            used.sort_by_key(|key| (key.italic, key.bold));
            if used.is_empty() {
                continue;
            }
            xml.start("w:font", &[("w:name", family.docx_family.clone())])?;
            xml.value("w:altName", &family.family)?;
            for key in used {
                let face = family
                    .faces
                    .iter()
                    .find(|face| {
                        face.weight == if key.bold { 700 } else { 400 } && face.italic == key.italic
                    })
                    .ok_or("Word face selection is missing")?;
                let mut bytes = Vec::new();
                std::fs::File::open(self.folder.join(&face.path))
                    .map_err(|error| format!("Cannot read Word font {}: {error}", face.path))?
                    .take(32 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if bytes.len() > 32 * 1024 * 1024 {
                    return Err("Packaged Word font exceeds the 32 MiB limit".into());
                }
                let digest = Sha256::digest(&bytes);
                if super::super::digest_hex(&digest) != face.sha256 {
                    return Err(format!(
                        "Packaged Word font checksum mismatch: {}",
                        face.path
                    ));
                }
                validate(&bytes, face, &family.docx_family)?;
                let mut guid = [0; 16];
                guid.copy_from_slice(&digest[..16]);
                guid[6] = (guid[6] & 15) | 64;
                guid[8] = (guid[8] & 63) | 128;
                let key_id = uuid::Uuid::from_bytes(guid);
                obfuscate(&mut bytes, &key_id)?;
                let path = format!("word/fonts/{}.odttf", &face.sha256[..32]);
                package.add(
                    &path,
                    "application/vnd.openxmlformats-officedocument.obfuscatedFont",
                    bytes,
                )?;
                let relationship = package.relate(PART, "font", &path)?;
                let kind = match (key.bold, key.italic) {
                    (false, false) => "w:embedRegular",
                    (true, false) => "w:embedBold",
                    (false, true) => "w:embedItalic",
                    (true, true) => "w:embedBoldItalic",
                };
                xml.empty(
                    kind,
                    &[
                        ("r:id", relationship),
                        (
                            "w:fontKey",
                            format!("{{{}}}", key_id.to_string().to_ascii_uppercase()),
                        ),
                        ("w:subsetted", "0".into()),
                    ],
                )?;
            }
            xml.end()?;
            // OFL notices accompany the complete font payloads in the document.
            let license = std::fs::read(self.folder.join(id).join("LICENSE"))
                .map_err(|error| format!("Missing Word font licence {id}: {error}"))?;
            package.add(
                &format!("word/fonts/{id}-LICENSE.txt"),
                "text/plain",
                license,
            )?;
        }
        xml.end()?;
        package.add(PART, styles::content_type("fontTable"), xml.finish()?)?;
        package.relate(MAIN, "fontTable", PART)?;
        Ok(())
    }
}

fn obfuscate(bytes: &mut [u8], guid: &uuid::Uuid) -> Result<(), String> {
    let prefix = bytes
        .get_mut(..32)
        .ok_or("Embedded font is shorter than 32 bytes")?;
    for (index, byte) in prefix.iter_mut().enumerate() {
        *byte ^= guid.as_bytes()[15 - index % 16];
    }
    Ok(())
}

fn number(bytes: &[u8], offset: usize) -> Result<u16, String> {
    Ok(u16::from_be_bytes(
        *bytes
            .get(offset..)
            .and_then(<[u8]>::first_chunk::<2>)
            .ok_or("Truncated font metric")?,
    ))
}
fn long(bytes: &[u8], offset: usize) -> Result<usize, String> {
    usize::try_from(u32::from_be_bytes(
        *bytes
            .get(offset..)
            .and_then(<[u8]>::first_chunk::<4>)
            .ok_or("Truncated font table entry")?,
    ))
    .map_err(|error| error.to_string())
}

fn tables(bytes: &[u8]) -> Result<BTreeMap<&[u8], &[u8]>, String> {
    if bytes.len() > 32 * 1024 * 1024 || bytes.get(..4) != Some(&[0, 1, 0, 0]) {
        return Err("Word embedding requires a bounded static TrueType face".into());
    }
    let count = usize::from(number(bytes, 4)?);
    if count == 0 || count > 256 {
        return Err("Invalid font table count".into());
    }
    let mut tables = BTreeMap::new();
    for index in 0..count {
        let record = 12 + index * 16;
        let tag = bytes
            .get(record..record + 4)
            .ok_or("Truncated font table directory")?;
        let offset = long(bytes, record + 8)?;
        let end = offset
            .checked_add(long(bytes, record + 12)?)
            .ok_or("Font table range overflow")?;
        let table = bytes
            .get(offset..end)
            .ok_or("Font table is outside the font file")?;
        if tables.insert(tag, table).is_some() {
            return Err("Duplicate font table".into());
        }
    }
    Ok(tables)
}

fn validate(bytes: &[u8], face: &Face, family: &str) -> Result<(), String> {
    let tables = tables(bytes)?;
    if tables.contains_key(b"fvar".as_slice()) || tables.contains_key(b"gvar".as_slice()) {
        return Err("Variable fonts must be prepared at build time before Word embedding".into());
    }
    let os2 = *tables
        .get(b"OS/2".as_slice())
        .ok_or("Font has no embedding permissions")?;
    let permissions = number(os2, 8)?;
    if !matches!(permissions & 15, 0 | 8) || permissions & 512 != 0 {
        return Err("Font does not permit editable outline embedding".into());
    }
    let selection = number(os2, 62)?;
    if permissions != face.fs_type
        || number(os2, 4)? != face.weight
        || (selection & 1 != 0) != face.italic
        || (selection & 32 != 0) != (face.weight == 700)
    {
        return Err("Word font face metrics differ from its manifest".into());
    }
    let maxp = *tables
        .get(b"maxp".as_slice())
        .ok_or("Font has no glyph count")?;
    if number(maxp, 4)? != face.glyph_count || face.glyph_count == 0 {
        return Err("Word font glyph count differs from its complete face manifest".into());
    }
    let name = *tables
        .get(b"name".as_slice())
        .ok_or("Font has no family name")?;
    let count = usize::from(number(name, 2)?);
    let strings = usize::from(number(name, 4)?);
    let mut matched = false;
    for index in 0..count {
        let record = 6 + index * 12;
        if number(name, record)? == 3 && number(name, record + 6)? == 1 {
            let start = strings + usize::from(number(name, record + 10)?);
            let end = start + usize::from(number(name, record + 8)?);
            let data = name
                .get(start..end)
                .ok_or("Font family name is outside its table")?;
            let (pairs, remainder) = data.as_chunks::<2>();
            if !remainder.is_empty() {
                return Err("Font family name has an incomplete UTF-16 unit".into());
            }
            let units = pairs
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect::<Vec<_>>();
            if String::from_utf16(&units).map_err(|error| error.to_string())? == family {
                matched = true;
            }
        }
    }
    if !matched {
        return Err("Word font primary family differs from its manifest".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{
        DocxDocument,
        tests::{font_folder, parts, prepared, tags, theme},
    };
    use super::*;
    use crate::engine::publication::{
        render::{self, Formatter},
        theme_format::Role,
    };
    use serde_json::json;

    #[test]
    fn font_numbers_reject_truncation_and_overflowing_offsets() {
        let bytes = [0, 1, 0, 2];
        assert_eq!(number(&bytes, 0).unwrap(), 1);
        assert_eq!(long(&bytes, 0).unwrap(), 65_538);
        for offset in [3, 4, usize::MAX - 1, usize::MAX] {
            assert!(number(&bytes, offset).is_err());
            assert!(long(&bytes, offset).is_err());
        }
    }

    fn source() -> serde_json::Value {
        json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"Regular Résumé 123 "},{"type":"text","text":"Bold Résumé 123 ","marks":[{"type":"strong"}]},{"type":"text","text":"Italic Résumé 123","marks":[{"type":"emphasis"}]}]},
            {"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"Bold italic Résumé 123","marks":[{"type":"strong"}]}]}]}
        ]})
    }

    #[test]
    fn obfuscation_reverses_guid_bytes_for_both_prefix_blocks_and_preserves_the_tail() {
        let guid = uuid::Uuid::parse_str("001B70DC-AA60-4AD5-90EC-18A0948E1EAE").unwrap();
        let mask = [
            0xae, 0x1e, 0x8e, 0x94, 0xa0, 0x18, 0xec, 0x90, 0xd5, 0x4a, 0x60, 0xaa, 0xdc, 0x70,
            0x1b, 0x00,
        ];
        let mut bytes = vec![0u8; 40];
        obfuscate(&mut bytes, &guid).unwrap();
        assert_eq!(&bytes[..16], &mask);
        assert_eq!(&bytes[16..32], &mask);
        assert_eq!(&bytes[32..], &[0; 8]);
        obfuscate(&mut bytes, &guid).unwrap();
        assert_eq!(bytes, vec![0; 40]);
        assert!(obfuscate(&mut [0; 31], &guid).is_err());
    }

    #[test]
    fn only_used_faces_are_embedded_and_each_deobfuscates_to_the_complete_packaged_face() {
        let mut theme = theme("classic");
        theme.styles.get_mut(&Role::BoxWarning).unwrap().font_family = "noto-sans".into();
        let catalogue = Fonts::load(&font_folder(), &theme).unwrap();
        let bundle = render::render(
            &prepared(&source(), "faces"),
            DocxDocument::new(&theme, "Faces", "en", &font_folder()).unwrap(),
        )
        .unwrap();
        let files = parts(&bundle.bytes);
        let font_tags = tags(&files[PART]);
        let embedded: Vec<_> = font_tags
            .iter()
            .filter(|(name, _)| name.starts_with("w:embed"))
            .collect();
        assert_eq!(embedded.len(), 4);
        assert_eq!(
            font_tags
                .iter()
                .filter(|(name, _)| name == "w:font")
                .count(),
            1
        );
        assert!(!String::from_utf8_lossy(&files[PART]).contains("Noto Sans"));
        let relationships = tags(&files["word/_rels/fontTable.xml.rels"]);
        for (_, attributes) in embedded {
            assert_eq!(attributes["w:subsetted"], "0");
            let target = &relationships
                .iter()
                .find(|(_, attrs)| attrs.get("Id") == attributes.get("r:id"))
                .unwrap()
                .1["Target"];
            let mut bytes = files[&format!("word/{target}")].clone();
            obfuscate(
                &mut bytes,
                &uuid::Uuid::parse_str(&attributes["w:fontKey"]).unwrap(),
            )
            .unwrap();
            let hash = super::super::super::digest_hex(&Sha256::digest(&bytes));
            let face = catalogue.families["source-serif-4"]
                .faces
                .iter()
                .find(|face| face.sha256 == hash)
                .unwrap();
            assert_eq!(
                bytes,
                std::fs::read(font_folder().join(&face.path)).unwrap()
            );
        }
        assert!(files.contains_key("word/fonts/source-serif-4-LICENSE.txt"));
    }

    #[test]
    fn static_validation_rejects_restricted_read_only_bitmap_and_malformed_faces() {
        let catalogue = Fonts::load(&font_folder(), &theme("classic")).unwrap();
        let family = &catalogue.families["source-serif-4"];
        let face = &family.faces[0];
        let bytes = std::fs::read(font_folder().join(&face.path)).unwrap();
        validate(&bytes, face, &family.docx_family).unwrap();
        let variable = std::fs::read(
            font_folder()
                .parent()
                .unwrap()
                .join("pdf/source-serif-4/source-serif-4-200-900-normal.ttf"),
        )
        .unwrap();
        assert!(
            validate(&variable, face, &family.docx_family)
                .unwrap_err()
                .contains("Variable fonts")
        );
        let offset = (0..usize::from(number(&bytes, 4).unwrap()))
            .map(|index| 12 + index * 16)
            .find(|index| &bytes[*index..*index + 4] == b"OS/2")
            .map(|index| long(&bytes, index + 8).unwrap())
            .unwrap();
        for permissions in [2u16, 4, 512, 0x204] {
            let mut altered = bytes.clone();
            altered[offset + 8..offset + 10].copy_from_slice(&permissions.to_be_bytes());
            assert!(
                validate(&altered, face, &family.docx_family)
                    .unwrap_err()
                    .contains("editable")
            );
        }
        let mut editable = bytes.clone();
        editable[offset + 8..offset + 10].copy_from_slice(&0x108u16.to_be_bytes());
        let mut metadata = face.clone();
        metadata.fs_type = 0x108;
        validate(&editable, &metadata, &family.docx_family).unwrap();
        assert!(validate(&bytes[..40], face, &family.docx_family).is_err());
        assert!(validate(&bytes, face, "Another family").is_err());
    }

    #[test]
    fn changed_packaged_font_bytes_fail_before_the_document_can_be_published() {
        let theme = theme("classic");
        let mut catalogue = Fonts::load(&font_folder(), &theme).unwrap();
        catalogue.use_style(&theme.styles[&Role::Body], false, false);
        let face = &catalogue.families["source-serif-4"].faces[0];
        let mut data = std::fs::read(font_folder().join(&face.path)).unwrap();
        data[40] ^= 1;
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(&face.path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, data).unwrap();
        catalogue.folder = temp.path().to_owned();
        assert!(
            catalogue
                .install(&mut Package::default())
                .unwrap_err()
                .contains("checksum mismatch")
        );
    }

    #[test]
    #[ignore = "Explicit developer command writes embedded-font proofs for all catalogue families."]
    fn write_embedded_font_proofs() {
        let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/embedded-font-proof");
        std::fs::create_dir_all(&output).unwrap();
        let base = theme("classic");
        let catalogue = Fonts::load(&font_folder(), &base).unwrap();
        for family in catalogue.families.values() {
            let mut theme = base.clone();
            for style in theme.styles.values_mut() {
                style.font_family = family.id.clone();
            }
            let mut writer =
                DocxDocument::new(&theme, &family.family, "en", &font_folder()).unwrap();
            render::walk(&prepared(&source(), &family.id), &mut writer).unwrap();
            std::fs::write(
                output.join(format!("{}.docx", family.id)),
                writer.finish().unwrap().bytes,
            )
            .unwrap();
        }
    }
}
