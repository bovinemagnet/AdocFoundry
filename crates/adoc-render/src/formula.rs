//! A typeset inline formula's image and its measures. The baseline travels inside the SVG, as an
//! attribute of its root element, so a formula read back from the diagram cache still knows
//! where it sits on the line.

use std::{
    collections::HashMap,
    io::Read as _,
    path::{Path, PathBuf},
};

use crate::{ViewInline, ViewTree};

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
    adoc_core::probe::hit(adoc_core::probe::Probe::FormulaRead);
    // Read as bytes: the head may end inside a character.
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEAD)
        .read_to_end(&mut head)
        .ok()?;
    let (width, height, baseline) = measures(&String::from_utf8_lossy(&head))?;
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

/// An inline formula as written: its notation and its source (trimmed). AsciiMath and LaTeX read
/// the same source differently, so both name the formula.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FormulaKey {
    pub latex: bool,
    pub source: String,
}

impl FormulaKey {
    /// The formula a STEM macro named `name` (`stem`, `latexmath` or `asciimath`) with `source`
    /// writes, `stem` being LaTeX when `stem_latex` (from [`ViewTree::stem_is_latex`]).
    #[must_use]
    pub fn of_macro(name: &str, source: &str, stem_latex: bool) -> Self {
        Self {
            latex: match name {
                "latexmath" => true,
                "asciimath" => false,
                _ => stem_latex,
            },
            source: source.trim().to_owned(),
        }
    }
}

impl ViewTree {
    /// Whether `stem:[…]` is LaTeX on this page: when its `:stem:` is `latexmath`, as the parser
    /// reads it, else AsciiMath.
    #[must_use]
    pub fn stem_is_latex(&self) -> bool {
        self.attributes
            .get("stem")
            .is_some_and(|value| value == "latexmath")
    }

    /// Every typeset inline formula, by its notation and source: what the visual editor draws in
    /// place of a STEM macro, whose text is that source.
    #[must_use]
    pub fn inline_formulas(&self) -> HashMap<FormulaKey, FormulaImage> {
        let mut out = HashMap::new();
        if let Some(title) = &self.title {
            formulas_in(title, &mut out);
        }
        self.each_block(&mut |block| {
            block.each_inline_run(&mut |inlines| formulas_in(inlines, &mut out));
        });
        for footnote in &self.footnotes {
            formulas_in(&footnote.content, &mut out);
        }
        out
    }
}

fn formulas_in(inlines: &[ViewInline], out: &mut HashMap<FormulaKey, FormulaImage>) {
    for inline in inlines {
        match inline {
            ViewInline::Stem {
                latex,
                source,
                rendered: Some(image),
            } => {
                let key = FormulaKey {
                    latex: *latex,
                    source: source.trim().to_owned(),
                };
                out.insert(key, image.clone());
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
    use crate::{ViewBlock, ViewKind};

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
            attribute_images: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            tree.inline_formulas(),
            HashMap::from([(FormulaKey::of_macro("asciimath", "x^2", false), image)])
        );
    }

    /// A formula in a callout item is found too.
    #[test]
    fn inline_formulas_are_found_in_callout_items() {
        let image = FormulaImage {
            path: PathBuf::from("/cache/x.svg"),
            width: 1.0,
            height: 2.0,
            baseline: 1.5,
        };
        let tree = ViewTree {
            source: PathBuf::from("/page.adoc"),
            blocks: vec![crate::ViewBlock {
                span: crate::SourceSpan::default(),
                id: None,
                roles: Vec::new(),
                style: None,
                title: Vec::new(),
                caption: None,
                attributes: std::collections::BTreeMap::new(),
                options: std::collections::BTreeSet::new(),
                kind: crate::ViewKind::CalloutList(vec![crate::ViewCallout {
                    span: crate::SourceSpan::default(),
                    number: 1,
                    content: vec![ViewInline::Stem {
                        latex: false,
                        source: "x".to_owned(),
                        rendered: Some(image.clone()),
                    }],
                    blocks: Vec::new(),
                }]),
            }],
            ..ViewTree::default()
        };
        assert_eq!(
            tree.inline_formulas(),
            HashMap::from([(FormulaKey::of_macro("asciimath", "x", false), image)])
        );
    }

    /// Issue #473: AsciiMath and LaTeX read the same text differently, so `stem:[x/2]` and
    /// `latexmath:[x/2]` on one page are two formulas, each with its own image.
    #[test]
    fn the_same_source_in_two_notations_is_two_formulas() {
        let image = |name: &str| FormulaImage {
            path: PathBuf::from(format!("/cache/{name}.svg")),
            width: 1.0,
            height: 2.0,
            baseline: 1.5,
        };
        let stem = |latex: bool, rendered: FormulaImage| ViewInline::Stem {
            latex,
            source: "x/2".to_owned(),
            rendered: Some(rendered),
        };
        let tree = ViewTree {
            title: Some(vec![
                stem(false, image("asciimath")),
                stem(true, image("latex")),
            ]),
            ..ViewTree::default()
        };
        let formulas = tree.inline_formulas();
        assert_eq!(formulas.len(), 2);
        assert_eq!(
            formulas[&FormulaKey::of_macro("latexmath", "x/2", false)],
            image("latex")
        );
        assert_eq!(
            formulas[&FormulaKey::of_macro("asciimath", "x/2", true)],
            image("asciimath")
        );
        // `stem:[…]` takes the page's `:stem:`.
        assert!(!tree.stem_is_latex());
        let latex_page = ViewTree {
            attributes: [("stem".to_owned(), "latexmath".to_owned())].into(),
            ..ViewTree::default()
        };
        assert!(latex_page.stem_is_latex());
        assert_eq!(
            formulas[&FormulaKey::of_macro("stem", " x/2 ", latex_page.stem_is_latex())],
            image("latex")
        );
    }

    /// The head read may end inside a character, which leaves the root element readable.
    #[test]
    fn a_head_ending_inside_a_character_is_read() {
        let svg = with_baseline(TYPST, 9.541);
        let padding = HEAD as usize - svg.len() - 1;
        let svg = format!("{svg}{}\u{e9}", " ".repeat(padding));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("formula.svg");
        std::fs::write(&path, &svg).unwrap();
        assert!(formula_image(&path).is_some());
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
