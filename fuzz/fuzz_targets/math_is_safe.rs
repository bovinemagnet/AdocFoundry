//! `adoc_math::is_safe`, the boundary deciding what reaches Typst's `eval`, and the
//! converters behind `to_typst`. The first byte chooses what the rest is: Typst maths markup
//! given to `is_safe` itself, or AsciiMath or LaTeX given to `to_typst`. Whatever `is_safe`
//! accepts, or `to_typst` returns, is checked by a second, independent walk of the tree:
//!
//! - Typst parses it as maths without errors;
//! - no node is code: no `#`, no code or content block, binding, rule, loop, closure,
//!   import, include, keyword or nested equation;
//! - a `#` in the text is only ever inside a string, a comment, an escape or a leading `#!`
//!   line, which Typst skips: shown or ignored, never run (the markup may hold `#` as text,
//!   so "contains no `#`" would be wrong);
//! - every name of more than one letter, wherever it is in the tree, is in Typst's maths
//!   scope or `HELPERS`; the fields of a field access are the variants of a name so checked.

#![no_main]

use std::{collections::BTreeSet, sync::OnceLock};

use adoc_math::{is_safe, to_typst, Notation, HELPERS};
use libfuzzer_sys::fuzz_target;
use typst_syntax::{SyntaxKind, SyntaxNode};

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let source = String::from_utf8_lossy(rest);
    let markup = match selector % 3 {
        0 => {
            if !is_safe(&source) {
                return;
            }
            source.into_owned()
        }
        choice => {
            let notation = if choice == 1 {
                Notation::AsciiMath
            } else {
                Notation::Latex
            };
            let Ok(math) = to_typst(notation, &source) else {
                return;
            };
            assert!(is_safe(math.as_str()));
            math.as_str().to_owned()
        }
    };
    let root = typst_syntax::parse_math(&markup);
    assert!(
        !root.diagnosis().errors,
        "{markup:?} was accepted with errors"
    );
    check(&root, &markup);
});

fn check(node: &SyntaxNode, markup: &str) {
    let kind = node.kind();
    assert!(!is_code(kind), "{markup:?} was accepted holding {kind:?}");
    if node.leaf_text().contains('#') {
        assert!(
            matches!(
                kind,
                SyntaxKind::Str
                    | SyntaxKind::LineComment
                    | SyntaxKind::BlockComment
                    | SyntaxKind::Shebang
                    | SyntaxKind::Escape
            ),
            "{markup:?} was accepted with `#` in {kind:?}"
        );
    }
    match kind {
        SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess => {
            let mut children = node.children();
            if let Some(base) = children.next() {
                check(base, markup);
            }
            for field in children {
                assert!(
                    matches!(
                        field.kind(),
                        SyntaxKind::Dot | SyntaxKind::Ident | SyntaxKind::MathIdent
                    ),
                    "{markup:?} was accepted with a field {:?}",
                    field.kind()
                );
            }
            return;
        }
        // `delim: "["`: the argument's name is not a name looked up.
        SyntaxKind::Named => {
            for child in node
                .children()
                .skip_while(|child| child.kind() != SyntaxKind::Colon)
            {
                check(child, markup);
            }
            return;
        }
        SyntaxKind::MathIdent | SyntaxKind::Ident => {
            let name = node.leaf_text();
            assert!(
                name.chars().count() <= 1 || allowed().contains(name.as_str()),
                "{markup:?} was accepted naming {name:?}"
            );
        }
        _ => {}
    }
    for child in node.children() {
        check(child, markup);
    }
}

/// Kinds that only code makes.
fn is_code(kind: SyntaxKind) -> bool {
    kind.is_keyword()
        || matches!(
            kind,
            SyntaxKind::Hash
                | SyntaxKind::Error
                | SyntaxKind::Code
                | SyntaxKind::CodeBlock
                | SyntaxKind::ContentBlock
                | SyntaxKind::Markup
                | SyntaxKind::Equation
                | SyntaxKind::Raw
                | SyntaxKind::Closure
                | SyntaxKind::Params
                | SyntaxKind::LetBinding
                | SyntaxKind::SetRule
                | SyntaxKind::ShowRule
                | SyntaxKind::Contextual
                | SyntaxKind::Conditional
                | SyntaxKind::WhileLoop
                | SyntaxKind::ForLoop
                | SyntaxKind::ModuleImport
                | SyntaxKind::ImportItems
                | SyntaxKind::ImportItemPath
                | SyntaxKind::RenamedImportItem
                | SyntaxKind::ModuleInclude
                | SyntaxKind::LoopBreak
                | SyntaxKind::LoopContinue
                | SyntaxKind::FuncReturn
                | SyntaxKind::Destructuring
                | SyntaxKind::DestructAssignment
        )
}

/// Typst's maths scope and the helpers, gathered here rather than taken from `adoc-math`.
fn allowed() -> &'static BTreeSet<String> {
    static NAMES: OnceLock<BTreeSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        typst_library::math::module()
            .scope()
            .iter()
            .map(|(name, _)| name.to_string())
            .chain(HELPERS.iter().map(|&name| (*name).to_owned()))
            .collect()
    })
}
