//! The pictures of a book: read through the export's [`FileAccess`], told apart by their bytes
//! rather than their names, checked whole and within limits, and kept once each however often
//! they are shown. PNG, JPEG and GIF go into the book as they are; SVG, which neither format
//! takes, is drawn as a PNG in process with `resvg`, with nothing loaded from outside the file
//! (Kindle PRD §6.4; settles §15's "Where SVG is rasterised" as the DOCX exporter did).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_core::FileAccess;
use resvg::{tiny_skia, usvg};

use crate::book::{Asset, AssetKind};

/// The largest file a picture may come from.
pub(crate) const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
/// The most pixels on a side, and in all, a picture may have.
const MAX_SIDE: u32 = 10_000;
const MAX_PIXELS: u64 = 40_000_000;
/// The most bytes of pictures a book holds.
pub(crate) const MAX_BOOK_BYTES: usize = 256 * 1024 * 1024;
/// How much finer than its own size an SVG is drawn, so it stays sharp on a dense screen.
const SVG_SCALE: f32 = 2.0;

#[derive(Default)]
pub(crate) struct Media {
    pub(crate) assets: Vec<Asset>,
    by_path: HashMap<PathBuf, usize>,
    bytes: usize,
    /// The fonts SVG text is drawn with, read on the first SVG.
    fonts: Option<Arc<usvg::fontdb::Database>>,
}

impl Media {
    /// The asset for the picture at `path`, or why it cannot be shown, as the end of a
    /// sentence naming it.
    pub(crate) fn load(
        &mut self,
        path: &Path,
        access: &FileAccess,
        format: crate::Format,
    ) -> Result<usize, String> {
        if let Some(index) = self.by_path.get(path) {
            return Ok(*index);
        }
        if !access.permits(path) {
            return Err("cannot be read: it is outside the folders the export may read".to_owned());
        }
        let bytes = access
            .read(path)
            .map_err(|error| format!("cannot be read: {error}"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "is {} MB, more than {} MB",
                bytes.len() / 1_000_000,
                MAX_FILE_BYTES / 1_000_000
            ));
        }
        let asset = if looks_like_svg(&bytes) {
            self.svg(&bytes)?
        } else {
            let kind = kind_of(&bytes).ok_or_else(|| {
                format!(
                    "is not a picture {} shows (PNG, JPEG, GIF or SVG)",
                    format.reader()
                )
            })?;
            let (width, height) = raster_size(kind, &bytes)?;
            check_size(width, height)?;
            Asset {
                kind,
                bytes,
                width,
                height,
            }
        };
        if self.bytes + asset.bytes.len() > MAX_BOOK_BYTES {
            return Err(format!(
                "would take the book's pictures past {} MB",
                MAX_BOOK_BYTES / 1_000_000
            ));
        }
        self.bytes += asset.bytes.len();
        // The same bytes from another file are kept once.
        let index = match self
            .assets
            .iter()
            .position(|kept| kept.bytes == asset.bytes)
        {
            Some(index) => index,
            None => {
                self.assets.push(asset);
                self.assets.len() - 1
            }
        };
        self.by_path.insert(path.to_path_buf(), index);
        Ok(index)
    }

    /// An SVG drawn as a PNG, with the system's fonts and nothing loaded from outside it.
    fn svg(&mut self, bytes: &[u8]) -> Result<Asset, String> {
        let fonts = self
            .fonts
            .get_or_insert_with(|| {
                let mut fonts = usvg::fontdb::Database::new();
                fonts.load_system_fonts();
                Arc::new(fonts)
            })
            .clone();
        let options = usvg::Options {
            resources_dir: None,
            fontdb: fonts,
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
                // Never a file or an address: an SVG shows only what it holds.
                resolve_string: Box::new(|_, _| None),
            },
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(bytes, &options)
            .map_err(|_| "is not an SVG that can be drawn".to_owned())?;
        let size = tree.size();
        let scale = SVG_SCALE
            .min(MAX_SIDE as f32 / size.width().max(size.height()))
            .min((MAX_PIXELS as f32 / (size.width() * size.height())).sqrt());
        let width = (size.width() * scale).round().max(1.0) as u32;
        let height = (size.height() * scale).round().max(1.0) as u32;
        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| format!("cannot be drawn at {width} × {height} pixels"))?;
        // A white ground: a Kindle's page is light, but a transparent drawing on a dark theme
        // would be lost.
        pixmap.fill(tiny_skia::Color::WHITE);
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let png = pixmap
            .encode_png()
            .map_err(|error| format!("cannot be drawn: {error}"))?;
        Ok(Asset {
            kind: AssetKind::Png,
            bytes: png,
            width,
            height,
        })
    }
}

fn kind_of(bytes: &[u8]) -> Option<AssetKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(AssetKind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(AssetKind::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(AssetKind::Gif)
    } else {
        None
    }
}

/// Whether `bytes` begin, past any declaration, comments and white space, with an `<svg`
/// element.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let Ok(text) = std::str::from_utf8(head)
        .or_else(|error| std::str::from_utf8(&head[..error.valid_up_to()]))
    else {
        return false;
    };
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<?") {
            rest = after
                .split_once("?>")
                .map_or("", |(_, after)| after)
                .trim_start();
        } else if let Some(after) = rest.strip_prefix("<!--") {
            rest = after
                .split_once("-->")
                .map_or("", |(_, after)| after)
                .trim_start();
        } else if let Some(after) = rest.strip_prefix("<!DOCTYPE") {
            rest = after
                .split_once('>')
                .map_or("", |(_, after)| after)
                .trim_start();
        } else {
            return rest.starts_with("<svg");
        }
    }
}

/// The size a PNG, JPEG or GIF says it has, and whether it is whole.
fn raster_size(kind: AssetKind, bytes: &[u8]) -> Result<(u32, u32), &'static str> {
    const DAMAGED: &str = "is cut short or damaged";
    match kind {
        AssetKind::Png => {
            if bytes.len() < 33
                || &bytes[12..16] != b"IHDR"
                || !bytes.ends_with(b"IEND\xae\x42\x60\x82")
            {
                return Err(DAMAGED);
            }
            let read = |at: usize| {
                u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
            };
            Ok((read(16), read(20)))
        }
        AssetKind::Gif => {
            if bytes.len() < 14 || !bytes.ends_with(b";") {
                return Err(DAMAGED);
            }
            let read = |at: usize| u32::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
            Ok((read(6), read(8)))
        }
        AssetKind::Jpeg => {
            if !bytes.ends_with(&[0xFF, 0xD9]) {
                return Err(DAMAGED);
            }
            let mut at = 2;
            while at + 9 < bytes.len() {
                if bytes[at] != 0xFF {
                    return Err(DAMAGED);
                }
                let marker = bytes[at + 1];
                let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
                let frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
                if frame {
                    let height = u32::from(u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]));
                    let width = u32::from(u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]));
                    return Ok((width, height));
                }
                at += 2 + length;
            }
            Err(DAMAGED)
        }
    }
}

fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("has no size".to_owned());
    }
    if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "is {width} × {height} pixels where {MAX_SIDE} on a side is the most"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_told_by_their_bytes() {
        assert_eq!(kind_of(b"\x89PNG\r\n\x1a\nrest"), Some(AssetKind::Png));
        assert_eq!(kind_of(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(AssetKind::Jpeg));
        assert_eq!(kind_of(b"GIF89a..."), Some(AssetKind::Gif));
        assert!(looks_like_svg(
            b"<?xml version=\"1.0\"?>\n<!-- x -->\n<svg/>"
        ));
        assert!(!looks_like_svg(b"<html><svg/></html>"));
        assert_eq!(kind_of(b"RIFF....WEBP"), None);
    }

    #[test]
    fn a_jpeg_s_size_is_its_first_frame_s() {
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00,
            0x14, 0x00, 0x1E, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9,
        ];
        assert_eq!(raster_size(AssetKind::Jpeg, &jpeg), Ok((30, 20)));
        assert!(raster_size(AssetKind::Jpeg, &jpeg[..20]).is_err());
    }
}
