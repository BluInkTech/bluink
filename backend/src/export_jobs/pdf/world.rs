// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Typst, linked into Bluink. [`compile`] typesets one staged PDF bundle with
//! only Bluink's packaged fonts. Exports run it in a child process
//! ([`super::Renderer`]), because Typst cannot stop a compile midway.

use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use chrono::{Datelike, Timelike};
use typst::{
    Feature, Library, LibraryExt, World, WorldExt,
    diag::{FileError, FileResult, Severity, SourceDiagnostic, Warned},
    foundations::{Bytes, Datetime, Duration, Smart},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook, FontInfo},
    utils::LazyHash,
};
use typst_layout::PagedDocument;
use typst_pdf::{PdfOptions, PdfStandard, PdfStandards, Timestamp};

use crate::domain::publication::PdfProfile;

/// A font file read for its metadata; its data loads on first use.
struct FontSlot {
    path: PathBuf,
    index: u32,
    font: OnceLock<Option<Font>>,
}

/// One staged bundle. Files resolve only inside `root`: there are no packages,
/// system fonts or network.
struct Bundle {
    root: PathBuf,
    main: FileId,
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
    sources: Mutex<HashMap<FileId, Source>>,
    files: Mutex<HashMap<FileId, Bytes>>,
    now: chrono::DateTime<chrono::Utc>,
}

/// Typesets `main`, a path inside `root`, and returns the PDF. An error holds
/// Typst's diagnostics, one `path:line:column: error: message` per line.
pub(super) fn compile(
    root: &Path,
    main: &str,
    font_folder: &Path,
    profile: PdfProfile,
) -> Result<Vec<u8>, String> {
    let (world, document) = typeset(root, main, font_folder)?;
    let standards = match profile {
        PdfProfile::Default => PdfStandards::default(),
        PdfProfile::Accessible => PdfStandards::new(&[PdfStandard::Ua_1])
            .map_err(|error| format!("error: {}", error.message()))?,
    };
    let options = PdfOptions {
        ident: Smart::Auto,
        timestamp: world.timestamp(),
        standards,
        ..PdfOptions::default()
    };
    typst_pdf::pdf(&document, &options).map_err(|errors| world.describe(&errors))
}

fn typeset(root: &Path, main: &str, font_folder: &Path) -> Result<(Bundle, PagedDocument), String> {
    let world = Bundle::new(root, main, font_folder)?;
    let Warned { output, .. } = typst::compile::<PagedDocument>(&world);
    let document = output.map_err(|errors| world.describe(&errors))?;
    Ok((world, document))
}

/// One field of every `table` or `metadata` element in the typeset document,
/// as `typst query <element> --field <field>` reports it.
#[cfg(test)]
pub(crate) fn query(
    root: &Path,
    main: &str,
    font_folder: &Path,
    element: &str,
    field: &str,
) -> Result<Vec<serde_json::Value>, String> {
    use typst::{
        foundations::{Element, Selector},
        introspection::{Introspector, MetadataElem},
        model::TableElem,
    };
    let element = match element {
        "table" => Element::of::<TableElem>(),
        "metadata" => Element::of::<MetadataElem>(),
        other => return Err(format!("No query for {other} elements.")),
    };
    let (_, document) = typeset(root, main, font_folder)?;
    document
        .introspector()
        .query(&Selector::Elem(element, None))
        .iter()
        .map(|found| {
            let value = found
                .field_by_name(field)
                .map_err(|error| error.to_string())?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        })
        .collect()
}

/// The PDF's creation time. Reproducible export proofs set the conventional
/// `SOURCE_DATE_EPOCH`, as the Typst CLI read it; the child inherits it.
fn generated_at() -> Result<chrono::DateTime<chrono::Utc>, String> {
    match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value
            .parse()
            .ok()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .ok_or_else(|| "Invalid SOURCE_DATE_EPOCH".to_owned()),
        Err(std::env::VarError::NotPresent) => Ok(chrono::Utc::now()),
        Err(_) => Err("Invalid SOURCE_DATE_EPOCH".to_owned()),
    }
}

/// Every font in the folder, recursively and in path order, so a build sees
/// the same book on every platform.
fn load_fonts(folder: &Path) -> Result<(FontBook, Vec<FontSlot>), String> {
    let mut files = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("Cannot read packaged fonts: {error}"))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("Cannot read packaged fonts: {error}"))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    ["ttf", "otf", "ttc", "otc"]
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                })
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut book = FontBook::new();
    let mut fonts = Vec::new();
    for path in files {
        let data =
            fs::read(&path).map_err(|error| format!("Cannot read packaged font: {error}"))?;
        // A collection holds several faces; a single font file holds one.
        for index in 0.. {
            let Some(info) = FontInfo::new(&data, index) else {
                break;
            };
            book.push(info);
            fonts.push(FontSlot {
                path: path.clone(),
                index,
                font: OnceLock::new(),
            });
        }
    }
    Ok((book, fonts))
}

impl Bundle {
    fn new(root: &Path, main: &str, font_folder: &Path) -> Result<Self, String> {
        let main = VirtualPath::new(main)
            .map(|path| RootedPath::new(VirtualRoot::Project, path).intern())
            .map_err(|error| format!("PDF bundle shell path is invalid: {error}"))?;
        let (book, fonts) = load_fonts(font_folder)?;
        Ok(Self {
            root: root.to_path_buf(),
            main,
            library: LazyHash::new(
                Library::builder()
                    .with_features(std::iter::once(Feature::A11yExtras).collect())
                    .build(),
            ),
            book: LazyHash::new(book),
            fonts,
            sources: Mutex::default(),
            files: Mutex::default(),
            now: generated_at()?,
        })
    }

    fn path(id: FileId, root: &Path) -> FileResult<PathBuf> {
        let rooted = id.get();
        if *rooted.root() != VirtualRoot::Project {
            return Err(FileError::Other(Some(
                "Typst packages are not available in Bluink exports.".into(),
            )));
        }
        rooted.vpath().realize(root).map_err(FileError::Realize)
    }

    fn read(&self, id: FileId) -> FileResult<Bytes> {
        let cached = |files: &HashMap<FileId, Bytes>| files.get(&id).cloned();
        let found = self.files.lock().ok().as_deref().and_then(cached);
        if let Some(bytes) = found {
            return Ok(bytes);
        }
        let path = Self::path(id, &self.root)?;
        if path.is_dir() {
            return Err(FileError::IsDirectory);
        }
        let bytes = Bytes::new(fs::read(&path).map_err(|error| FileError::from_io(error, &path))?);
        if let Ok(mut files) = self.files.lock() {
            files.insert(id, bytes.clone());
        }
        Ok(bytes)
    }

    fn timestamp(&self) -> Option<Timestamp> {
        let now = self.now;
        Datetime::from_ymd_hms(
            now.year(),
            u8::try_from(now.month()).ok()?,
            u8::try_from(now.day()).ok()?,
            u8::try_from(now.hour()).ok()?,
            u8::try_from(now.minute()).ok()?,
            u8::try_from(now.second()).ok()?,
        )
        .map(Timestamp::new_utc)
    }

    /// Errors in Typst's short format, which `source_for_diagnostic` reads.
    fn describe(&self, diagnostics: &[SourceDiagnostic]) -> String {
        let mut text = String::new();
        for diagnostic in diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
        {
            if let Some(id) = diagnostic.span.id() {
                let _ = write!(text, "{}", id.get().vpath().get_without_slash());
                let position = self.range(diagnostic.span).and_then(|range| {
                    self.source(id)
                        .ok()?
                        .lines()
                        .byte_to_line_column(range.start)
                });
                if let Some((line, column)) = position {
                    let _ = write!(text, ":{}:{}", line + 1, column + 1);
                }
                text.push_str(": ");
            }
            let _ = writeln!(text, "error: {}", diagnostic.message);
            for hint in &diagnostic.hints {
                let _ = writeln!(text, "hint: {}", hint.v);
            }
        }
        text
    }
}

impl World for Bundle {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        let cached = |sources: &HashMap<FileId, Source>| sources.get(&id).cloned();
        let found = self.sources.lock().ok().as_deref().and_then(cached);
        if let Some(source) = found {
            return Ok(source);
        }
        let bytes = self.read(id)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| FileError::InvalidUtf8)?;
        let source = Source::new(id, text.trim_start_matches('\u{feff}').to_owned());
        if let Ok(mut sources) = self.sources.lock() {
            sources.insert(id, source.clone());
        }
        Ok(source)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.read(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        let slot = self.fonts.get(index)?;
        slot.font
            .get_or_init(|| {
                let data = fs::read(&slot.path).ok()?;
                Font::new(Bytes::new(data), slot.index)
            })
            .clone()
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let date = match offset {
            None => self.now.with_timezone(&chrono::Local).date_naive(),
            Some(offset) => {
                let [weeks, days, hours, minutes, seconds] = offset.decompose();
                let offset = chrono::Duration::try_weeks(weeks)?
                    + chrono::Duration::try_days(days)?
                    + chrono::Duration::try_hours(hours)?
                    + chrono::Duration::try_minutes(minutes)?
                    + chrono::Duration::try_seconds(seconds)?;
                (self.now + offset).date_naive()
            }
        };
        Datetime::from_ymd(
            date.year(),
            u8::try_from(date.month()).ok()?,
            u8::try_from(date.day()).ok()?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fonts/assets/pdf")
    }

    #[test]
    fn compiles_with_packaged_fonts_only_and_reports_positions() {
        let stage = tempfile::tempdir().unwrap();
        std::fs::write(
            stage.path().join("main.typ"),
            "#set document(title: \"Proof\")\n#set text(font: \"Inter\")\nHello #datetime.today().year()",
        )
        .unwrap();
        for profile in [PdfProfile::Default, PdfProfile::Accessible] {
            let pdf = compile(stage.path(), "main.typ", &fonts(), profile).unwrap();
            assert!(pdf.starts_with(b"%PDF-"));
        }
        std::fs::write(stage.path().join("main.typ"), "First\n#undefined-thing").unwrap();
        let error = compile(stage.path(), "main.typ", &fonts(), PdfProfile::Default).unwrap_err();
        assert!(
            error.starts_with("main.typ:2:2: error: unknown variable"),
            "{error}"
        );
        // Only packaged families: no system fonts and none of Typst's own.
        let (book, _) = load_fonts(&fonts()).unwrap();
        assert!(book.select_family("inter").next().is_some());
        for family in ["arial", "libertinus serif", "dejavu sans mono"] {
            assert!(book.select_family(family).next().is_none(), "{family}");
        }
    }

    #[test]
    fn every_catalogue_family_renders_bold_and_italic_without_warnings() {
        let catalogue: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../../fonts/catalogue.json")).unwrap();
        let source = catalogue
            .iter()
            .map(|font| {
                let family = serde_json::to_string(&font["family"]).unwrap();
                format!("#text(font: {family})[{family}: Résumé 123 #strong[Bold] #emph[Italic]]")
            })
            .collect::<Vec<_>>()
            .join(
                "

",
            );
        let stage = tempfile::tempdir().unwrap();
        std::fs::write(stage.path().join("main.typ"), source).unwrap();
        let world = Bundle::new(stage.path(), "main.typ", &fonts()).unwrap();
        let Warned { output, warnings } = typst::compile::<PagedDocument>(&world);
        let messages: Vec<_> = warnings.iter().map(|warning| &warning.message).collect();
        assert!(messages.is_empty(), "{messages:?}");
        output.unwrap();
        for font in &catalogue {
            let family = font["family"].as_str().unwrap().to_lowercase();
            assert!(
                world.book.select_family(&family).next().is_some(),
                "{family}"
            );
        }
    }

    #[test]
    fn files_resolve_only_inside_the_bundle() {
        let outer = tempfile::tempdir().unwrap();
        let stage = outer.path().join("stage");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(outer.path().join("secret.typ"), "secret").unwrap();
        for source in [
            "#include \"../secret.typ\"",
            "#import \"@preview/example:0.1.0\": *",
        ] {
            std::fs::write(stage.join("main.typ"), source).unwrap();
            let error = compile(&stage, "main.typ", &fonts(), PdfProfile::Default).unwrap_err();
            assert!(error.starts_with("main.typ:1:"), "{error}");
        }
    }
}
