//! STEM notation for typesetting: AsciiMath and LaTeX converted to Typst maths markup, and that
//! markup checked to hold nothing but maths, so it can be evaluated without letting a document
//! run Typst code.

use std::{collections::BTreeSet, sync::OnceLock};

use typst_library::foundations::Value;
use typst_syntax::{ast, SyntaxKind, SyntaxNode};

mod asciimath;
mod latex;

/// The notation a formula is written in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Notation {
    AsciiMath,
    Latex,
}

/// Typst maths markup that passed [`is_safe`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypstMath(String);

impl TypstMath {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a formula could not be converted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MathError {
    pub message: String,
    /// A byte offset into the source, when the converter knows where.
    pub offset: Option<usize>,
}

impl MathError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            offset: None,
        }
    }
}

/// The functions LaTeX conversion calls that Typst's maths does not have. Whoever evaluates the
/// converted markup must define them in the scope it evaluates in; `adoc-pdf` does.
pub const HELPERS: &[&str] = &[
    "Bmatrix",
    "Set",
    "Vmatrix",
    "aligned",
    "arcctg",
    "arctg",
    "argmax",
    "argmin",
    "bcancel",
    "big",
    "Big",
    "bigg",
    "Bigg",
    "bmatrix",
    "boxed",
    "bra",
    "braket",
    "cfrac",
    "ch",
    "cosec",
    "cotg",
    "cth",
    "dbinom",
    "dfrac",
    "injlim",
    "ket",
    "mathbin",
    "mathclap",
    "mathclose",
    "mathinner",
    "mathop",
    "mathopen",
    "mathord",
    "mathpunct",
    "mathrel",
    "matrix",
    "middle",
    "mitexbold",
    "mitexcal",
    "mitexcolor",
    "mitexdisplay",
    "mitexfrak",
    "mitexinline",
    "mitexitalic",
    "mitexmathbf",
    "mitexmono",
    "mitexoverbrace",
    "mitexoverbracket",
    "mitexsans",
    "mitexscript",
    "mitexsscript",
    "mitexunderbrace",
    "mitexunderbracket",
    "mitexupright",
    "negmedspace",
    "negthickspace",
    "negthinspace",
    "overset",
    "phantom",
    "plim",
    "pmatrix",
    "pmod",
    "projlim",
    "rcases",
    "sect",
    "set",
    "sh",
    "smallint",
    "stackrel",
    "tbinom",
    "tfrac",
    "th",
    "thickapprox",
    "thicksim",
    "underbar",
    "underset",
    "varDelta",
    "varGamma",
    "varLambda",
    "varOmega",
    "varPhi",
    "varPi",
    "varPsi",
    "varSigma",
    "varTheta",
    "varUpsilon",
    "varXi",
    "vmatrix",
    "xLeftarrow",
    "xLeftrightarrow",
    "xRightarrow",
    "xleftarrow",
    "xleftrightarrow",
    "xlongequal",
    "xmapsto",
    "xrightarrow",
];

/// `source` in `notation` as Typst maths markup. A formula that cannot be converted, or whose
/// conversion is not plainly maths, is an error, never a panic.
///
/// # Errors
///
/// When the source cannot be read, its conversion uses anything [`is_safe`] refuses, or it calls
/// a function with fewer or more arguments than the function takes (`\frac{1}`), which Typst
/// would refuse when it evaluates the formula.
pub fn to_typst(notation: Notation, source: &str) -> Result<TypstMath, MathError> {
    let converted = match notation {
        Notation::AsciiMath => asciimath::convert(source)?,
        Notation::Latex => latex::convert(source)?,
    };
    if !is_safe(&converted) {
        return Err(MathError::new(
            "the formula uses something other than maths, and is shown as its source",
        ));
    }
    if let Some(problem) = wrong_arguments(&typst_syntax::parse_math(&converted)) {
        return Err(MathError::new(format!(
            "{problem}, and the formula is shown as its source"
        )));
    }
    Ok(TypstMath(converted))
}

/// The first call in `node` given fewer or more positional arguments than its function takes,
/// as Typst counts a call's arguments when it evaluates it; `None` when every call fits or its
/// arguments cannot be counted (named, spread or two-dimensional).
fn wrong_arguments(node: &SyntaxNode) -> Option<String> {
    if let Some(call) = node.cast::<ast::MathCall>() {
        if let ast::MathAccess::MathIdent(ident) = call.callee() {
            let name = ident.get().as_str();
            let mut count = 0;
            let mut countable = true;
            for arg in call.args().arg_items() {
                match arg.arg {
                    ast::Arg::Pos(_) if !arg.ends_in_semicolon => count += 1,
                    ast::Arg::Named(_) => {}
                    _ => countable = false,
                }
            }
            if let (true, Some((least, most))) = (countable, arity(name)) {
                if count < least {
                    return Some(format!("`{name}` is missing an argument"));
                }
                if most.is_some_and(|most| count > most) {
                    return Some(format!("`{name}` is given too many arguments"));
                }
            }
        }
    }
    node.children().find_map(wrong_arguments)
}

/// How many positional arguments the function `name` takes, at least and at most (`None` for
/// any number); `None` for a name that is not a function: a helper's from its definition in
/// `adoc-pdf`'s template, any other's from Typst's maths scope.
fn arity(name: &str) -> Option<(usize, Option<usize>)> {
    match name {
        "cfrac" | "dbinom" | "dfrac" | "mitexcolor" | "overset" | "stackrel" | "tbinom"
        | "tfrac" | "underset" => return Some((2, Some(2))),
        "Set" | "aligned" | "bcancel" | "big" | "Big" | "bigg" | "Bigg" | "boxed" | "bra"
        | "braket" | "ket" | "mathbin" | "mathclap" | "mathclose" | "mathinner" | "mathop"
        | "mathopen" | "mathord" | "mathpunct" | "mathrel" | "middle" | "phantom" | "pmod"
        | "set" | "underbar" | "xLeftarrow" | "xLeftrightarrow" | "xRightarrow" | "xleftarrow"
        | "xleftrightarrow" | "xlongequal" | "xmapsto" | "xrightarrow" => {
            return Some((1, Some(1)))
        }
        name if name.starts_with("mitex") => return Some((1, Some(1))),
        // The matrices and `rcases` take any number; the rest are not functions.
        name if HELPERS.contains(&name) => return None,
        _ => {}
    }
    let module = typst_library::math::module();
    let Value::Func(function) = module.scope().get(name)?.read() else {
        return None;
    };
    // Arguments are taken in order, so an optional parameter before a required one takes the
    // first argument (`root(3)` has no radicand): every parameter up to the last required one
    // needs an argument.
    let (mut least, mut most) = (0, Some(0));
    for (index, param) in function
        .params()
        .filter(|param| param.positional())
        .enumerate()
    {
        if param.required() {
            least = index + 1;
        }
        most = if param.variadic() {
            None
        } else {
            most.map(|most| most + 1)
        };
    }
    Some((least, most))
}

/// Whether `markup` is Typst maths and nothing else: no embedded code (`#`), and no function or
/// name other than Typst's maths functions and symbols and the [`HELPERS`]. A single letter is
/// always a variable. Markup Typst cannot parse is refused too.
#[must_use]
pub fn is_safe(markup: &str) -> bool {
    fn walk(node: &SyntaxNode, allowed: &BTreeSet<String>) -> bool {
        match node.kind() {
            SyntaxKind::Hash | SyntaxKind::Error => return false,
            // `arrow.r.long`: the base is a name to check, the fields are its variants.
            SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess => {
                return node
                    .children()
                    .next()
                    .is_some_and(|base| walk(base, allowed));
            }
            SyntaxKind::MathIdent => {
                let name = node.leaf_text();
                if name.chars().count() > 1 && !allowed.contains(name.as_str()) {
                    return false;
                }
            }
            _ => {}
        }
        if node.diagnosis().errors {
            return false;
        }
        node.children().all(|child| walk(child, allowed))
    }
    walk(&typst_syntax::parse_math(markup), allowed_names())
}

/// The names maths markup may use: every name in Typst's maths scope (functions and symbols)
/// and the [`HELPERS`].
fn allowed_names() -> &'static BTreeSet<String> {
    static NAMES: OnceLock<BTreeSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        typst_library::math::module()
            .scope()
            .iter()
            .map(|(name, _)| name.to_string())
            .chain(HELPERS.iter().map(|&name| name.to_owned()))
            .collect()
    })
}
