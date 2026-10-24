//! Formulas as images for the preview and the HTML backend: a diagram provider that converts
//! STEM to Typst maths with `adoc-math` and typesets it as SVG with `adoc-pdf`, so formulas are
//! rendered on workers and cached like diagrams. A PDF typesets its formulas itself.

use adoc_diagrams::{DiagramError, DiagramProvider, DiagramRequest};

/// Renders `stem-asciimath` and `stem-latex` requests; the attribute `inline` makes the formula
/// inline-sized rather than displayed.
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
        adoc_pdf::render_math_svg(&math, block, if block { 15.0 } else { 13.0 })
            .map_err(|error| DiagramError::Failed(error.to_string()))
    }
}
