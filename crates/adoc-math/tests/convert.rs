//! WP 5.4: AsciiMath and LaTeX to Typst maths, checked to hold no code.

use std::path::Path;

use adoc_math::{to_typst, Notation};

/// The `source<TAB>expected` pairs of a fixture, without comments and blank lines.
fn samples(name: &str) -> Vec<(String, String)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/stem")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (source, expected) = line.split_once('\t').expect("a tab between the two");
            (source.to_owned(), expected.to_owned())
        })
        .collect()
}

#[test]
fn asciimath_converts_to_typst() {
    for (source, expected) in samples("asciimath.txt") {
        let converted = to_typst(Notation::AsciiMath, &source)
            .unwrap_or_else(|error| panic!("{source}: {}", error.message));
        assert_eq!(converted.as_str(), expected, "from {source}");
    }
}

#[test]
fn latex_converts_to_typst() {
    for (source, expected) in samples("latex.txt") {
        let converted = to_typst(Notation::Latex, &source)
            .unwrap_or_else(|error| panic!("{source}: {}", error.message));
        assert_eq!(converted.as_str().trim_end(), expected, "from {source}");
    }
}

#[test]
fn converted_maths_holds_no_code() {
    // Whatever the source, nothing that runs code reaches Typst.
    for (notation, source) in [
        (Notation::Latex, r#"\text{#read("/etc/passwd")}"#),
        (Notation::Latex, r"\boxed{x}"),
        (Notation::Latex, r"\href{https://example.com}{x}"),
        (Notation::AsciiMath, r#"text(#eval("1"))"#),
        (Notation::AsciiMath, r##""#read(\"x\")""##),
    ] {
        if let Ok(converted) = to_typst(notation, source) {
            let text = converted.as_str();
            assert!(
                adoc_math::is_safe(text),
                "{source} gave {text}, which the check refuses"
            );
        }
    }
    // The check itself refuses code and functions outside maths.
    assert!(!adoc_math::is_safe("#read(\"x\")"));
    assert!(!adoc_math::is_safe("read(\"x\")"));
    assert!(!adoc_math::is_safe("eval(\"1\")"));
    assert!(adoc_math::is_safe("frac(a, b) + sqrt(x) + bold(R) + α"));
    assert!(adoc_math::is_safe(
        "pmatrix(a, b; c, d) + dot.c + arrow.r.long"
    ));
}

#[test]
fn calls_short_of_an_argument_are_refused() {
    // Each would fail Typst's evaluation, and with it the whole document it is in.
    for (notation, source) in [
        (Notation::Latex, r"\frac{1}"),
        (Notation::Latex, r"\binom{n}"),
        (Notation::Latex, r"\cfrac{1}"),
        (Notation::Latex, r"\overset{a}"),
        (Notation::Latex, r"\sqrt"),
        (Notation::AsciiMath, "frac"),
        (Notation::AsciiMath, "root(3)"),
    ] {
        let error = to_typst(notation, source).expect_err(source);
        assert!(error.message.contains("argument"), "{source}: {error:?}");
    }
    // Calls that fit, an optional argument left out among them.
    for (notation, source) in [
        (
            Notation::Latex,
            r"\frac{1}{2} + \binom{n}{k} + \sqrt{x} + \sqrt[3]{x}",
        ),
        (
            Notation::Latex,
            r"\overbrace{a+b}^{n} + \begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        ),
        (Notation::AsciiMath, "root(3)(x) + sqrt x + ubrace(a)"),
    ] {
        to_typst(notation, source).unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
}

#[test]
fn half_typed_maths_does_not_panic() {
    for source in [
        "", "x^", "a/", "sqrt(", "[[a,", "text(", "\"", "sum_(", "((((", "))", "|", "_",
    ] {
        let _ = to_typst(Notation::AsciiMath, source);
    }
    for source in [
        "",
        r"\frac{",
        r"\sqrt[",
        r"\begin{pmatrix}",
        r"\text{",
        "}",
        "^",
        "_",
        r"\left(",
        r"\end{x}",
    ] {
        let _ = to_typst(Notation::Latex, source);
    }
}
