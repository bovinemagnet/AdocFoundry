//! The Typst string literal writer behind `adoc-pdf`'s promise that every document string is
//! a Typst string literal: for any text, the literal parses in code as exactly one `Str` whose
//! value is the text; behind `#` in markup as exactly one embedded string; and two of them in
//! a row, in markup or as arguments, as two strings, so one can never end early and let the
//! text after it be read as code.

#![no_main]

use adoc_pdf::fuzzing::{string, text};
use libfuzzer_sys::fuzz_target;
use typst_syntax::{ast, SyntaxKind, SyntaxNode};

fuzz_target!(|data: &[u8]| {
    let value = String::from_utf8_lossy(data);
    let literal = string(&value);

    let code = typst_syntax::parse_code(&literal);
    assert_eq!(strings(&code, &literal), [value.as_ref()]);
    assert_eq!(kinds(&code), [SyntaxKind::Str], "{literal:?}");

    let markup = text(&value);
    if value.is_empty() {
        assert!(markup.is_empty());
    } else {
        let parsed = typst_syntax::parse(&markup);
        assert_eq!(strings(&parsed, &markup), [value.as_ref()]);
        assert_eq!(
            kinds(&parsed),
            [SyntaxKind::Hash, SyntaxKind::Str],
            "{markup:?}"
        );
    }

    // The text split in two, each half written on its own.
    let middle = (0..=value.len() / 2)
        .rev()
        .find(|&at| value.is_char_boundary(at))
        .unwrap_or(0);
    let (first, second) = value.split_at(middle);
    let pair = format!("{}{}", text(first), text(second));
    let parsed = typst_syntax::parse(&pair);
    let expected: Vec<&str> = [first, second]
        .into_iter()
        .filter(|half| !half.is_empty())
        .collect();
    assert_eq!(strings(&parsed, &pair), expected);
    let arguments = format!("f({}, {})", string(first), string(second));
    let parsed = typst_syntax::parse_code(&arguments);
    assert_eq!(strings(&parsed, &arguments), [first, second]);
});

/// The values of every string in the tree, which must parse without errors.
fn strings(root: &SyntaxNode, source: &str) -> Vec<String> {
    assert!(!root.diagnosis().errors, "{source:?} does not parse");
    assert_eq!(root.full_text(), source);
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

fn collect(node: &SyntaxNode, out: &mut Vec<String>) {
    if let Some(string) = node.cast::<ast::Str>() {
        out.push(string.get().to_string());
    }
    for child in node.children() {
        collect(child, out);
    }
}

/// The kinds of the root's children, trivia left out.
fn kinds(root: &SyntaxNode) -> Vec<SyntaxKind> {
    root.children()
        .map(SyntaxNode::kind)
        .filter(|kind| !kind.is_trivia())
        .collect()
}
