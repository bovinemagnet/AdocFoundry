//! WP 4.7: visual regression against asciidoctor-pdf. Each reference document is rendered by
//! `asciidoctor-pdf` and by `adocfoundry-cli pdf`, both PDFs are rasterised with `hayro`, and
//! every page is compared: the page counts must agree, the blurred pages must be close, and the
//! ink must sit in the same place. Skipped, with a message, when `asciidoctor-pdf` is not on
//! the `PATH`. Failing pages are written as PNG pairs under `target/pdf-regression/`.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use hayro::{hayro_interpret::InterpreterSettings, hayro_syntax::Pdf};

/// Rasterisation: 40 dpi.
const SCALE: f32 = 40.0 / 72.0;
/// Box blur radius in pixels, about 4pt: small shifts of a glyph or a line do not count.
const BLUR: usize = 2;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn asciidoctor_pdf_available() -> bool {
    Command::new("asciidoctor-pdf")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// A page as grey levels, 0 (ink) to 255 (paper), in rows.
struct Raster {
    width: usize,
    height: usize,
    grey: Vec<u8>,
}

impl Raster {
    fn blurred(&self) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut sum, mut count) = (0.0, 0.0);
                for dy in y.saturating_sub(BLUR)..=(y + BLUR).min(h - 1) {
                    for dx in x.saturating_sub(BLUR)..=(x + BLUR).min(w - 1) {
                        sum += f32::from(self.grey[dy * w + dx]);
                        count += 1.0;
                    }
                }
                out[y * w + x] = sum / count;
            }
        }
        out
    }

    /// The bounding box of the ink, in points: left, top, right, bottom.
    fn ink(&self) -> Option<[f32; 4]> {
        let mut bounds: Option<[usize; 4]> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.grey[y * self.width + x] < 200 {
                    let b = bounds.get_or_insert([x, y, x, y]);
                    b[0] = b[0].min(x);
                    b[1] = b[1].min(y);
                    b[2] = b[2].max(x);
                    b[3] = b[3].max(y);
                }
            }
        }
        bounds.map(|b| b.map(|v| v as f32 / SCALE))
    }

    fn png(&self) -> Vec<u8> {
        // A minimal greyscale PNG: stored (uncompressed) deflate blocks.
        fn crc(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffff_u32;
            for byte in data {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc(&body).to_be_bytes());
        }
        let mut raw = Vec::new();
        for row in self.grey.chunks(self.width) {
            raw.push(0);
            raw.extend(row);
        }
        let mut zlib = vec![0x78, 0x01];
        for (index, block) in raw.chunks(65_535).enumerate() {
            let last = (index + 1) * 65_535 >= raw.len();
            zlib.push(u8::from(last));
            let len = block.len() as u16;
            zlib.extend(len.to_le_bytes());
            zlib.extend((!len).to_le_bytes());
            zlib.extend(block);
        }
        let (mut a, mut b) = (1_u32, 0_u32);
        for byte in &raw {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend(((b << 16) | a).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend((self.width as u32).to_be_bytes());
        header.extend((self.height as u32).to_be_bytes());
        header.extend([8, 0, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }
}

fn rasterise(pdf: &Path) -> Vec<Raster> {
    let data = std::fs::read(pdf).unwrap();
    let pdf =
        Pdf::new(Arc::new(data)).unwrap_or_else(|error| panic!("{}: {error:?}", pdf.display()));
    let pixmaps = hayro::render_pdf(&pdf, SCALE, InterpreterSettings::default(), None).unwrap();
    pixmaps
        .into_iter()
        .map(|pixmap| {
            let (width, height) = (usize::from(pixmap.width()), usize::from(pixmap.height()));
            let grey = pixmap
                .data_as_u8_slice()
                .chunks(4)
                .map(|rgba| {
                    ((u16::from(rgba[0]) + u16::from(rgba[1]) + u16::from(rgba[2])) / 3) as u8
                })
                .collect();
            Raster {
                width,
                height,
                grey,
            }
        })
        .collect()
}

/// How far apart two pages are: the mean grey difference of the blurred pages, 0 to 1, and the
/// largest difference between the edges of their ink, in points.
fn distance(ours: &Raster, theirs: &Raster) -> (f32, f32) {
    let (a, b) = (ours.blurred(), theirs.blurred());
    let w = ours.width.min(theirs.width);
    let h = ours.height.min(theirs.height);
    let mut total = 0.0;
    for y in 0..h {
        for x in 0..w {
            total += (a[y * ours.width + x] - b[y * theirs.width + x]).abs();
        }
    }
    let mean = total / (w * h) as f32 / 255.0;
    let edges = match (ours.ink(), theirs.ink()) {
        (Some(a), Some(b)) => a
            .iter()
            .zip(b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max),
        (None, None) => 0.0,
        _ => f32::INFINITY,
    };
    (mean, edges)
}

struct Reference {
    name: &'static str,
    source: &'static str,
    /// Extra `asciidoctor-pdf` arguments, and the matching `adocfoundry-cli pdf` ones.
    asciidoctor: Vec<String>,
    ours: Vec<String>,
    /// Content placement must agree to within this many points; the first passing run was within
    /// 1.8pt, one pixel at 40 dpi.
    max_edge: f32,
}

/// The largest mean grey difference a page may have. Set from the first passing run, where no
/// reference page differed by more than 0.0085.
const MAX_MEAN: f32 = 0.02;

fn references() -> Vec<Reference> {
    let themes = repository_root().join("tests/fixtures/pdf/themes");
    let themes = themes.to_string_lossy().into_owned();
    vec![
        Reference {
            name: "article",
            source: "tests/fixtures/pdf/reference/article.adoc",
            asciidoctor: Vec::new(),
            ours: Vec::new(),
            max_edge: 6.0,
        },
        Reference {
            name: "custom-theme",
            source: "tests/fixtures/pdf/reference/article.adoc",
            asciidoctor: vec![
                "-a".into(),
                "title-page".into(),
                "-a".into(),
                format!("pdf-themesdir={themes}"),
                "-a".into(),
                "pdf-theme=acme".into(),
                "-a".into(),
                format!("pdf-fontsdir={themes};GEM_FONTS_DIR"),
            ],
            ours: vec![
                "--theme".into(),
                format!("{themes}/acme-theme.yml"),
                "--fonts-dir".into(),
                themes.clone(),
            ],
            max_edge: 6.0,
        },
    ]
}

#[test]
fn pages_match_asciidoctor_pdf() {
    if !asciidoctor_pdf_available() {
        eprintln!("skipped: no working `asciidoctor-pdf` on the PATH");
        return;
    }
    let out = repository_root().join("target/pdf-regression");
    std::fs::create_dir_all(&out).unwrap();
    let mut failures = Vec::new();
    for reference in references() {
        let theirs = out.join(format!("{}-asciidoctor-pdf.pdf", reference.name));
        let ours = out.join(format!("{}-adocfoundry.pdf", reference.name));
        let source = repository_root().join(reference.source);
        // `title-page` is an attribute for asciidoctor-pdf; ours reads it from the document, so
        // it exports a copy that sets it, beside a copy of the images.
        let copy = tempfile::tempdir().unwrap();
        let our_source = if reference.asciidoctor.iter().any(|a| a == "title-page") {
            let images = copy.path().join("images");
            std::fs::create_dir_all(&images).unwrap();
            for entry in std::fs::read_dir(source.parent().unwrap().join("images")).unwrap() {
                let entry = entry.unwrap();
                std::fs::copy(entry.path(), images.join(entry.file_name())).unwrap();
            }
            let path = copy.path().join(source.file_name().unwrap());
            let text = std::fs::read_to_string(&source).unwrap().replacen(
                ":toc:",
                ":toc:\n:title-page:",
                1,
            );
            std::fs::write(&path, text).unwrap();
            path
        } else {
            source.clone()
        };
        let status = Command::new("asciidoctor-pdf")
            .current_dir(source.parent().unwrap())
            .args(&reference.asciidoctor)
            .arg(&source)
            .arg("-o")
            .arg(&theirs)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let status = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .arg("pdf")
            .arg(&our_source)
            .args(&reference.ours)
            .arg("--output")
            .arg(&ours)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );

        let (theirs, ours) = (rasterise(&theirs), rasterise(&ours));
        if theirs.len() != ours.len() {
            failures.push(format!(
                "{}: {} pages from asciidoctor-pdf, {} here",
                reference.name,
                theirs.len(),
                ours.len()
            ));
        }
        for (index, (a, b)) in ours.iter().zip(&theirs).enumerate() {
            let (mean, edges) = distance(a, b);
            eprintln!(
                "{} page {}: mean difference {mean:.4}, ink edges within {edges:.1}pt",
                reference.name,
                index + 1
            );
            if mean > MAX_MEAN || edges > reference.max_edge {
                let page = index + 1;
                std::fs::write(
                    out.join(format!("{}-{page}-adocfoundry.png", reference.name)),
                    a.png(),
                )
                .unwrap();
                std::fs::write(
                    out.join(format!("{}-{page}-asciidoctor-pdf.png", reference.name)),
                    b.png(),
                )
                .unwrap();
                failures.push(format!(
                    "{} page {page}: mean difference {mean:.4} (at most {MAX_MEAN}), ink edges {edges:.1}pt apart (at most {})",
                    reference.name, reference.max_edge
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
