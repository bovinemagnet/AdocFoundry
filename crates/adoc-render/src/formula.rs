//! A typeset inline formula's image and its measures. The baseline travels inside the SVG, as an
//! attribute of its root element, so a formula read back from the diagram cache still knows
//! where it sits on the line.

use std::{
    collections::HashMap,
    io::Read as _,
    path::{Path, PathBuf},
};

use crate::{ViewBlock, ViewInline, ViewKind, ViewTree};

/// The size of type an inline formula is rendered at, in points.
pub const INLINE_FORMULA_POINTS: f32 = 13.0;

/// The root element's attribute holding the baseline, in points from the top.
const BASELINE: &str = "data-baseline";

/// How much of the file is read to find the root element.
const HEAD: u64 = 4096;

/// An inline formula typeset as an image, with its measures in points.
#[derive(Clone, Debug, PartialEq)]
pub struct FormulaImage {
    pub path: PathBuf,
    pub width: f32,
    pub height: f32,
    /// From the top of the image.
    pub baseline: f32,
}

/// `svg` with `baseline` (points from the top) recorded on its root element.
#[must_use]
pub fn with_baseline(svg: &str, baseline: f64) -> String {
    match svg.strip_prefix("<svg") {
        Some(rest) => format!("<svg {BASELINE}=\"{baseline:.3}pt\"{rest}"),
        None => svg.to_owned(),
    }
}

/// The formula image at `path`, or `None` when it cannot be read or its root element lacks a
/// size or a baseline.
#[must_use]
pub fn formula_image(path: &Path) -> Option<FormulaImage> {
    let mut head = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEAD)
        .read_to_string(&mut head)
        .ok()?;
    let (width, height, baseline) = measures(&head)?;
    Some(FormulaImage {
        path: path.to_path_buf(),
        width,
        height,
        baseline,
    })
}

/// The width, height and baseline on the root `<svg>` element of `head`.
fn measures(head: &str) -> Option<(f32, f32, f32)> {
    let start = head.find("<svg")?;
    let root = &head[start..start + head[start..].find('>')?];
    let points = |name: &str| -> Option<f32> {
        let key = format!(" {name}=\"");
        let value = &root[root.find(&key)? + key.len()..];
        let value = &value[..value.find('"')?];
        let number: f32 = value.strip_suffix("pt").unwrap_or(value).parse().ok()?;
        (number.is_finite() && number >= 0.0).then_some(number)
    };
    Some((points("width")?, points("height")?, points(BASELINE)?))
}

impl ViewTree {
    /// Every typeset inline formula, by its source as written (trimmed): what the visual editor
    /// draws in place of a STEM macro, whose text is that source.
    #[must_use]
    pub fn inline_formulas(&self) -> HashMap<String, FormulaImage> {
        let mut out = HashMap::new();
        if let Some(title) = &self.title {
            formulas_in(title, &mut out);
        }
        blocks_formulas(&self.blocks, &mut out);
        for footnote in &self.footnotes {
            formulas_in(&footnote.content, &mut out);
        }
        out
    }
}

fn blocks_formulas(blocks: &[ViewBlock], out: &mut HashMap<String, FormulaImage>) {
    for block in blocks {
        formulas_in(&block.title, out);
        match &block.kind {
            ViewKind::Section { title, blocks, .. } => {
                formulas_in(title, out);
                blocks_formulas(blocks, out);
            }
            ViewKind::Heading { title, .. } => formulas_in(title, out),
            ViewKind::Paragraph(content) => formulas_in(content, out),
            ViewKind::List { items, .. } => {
                for item in items {
                    formulas_in(&item.content, out);
                    blocks_formulas(&item.blocks, out);
                }
            }
            ViewKind::DescriptionList(items) => {
                for item in items {
                    formulas_in(&item.term, out);
                    formulas_in(&item.content, out);
                    blocks_formulas(&item.blocks, out);
                }
            }
            ViewKind::Table(table) => {
                for cell in table
                    .header
                    .iter()
                    .flatten()
                    .chain(table.rows.iter().flatten())
                    .chain(table.footer.iter().flatten())
                {
                    blocks_formulas(&cell.blocks, out);
                }
            }
            ViewKind::Admonition { blocks, .. }
            | ViewKind::Quote { blocks, .. }
            | ViewKind::Sidebar(blocks)
            | ViewKind::Example(blocks)
            | ViewKind::Open(blocks) => blocks_formulas(blocks, out),
            _ => {}
        }
    }
}

fn formulas_in(inlines: &[ViewInline], out: &mut HashMap<String, FormulaImage>) {
    for inline in inlines {
        match inline {
            ViewInline::Stem {
                source,
                rendered: Some(image),
                ..
            } => {
                out.insert(source.trim().to_owned(), image.clone());
            }
            ViewInline::Strong(content)
            | ViewInline::Emphasis(content)
            | ViewInline::Code(content)
            | ViewInline::Mark(content)
            | ViewInline::Superscript(content)
            | ViewInline::Subscript(content)
            | ViewInline::Span { content, .. }
            | ViewInline::Link { content, .. }
            | ViewInline::Xref { content, .. }
            | ViewInline::IndexTerm { content, .. } => formulas_in(content, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPST: &str = r#"<svg class="typst-doc" viewBox="0 0 14.6854 12.2" width="14.6854pt" height="12.2pt" xmlns="http://www.w3.org/2000/svg"><path d="M0 0"/></svg>"#;

    #[test]
    fn the_baseline_travels_in_the_svg() {
        let svg = with_baseline(TYPST, 9.541);
        assert!(svg.starts_with("<svg data-baseline=\"9.541pt\" class="));
        assert_eq!(measures(&svg), Some((14.6854, 12.2, 9.541)));

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("formula.svg");
        std::fs::write(&path, &svg).unwrap();
        let image = formula_image(&path).unwrap();
        assert_eq!(
            (image.width, image.height, image.baseline),
            (14.6854, 12.2, 9.541)
        );
    }

    #[test]
    fn inline_formulas_are_found_by_their_source() {
        let image = FormulaImage {
            path: PathBuf::from("/cache/x.svg"),
            width: 1.0,
            height: 2.0,
            baseline: 1.5,
        };
        let stem = |source: &str, rendered: Option<FormulaImage>| ViewInline::Stem {
            latex: false,
            source: source.to_owned(),
            rendered,
        };
        let block = |kind| ViewBlock {
            span: crate::SourceSpan::default(),
            id: None,
            roles: Vec::new(),
            style: None,
            title: Vec::new(),
            caption: None,
            attributes: std::collections::BTreeMap::new(),
            options: std::collections::BTreeSet::new(),
            kind,
        };
        let tree = ViewTree {
            source: PathBuf::from("/page.adoc"),
            title: None,
            blocks: vec![block(ViewKind::Example(vec![block(ViewKind::Paragraph(
                vec![
                    ViewInline::Strong(vec![stem(" x^2 ", Some(image.clone()))]),
                    stem("y", None),
                ],
            ))]))],
            toc: Vec::new(),
            footnotes: Vec::new(),
            attributes: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            tree.inline_formulas(),
            HashMap::from([("x^2".to_owned(), image)])
        );
    }

    #[test]
    fn an_image_without_a_baseline_has_no_measures() {
        assert_eq!(measures(TYPST), None);
        assert_eq!(measures("not svg"), None);
        assert_eq!(
            measures("<svg width=\"x\" height=\"1\" data-baseline=\"1\">"),
            None
        );
        assert_eq!(with_baseline("<?xml?>", 1.0), "<?xml?>");
    }
}
