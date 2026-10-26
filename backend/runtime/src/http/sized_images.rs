// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Smaller copies of image assets for the browser, made on first request.
//!
//! The original stays the portable asset that publications use. A copy at one
//! of a few fixed widths is kept in a rebuildable cache beside the search
//! index, named after the original's size and modified time so that a changed
//! original gets a fresh copy.

use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

/// The widths the browser may ask for. A fixed set keeps the cache small.
pub(crate) const WIDTHS: [u32; 4] = [160, 320, 640, 1280];
/// Originals larger than this are served as they are rather than decoded.
/// Scans and photographs reach about 90 megapixels; decoding one briefly needs
/// up to four bytes a pixel. At most two HTTP resizes run at once.
const MAX_SOURCE_PIXELS: u64 = 120_000_000;
const JPEG_QUALITY: u8 = 82;
/// Keep CPU-heavy decoding off Tokio's blocking pool, which also serves RPCs.
static RESIZES: LazyLock<Result<rayon::ThreadPool, String>> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .thread_name(|index| format!("bluink-images-{index}"))
        .build()
        .map_err(|error| error.to_string())
});

pub(super) async fn prepare(
    original: PathBuf,
    cache: PathBuf,
    owner: String,
    width: u32,
) -> std::io::Result<Option<SizedCopy>> {
    static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    // Acquire before queuing work. Cancellation cannot release this permit
    // until the CPU worker has actually finished.
    let permit = ADMISSION.acquire().await.map_err(std::io::Error::other)?;
    let pool = RESIZES
        .as_ref()
        .map_err(|error| std::io::Error::other(error.clone()))?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    pool.spawn(move || {
        let _permit = permit;
        let _ = sender.send(sized_copy(&original, &cache, &owner, width));
    });
    receiver.await.map_err(std::io::Error::other)?
}

/// The width asked for by a `w=` query parameter, when it is one we make.
pub(crate) fn requested_width(query: Option<&str>) -> Option<u32> {
    let width = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("w="))?
        .parse::<u32>()
        .ok()?;
    WIDTHS.contains(&width).then_some(width)
}

/// A sized copy of an image asset.
pub(crate) struct SizedCopy {
    pub(crate) path: PathBuf,
    pub(crate) mime: &'static str,
    /// Whether this request made it, rather than finding it already made.
    pub(crate) made: bool,
}

/// A copy of `original` at most `width` pixels wide, made if it is missing.
/// `None` means the original should be served: its format is not resized,
/// it is no wider than asked, or it is too large to decode safely.
pub(crate) fn sized_copy(
    original: &Path,
    cache: &Path,
    owner: &str,
    width: u32,
) -> std::io::Result<Option<SizedCopy>> {
    let Some(name) = original.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    // Animated and vector images keep their original.
    let Ok(source_format @ (ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)) =
        ImageFormat::from_path(original)
    else {
        return Ok(None);
    };
    let metadata = fs::metadata(original)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let directory = cache.join(owner);
    // Every copy of this version of the original starts with `version`.
    let version = format!("{name}.{}-{modified}-", metadata.len());
    let stem = format!("{version}{width}");
    for (extension, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
        let path = directory.join(format!("{stem}.{extension}"));
        if path.is_file() {
            return Ok(Some(SizedCopy {
                path,
                mime,
                made: false,
            }));
        }
    }
    let (source_width, source_height) = reader(original, source_format)?
        .into_dimensions()
        .map_err(std::io::Error::other)?;
    if source_width <= width
        || u64::from(source_width) * u64::from(source_height) > MAX_SOURCE_PIXELS
    {
        return Ok(None);
    }
    let image = reader(original, source_format)?
        .decode()
        .map_err(std::io::Error::other)?;
    let resized = image.thumbnail(width, u32::MAX);
    // Photographs become JPEG; images with transparency stay PNG.
    let (extension, mime, encoded) = if resized.color().has_alpha() {
        ("png", "image/png", encode_png(&resized)?)
    } else {
        ("jpg", "image/jpeg", encode_jpeg(&resized)?)
    };
    fs::create_dir_all(&directory)?;
    remove_stale(&directory, name, &version);
    let path = directory.join(format!("{stem}.{extension}"));
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    std::io::Write::write_all(&mut temporary, &encoded)?;
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(Some(SizedCopy {
        path,
        mime,
        made: true,
    }))
}

fn reader(
    path: &Path,
    format: ImageFormat,
) -> std::io::Result<ImageReader<std::io::BufReader<fs::File>>> {
    let mut reader = ImageReader::open(path)?;
    reader.set_format(format);
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_SOURCE_PIXELS * 4);
    reader.limits(limits);
    Ok(reader)
}

fn encode_jpeg(image: &DynamicImage) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
    image
        .to_rgb8()
        .write_with_encoder(encoder)
        .map_err(std::io::Error::other)?;
    Ok(bytes)
}

fn encode_png(image: &DynamicImage) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(std::io::Error::other)?;
    Ok(bytes)
}

/// Copies made from an earlier version of `name` can no longer be reached.
fn remove_stale(directory: &Path, name: &str, version: &str) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        // `{name}.{size}-{modified}-{width}.{extension}`
        let Some(copy) = file
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        let Some((stem, _extension)) = copy.rsplit_once('.') else {
            continue;
        };
        let numbers = stem.split('-').collect::<Vec<_>>();
        let is_copy = numbers.len() == 3 && numbers.iter().all(|part| part.parse::<u128>().is_ok());
        if is_copy && !file.starts_with(version) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(path: &Path, width: u32, height: u32, alpha: bool) {
        let image = if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([10, 20, 30, 128]),
            ))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                width,
                height,
                image::Rgb([10, 20, 30]),
            ))
        };
        image.save(path).unwrap();
    }

    #[test]
    fn only_listed_widths_are_made() {
        assert_eq!(requested_width(Some("w=320")), Some(320));
        assert_eq!(requested_width(Some("v=2&w=1280")), Some(1280));
        assert_eq!(requested_width(Some("w=321")), None);
        assert_eq!(requested_width(Some("w=-1")), None);
        assert_eq!(requested_width(None), None);
    }

    #[test]
    fn resizing_does_not_wait_for_the_rpc_blocking_pool() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("cover.png");
        picture(&original, 400, 200, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, waiting) = std::sync::mpsc::channel();
        let worker = runtime.spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = blocked.recv();
        });
        waiting.recv().unwrap();
        let result = runtime.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                prepare(original, root.path().join("cache"), "owner".into(), 160),
            )
            .await
        });
        release.send(()).unwrap();
        runtime.block_on(worker).unwrap();
        assert!(result.unwrap().unwrap().unwrap().made);
    }

    fn copy(original: &Path, cache: &Path, width: u32) -> SizedCopy {
        sized_copy(original, cache, "owner", width)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn makes_a_copy_once_and_keeps_the_original() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("cover.jpg");
        picture(&original, 900, 600, false);
        let before = fs::read(&original).unwrap();
        let cache = root.path().join("cache");
        let first = copy(&original, &cache, 320);
        assert!(first.made);
        assert_eq!(first.mime, "image/jpeg");
        assert_eq!(image::image_dimensions(&first.path).unwrap(), (320, 213));
        assert_eq!(fs::read(&original).unwrap(), before);
        // A second request reuses the copy.
        let modified = fs::metadata(&first.path).unwrap().modified().unwrap();
        let again = copy(&original, &cache, 320);
        assert!(!again.made);
        assert_eq!(again.path, first.path);
        assert_eq!(
            fs::metadata(&again.path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn transparency_stays_png_and_small_or_unsupported_images_are_served_as_they_are() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let clear = root.path().join("clear.png");
        picture(&clear, 800, 800, true);
        let SizedCopy { path, mime, .. } =
            sized_copy(&clear, &cache, "owner", 160).unwrap().unwrap();
        assert_eq!(
            (mime, image::image_dimensions(&path).unwrap()),
            ("image/png", (160, 160))
        );
        let small = root.path().join("small.png");
        picture(&small, 120, 80, false);
        assert!(sized_copy(&small, &cache, "owner", 160).unwrap().is_none());
        let vector = root.path().join("diagram.svg");
        fs::write(&vector, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        assert!(sized_copy(&vector, &cache, "owner", 160).unwrap().is_none());
    }

    #[test]
    fn a_changed_original_gets_a_fresh_copy_and_the_old_one_goes() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let original = root.path().join("cover.png");
        picture(&original, 1000, 500, false);
        let first = copy(&original, &cache, 640).path;
        let other_width = copy(&original, &cache, 320).path;
        std::thread::sleep(std::time::Duration::from_millis(20));
        picture(&original, 1000, 1000, false);
        let second = copy(&original, &cache, 640).path;
        assert_ne!(first, second);
        assert_eq!(image::image_dimensions(&second).unwrap(), (640, 640));
        assert!(!first.exists());
        assert!(
            !other_width.exists(),
            "every copy of the old version is removed"
        );
    }
}
