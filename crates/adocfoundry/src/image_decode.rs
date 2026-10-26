//! Reads and decodes an image file for the image viewer, within limits, on a background thread.
//! Raster formats go through the `image` crate, which GPUI already uses; SVG is drawn by
//! `resvg` with nothing loaded from outside the file: no linked images, local or remote, and no
//! scripts or links, which `resvg` never runs or follows. The content decides the format; the
//! extension only names what a file that fails to decode was meant to be.

use std::{
    fmt,
    io::Cursor,
    path::Path,
    sync::{Arc, OnceLock},
};

use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage};
use resvg::{tiny_skia, usvg};

/// The most an image may take: anything larger is refused rather than decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// The file's size on disk.
    pub max_file_bytes: u64,
    /// The width or height in pixels: no wider than a GPU texture may be.
    pub max_side: u32,
    /// Width times height: four bytes each once decoded.
    pub max_pixels: u64,
}

impl Limits {
    pub const DEFAULT: Self = Self {
        max_file_bytes: 256 * 1024 * 1024,
        max_side: 16_384,
        max_pixels: 64 * 1024 * 1024,
    };
}

/// What the viewer shows about an image.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    /// The format found in the content, such as `PNG`.
    pub format: &'static str,
    /// The decoded pixels' size, after any EXIF orientation.
    pub width: u32,
    pub height: u32,
    pub file_bytes: u64,
    /// For an SVG, its own size in CSS pixels; it was drawn at `width` by `height`.
    pub intrinsic: Option<(f32, f32)>,
    /// Whether any pixel is less than opaque, for the checkerboard behind it.
    pub has_alpha: bool,
}

/// A decoded image: its information and its pixels, in the BGRA order GPUI draws.
pub struct Decoded {
    pub info: ImageInfo,
    pub pixels: RgbaImage,
}

/// Why an image cannot be shown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// Missing, not permitted, or another failure to read it, as the system said.
    Unreadable(String),
    /// A format the viewer does not decode, named.
    Unsupported(String),
    /// Damaged, or not an image at all, with what the decoder said.
    Corrupt(String),
    /// Over the [`Limits`]: what is too large.
    TooLarge(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "The file cannot be read: {why}."),
            Self::Unsupported(format) => {
                write!(f, "AdocFoundry cannot show {format} images.")
            }
            Self::Corrupt(why) => {
                write!(f, "The file is damaged, or is not an image: {why}.")
            }
            Self::TooLarge(why) => write!(f, "The image is too large to show: {why}."),
        }
    }
}

/// Formats recognised by extension that the viewer does not decode.
const UNSUPPORTED: &[(&str, &str)] = &[("heic", "HEIC"), ("heif", "HEIF"), ("avif", "AVIF")];

/// Reads and decodes the image at `path`.
pub fn decode_file(path: &Path, limits: Limits) -> Result<Decoded, DecodeError> {
    let metadata = std::fs::metadata(path).map_err(|error| unreadable(&error))?;
    if metadata.len() > limits.max_file_bytes {
        return Err(DecodeError::TooLarge(format!(
            "the file is {}, over the {} limit",
            byte_size(metadata.len()),
            byte_size(limits.max_file_bytes)
        )));
    }
    let bytes = std::fs::read(path).map_err(|error| unreadable(&error))?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    decode(&bytes, &extension, limits)
}

fn unreadable(error: &std::io::Error) -> DecodeError {
    DecodeError::Unreadable(error.to_string())
}

/// Decodes `bytes`, from a file with the lower-case `extension`.
pub fn decode(bytes: &[u8], extension: &str, limits: Limits) -> Result<Decoded, DecodeError> {
    let file_bytes = bytes.len() as u64;
    if let Ok(format) = image::guess_format(bytes) {
        if let Some(name) = raster_name(format) {
            return decode_raster(bytes, format, name, file_bytes, limits);
        }
        return Err(DecodeError::Unsupported(
            format!("{format:?}").to_uppercase(),
        ));
    }
    if extension == "svg" {
        return decode_svg(bytes, file_bytes, limits);
    }
    if let Some((_, name)) = UNSUPPORTED.iter().find(|(known, _)| *known == extension) {
        return Err(DecodeError::Unsupported((*name).to_owned()));
    }
    Err(DecodeError::Corrupt(format!(
        "its content is not a {} image",
        if extension.is_empty() {
            "known".to_owned()
        } else {
            extension.to_uppercase()
        }
    )))
}

/// The name of a raster format the viewer decodes, or `None`.
fn raster_name(format: ImageFormat) -> Option<&'static str> {
    Some(match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        _ => return None,
    })
}

fn check_size(width: u32, height: u32, limits: Limits) -> Result<(), DecodeError> {
    if width == 0 || height == 0 {
        return Err(DecodeError::Corrupt("it has no pixels".to_owned()));
    }
    if width > limits.max_side || height > limits.max_side {
        return Err(DecodeError::TooLarge(format!(
            "{width} × {height} pixels, over the {} pixel limit on either side",
            limits.max_side
        )));
    }
    let pixels = u64::from(width).checked_mul(u64::from(height));
    if pixels.is_none_or(|pixels| pixels > limits.max_pixels) {
        return Err(DecodeError::TooLarge(format!(
            "{width} × {height} pixels, over the {} megapixel limit",
            limits.max_pixels / (1024 * 1024)
        )));
    }
    Ok(())
}

fn decode_raster(
    bytes: &[u8],
    format: ImageFormat,
    name: &'static str,
    file_bytes: u64,
    limits: Limits,
) -> Result<Decoded, DecodeError> {
    let corrupt = |error: image::ImageError| match error {
        image::ImageError::Limits(_) => DecodeError::TooLarge(error.to_string()),
        image::ImageError::Unsupported(_) => DecodeError::Unsupported(name.to_owned()),
        error => DecodeError::Corrupt(error.to_string()),
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    // The size is checked below, from the header, with a message that names it; the decoder
    // checks only what it allocates: room for the decoded image and its own buffers.
    let mut decoder_limits = image::Limits::no_limits();
    decoder_limits.max_alloc = Some(limits.max_pixels.saturating_mul(8));
    reader.limits(decoder_limits);
    let mut decoder = reader.into_decoder().map_err(corrupt)?;
    let (width, height) = decoder.dimensions();
    check_size(width, height, limits)?;
    // A damaged EXIF block is no reason not to show the picture.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    // Animated GIF and WebP files decode to their first frame.
    let mut image = DynamicImage::from_decoder(decoder).map_err(corrupt)?;
    image.apply_orientation(orientation);
    let mut pixels = image.into_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Decoded {
        info: ImageInfo {
            format: name,
            width: pixels.width(),
            height: pixels.height(),
            file_bytes,
            intrinsic: None,
            has_alpha: has_alpha(&pixels),
        },
        pixels,
    })
}

/// The system's fonts, for text in SVGs: read once, on the first SVG.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        })
        .clone()
}

/// The longer side, in pixels, an SVG is drawn at, so it stays sharp when zoomed in; less when
/// the limits say so.
const SVG_LONG_SIDE: f32 = 2048.0;

fn decode_svg(bytes: &[u8], file_bytes: u64, limits: Limits) -> Result<Decoded, DecodeError> {
    let options = usvg::Options {
        resources_dir: None,
        fontdb: fonts(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            // Never a file or an address: an SVG shows only what it holds.
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options)
        .map_err(|error| DecodeError::Corrupt(error.to_string()))?;
    let size = tree.size();
    let (intrinsic_width, intrinsic_height) = (size.width(), size.height());
    let long_side = intrinsic_width.max(intrinsic_height);
    let max_side = limits.max_side as f32;
    let max_area = limits.max_pixels as f32;
    let scale = (SVG_LONG_SIDE / long_side)
        .clamp(1.0, 16.0)
        .min(max_side / long_side)
        .min((max_area / (intrinsic_width * intrinsic_height)).sqrt());
    let width = (intrinsic_width * scale).round().max(1.0) as u32;
    let height = (intrinsic_height * scale).round().max(1.0) as u32;
    check_size(width, height, limits)?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| DecodeError::TooLarge(format!("{width} × {height} pixels")))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            width as f32 / intrinsic_width,
            height as f32 / intrinsic_height,
        ),
        &mut pixmap.as_mut(),
    );
    let mut pixels = RgbaImage::from_raw(width, height, pixmap.take())
        .ok_or_else(|| DecodeError::Corrupt("the drawing has the wrong size".to_owned()))?;
    // Premultiplied RGBA to straight BGRA.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            if let Some(straight) = (u16::from(*channel) * 255).checked_div(alpha) {
                *channel = straight.min(255) as u8;
            }
        }
    }
    Ok(Decoded {
        info: ImageInfo {
            format: "SVG",
            width,
            height,
            file_bytes,
            intrinsic: Some((intrinsic_width, intrinsic_height)),
            has_alpha: has_alpha(&pixels),
        },
        pixels,
    })
}

fn has_alpha(pixels: &RgbaImage) -> bool {
    pixels.pixels().any(|pixel| pixel.0[3] < u8::MAX)
}

/// A file size as people read it: `222 bytes`, `1.5 KB`, `3.2 MB`.
pub fn byte_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/image-viewer/modules/ROOT/images")
            .join(name)
    }

    fn decoded(name: &str) -> Decoded {
        decode_file(&fixture(name), Limits::DEFAULT)
            .unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    /// The pixel at `x`, `y` as RGBA.
    fn rgba(decoded: &Decoded, x: u32, y: u32) -> [u8; 4] {
        let [b, g, r, a] = decoded.pixels.get_pixel(x, y).0;
        [r, g, b, a]
    }

    #[test]
    fn the_supported_formats_decode_from_real_files() {
        for (name, format, width, height) in [
            ("logo.png", "PNG", 24, 16),
            ("Photo Café.PNG", "PNG", 24, 16),
            ("screenshot.jpg", "JPEG", 32, 20),
            ("still.gif", "GIF", 24, 16),
            ("still.webp", "WebP", 24, 16),
            ("bitmap.bmp", "BMP", 24, 16),
            ("icon.ico", "ICO", 24, 16),
            ("scan.tiff", "TIFF", 20, 12),
        ] {
            let decoded = decoded(name);
            assert_eq!(
                (decoded.info.format, decoded.info.width, decoded.info.height),
                (format, width, height),
                "{name}"
            );
            assert_eq!(decoded.pixels.dimensions(), (width, height), "{name}");
            assert_eq!(
                decoded.info.file_bytes,
                std::fs::metadata(fixture(name)).unwrap().len()
            );
        }
    }

    #[test]
    fn pixels_come_out_in_bgra_order_with_their_transparency() {
        let logo = decoded("logo.png");
        // The border is opaque red, the inside blue fading to transparent.
        assert_eq!(logo.pixels.get_pixel(0, 0).0, [30, 30, 200, 255]);
        assert_eq!(rgba(&logo, 1, 5), [30, 90, 200, 245]);
        assert!(rgba(&logo, 22, 5)[3] < 50);
    }

    #[test]
    fn only_images_with_transparent_pixels_say_so() {
        assert!(decoded("logo.png").info.has_alpha);
        assert!(decoded("architecture.svg").info.has_alpha);
        assert!(!decoded("screenshot.jpg").info.has_alpha);
    }

    #[test]
    fn a_jpeg_is_turned_as_its_exif_orientation_says() {
        let oriented = decoded("oriented.jpg");
        assert_eq!((oriented.info.width, oriented.info.height), (2, 4));
    }

    #[test]
    fn an_svg_is_drawn_sharp_at_its_own_proportions_and_loads_nothing() {
        let svg = decoded("architecture.svg");
        assert_eq!(svg.info.format, "SVG");
        assert_eq!(svg.info.intrinsic, Some((120.0, 60.0)));
        // Sixteen times its size, the most an SVG is enlarged to draw it.
        assert_eq!((svg.info.width, svg.info.height), (1920, 960));
        let at = |x: f32, y: f32| rgba(&svg, (x * 16.0) as u32, (y * 16.0) as u32);
        assert_eq!(at(30.0, 30.0), [0x1e, 0x5a, 0xc8, 255], "the square");
        assert_eq!(at(90.0, 30.0), [0xc8, 0x1e, 0x1e, 255], "the circle");
        // Where the linked images would be: neither the address nor the file loads.
        assert_eq!(at(60.0, 3.0)[3], 0, "no remote image");
        assert_eq!(at(60.0, 56.0)[3], 0, "no local image");
    }

    #[test]
    fn damaged_and_mislabelled_files_are_errors_not_blank_images() {
        assert!(matches!(
            decode_file(&fixture("broken.png"), Limits::DEFAULT),
            Err(DecodeError::Corrupt(_))
        ));
        let notes = decode_file(&fixture("notes.png"), Limits::DEFAULT)
            .err()
            .unwrap();
        assert_eq!(
            notes,
            DecodeError::Corrupt("its content is not a PNG image".to_owned())
        );
        assert!(matches!(
            decode(b"<svg", "svg", Limits::DEFAULT),
            Err(DecodeError::Corrupt(_))
        ));
    }

    #[test]
    fn formats_without_a_decoder_say_so() {
        assert_eq!(
            decode_file(&fixture("photo.heic"), Limits::DEFAULT).err(),
            Some(DecodeError::Unsupported("HEIC".to_owned()))
        );
        assert_eq!(
            DecodeError::Unsupported("HEIC".to_owned()).to_string(),
            "AdocFoundry cannot show HEIC images."
        );
    }

    #[test]
    fn a_missing_file_is_unreadable() {
        assert!(matches!(
            decode_file(&fixture("missing.png"), Limits::DEFAULT),
            Err(DecodeError::Unreadable(_))
        ));
    }

    #[test]
    fn images_over_the_limits_are_refused_before_decoding() {
        let small = |limits: Limits| decode_file(&fixture("logo.png"), limits).err();
        assert!(matches!(
            small(Limits {
                max_side: 20,
                ..Limits::DEFAULT
            }),
            Some(DecodeError::TooLarge(why)) if why.contains("24 × 16")
        ));
        assert!(matches!(
            small(Limits {
                max_pixels: 100,
                ..Limits::DEFAULT
            }),
            Some(DecodeError::TooLarge(_))
        ));
        assert!(matches!(
            small(Limits {
                max_file_bytes: 100,
                ..Limits::DEFAULT
            }),
            Some(DecodeError::TooLarge(why)) if why.contains("222 bytes")
        ));
        // An SVG is drawn smaller to fit, not refused.
        let svg = decode_file(
            &fixture("architecture.svg"),
            Limits {
                max_side: 600,
                ..Limits::DEFAULT
            },
        )
        .unwrap();
        assert_eq!((svg.info.width, svg.info.height), (600, 300));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(byte_size(222), "222 bytes");
        assert_eq!(byte_size(1536), "1.5 KB");
        assert_eq!(byte_size(3 * 1024 * 1024 + 200 * 1024), "3.2 MB");
    }
}
