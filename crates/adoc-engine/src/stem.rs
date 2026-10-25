//! Formulas as images for the preview and the HTML backend: a diagram provider that converts
//! STEM to Typst maths with `adoc-math` and typesets it as SVG with `adoc-pdf`, so formulas are
//! rendered on workers and cached like diagrams. A PDF typesets its formulas itself.

use adoc_diagrams::{DiagramError, DiagramProvider, DiagramRequest};

/// Renders `stem-asciimath` and `stem-latex` requests; the attribute `inline` makes the formula
/// inline-sized rather than displayed, and records its baseline in the image
/// (`adoc_render::with_baseline`) so the preview can set it on the line.
pub struct StemProvider;

impl DiagramProvider for StemProvider {
    fn kinds(&self) -> &[&str] {
        &["stem-asciimath", "stem-latex"]
    }

    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
        let notation = if request.kind == "stem-latex" {
            adoc_math::Notation::Latex
        } else {
            adoc_math::Notation::AsciiMath
        };
        let math = adoc_math::to_typst(notation, &request.source)
            .map_err(|error| DiagramError::Failed(error.message))?;
        let block = !request.attributes.contains_key("inline");
        let rendered = adoc_pdf::render_math(
            &math,
            block,
            if block {
                15.0
            } else {
                f64::from(adoc_render::INLINE_FORMULA_POINTS)
            },
        )
        .map_err(|error| DiagramError::Failed(error.to_string()))?;
        Ok(if block {
            rendered.svg
        } else {
            adoc_render::with_baseline(&rendered.svg, rendered.baseline)
        })
    }
}
