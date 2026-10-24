//! WP 5.5: STEM typeset as Typst maths in the PDF, and one formula rendered to SVG.

mod common;

use std::path::Path;

use adoc_math::{to_typst, Notation};
use adoc_pdf::{export, render_math_svg, to_typst as emit, typeset_text, PdfOptions, PdfStandard};

const PAGE: &str = "= Maths\n:stem:\n\nThe sum is stem:[sum_(i=1)^n i] here.\n\n[stem]\n++++\nalpha^2 + beta^2 = gamma^2\n++++\n\n[latexmath]\n++++\n\\frac{a}{b} + \\sqrt{x}\n++++\n";

#[test]
fn stem_typesets_in_the_pdf() {
    let view = common::view_of_text(Path::new("/doc/maths.adoc"), PAGE);
    let document = common::article(view);
    let typst = emit(&document, &common::default_theme(), &PdfOptions::default());
    assert_eq!(
        typst.source.matches("#adoc-stem(").count(),
        3,
        "three formulas"
    );
    assert!(typst.unconverted.is_empty(), "{:?}", typst.unconverted);
    // The formulas are typeset: their symbols are in the text, their source is not.
    let text = typeset_text(&typst, &common::default_theme(), &PdfOptions::default())
        .unwrap_or_else(|error| panic!("{error}"))
        .join("\n");
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    // Maths letters are set in their mathematical italic forms.
    for symbol in ["∑", "𝛼", "𝛽", "𝛾", "√"] {
        assert!(squeezed.contains(symbol), "{symbol} in {text}");
    }
    assert!(!text.contains("alpha^2"), "{text}");
    assert!(!text.contains("\\frac"), "{text}");
    // A tagged PDF with equations passes Typst's PDF/UA-1 checks.
    export(
        &document,
        &common::default_theme(),
        &PdfOptions {
            standards: vec![PdfStandard::Ua1],
            date: Some((2026, 10, 24)),
            ..PdfOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn unconvertible_stem_is_shown_as_source_with_a_warning() {
    let view = common::view_of_text(
        Path::new("/doc/maths.adoc"),
        "= Maths\n\n[latexmath]\n++++\n\\href{https://example.com}{x}\n++++\n",
    );
    let typst = emit(
        &common::article(view),
        &common::default_theme(),
        &PdfOptions::default(),
    );
    assert_eq!(typst.unconverted.len(), 1, "{}", typst.source);
    assert!(typst.source.contains("#adoc-code(raw("));
}

#[test]
fn every_helper_is_defined() {
    // The template defines exactly the helpers adoc-math lets converted LaTeX call.
    let template = include_str!("../src/math.typ");
    let scope = &template[template.find("#let adoc-math-scope").unwrap()..];
    let scope = &scope[..scope.find("\n}\n").unwrap()];
    let mut defined: Vec<&str> = scope
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|line| line.split_once("\":").map(|(name, _)| name))
        .collect();
    defined.sort_unstable();
    let mut helpers = adoc_math::HELPERS.to_vec();
    helpers.sort_unstable();
    assert_eq!(defined, helpers);
}

#[test]
fn a_formula_renders_to_svg() {
    let math = to_typst(Notation::AsciiMath, "sqrt(x^2 + 1)").unwrap();
    let svg = render_math_svg(&math, true, 14.0).unwrap_or_else(|error| panic!("{error}"));
    assert!(svg.starts_with("<svg"), "{}", &svg[..svg.len().min(200)]);
    let inline = render_math_svg(&math, false, 14.0).unwrap();
    assert!(
        inline.contains("<path") || inline.contains("<use"),
        "the glyphs are drawn"
    );
    // Every helper a LaTeX conversion can call is defined where formulas are evaluated.
    for latex in [
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        r"\mathbf{x} + \dfrac{a}{b} + \xrightarrow{f}",
        r"\operatorname{rank}(A) + \operatorname*{argmax}_x f",
        r"\text{if } x > 0",
        r"\sqrt[3]{x} + \overbrace{a+b}^{n}",
    ] {
        let math = to_typst(Notation::Latex, latex)
            .unwrap_or_else(|error| panic!("{latex}: {}", error.message));
        render_math_svg(&math, true, 12.0)
            .unwrap_or_else(|error| panic!("{latex} as {}: {error}", math.as_str()));
    }
}
