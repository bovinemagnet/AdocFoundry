//! AsciiMath to Typst maths, over `asciimath-parser`'s tree. Symbols become their Unicode
//! characters, which Typst maths takes as they are; commands become Typst's maths functions.

use asciimath_parser::tree::{
    Expression, Frac, Func, Group, Intermediate, Matrix, Script, ScriptFunc, Signed, Simple,
    SimpleBinary, SimpleScript, SimpleUnary,
};

use crate::MathError;

/// `source` as Typst maths.
pub fn convert(source: &str) -> Result<String, MathError> {
    let expression = asciimath_parser::parse(source);
    Ok(expression_text(&expression).trim().to_owned())
}

fn expression_text(expression: &Expression<'_>) -> String {
    let mut out = String::new();
    let mut spaced = true;
    for intermediate in expression.iter() {
        if let Intermediate::Space(_) = intermediate {
            if !out.is_empty() && !spaced {
                out.push(' ');
                spaced = true;
            }
            continue;
        }
        let text = intermediate_text(intermediate);
        if text.is_empty() {
            continue;
        }
        // Two terms written together (`2x`) stay two in Typst, not one name.
        if !spaced && needs_space(&out, &text) {
            out.push(' ');
        }
        out.push_str(&text);
        spaced = false;
    }
    out
}

/// Whether `next` written straight after `before` would join with it into one name or number.
fn needs_space(before: &str, next: &str) -> bool {
    let (Some(last), Some(first)) = (before.chars().last(), next.chars().next()) else {
        return false;
    };
    (last.is_alphanumeric() || last == ')' || last == '"')
        && (first.is_alphanumeric() || first == '"')
}

fn intermediate_text(intermediate: &Intermediate<'_>) -> String {
    match intermediate {
        Intermediate::ScriptFunc(script_func) => script_func_text(script_func),
        Intermediate::Frac(Frac { numer, denom }) => format!(
            "frac({}, {})",
            argument_of_script_func(numer),
            argument_of_script_func(denom)
        ),
        Intermediate::Space(_) => " ".to_owned(),
    }
}

fn script_func_text(script_func: &ScriptFunc<'_>) -> String {
    match script_func {
        ScriptFunc::Simple(SimpleScript { simple, script }) => {
            format!("{}{}", simple_text(simple), script_text(script))
        }
        ScriptFunc::Func(Func { func, script, arg }) => {
            let argument = script_func_text(arg);
            let name = function_name(func);
            // `f(x)`: Typst draws a call of something that is not a function as written.
            let gap = if argument.is_empty() || argument.starts_with('(') {
                ""
            } else {
                " "
            };
            format!("{name}{}{gap}{argument}", script_text(script))
        }
        ScriptFunc::Signed(Signed { sign, operand }) => {
            format!("{}{}", sign_text(sign), script_func_text(operand))
        }
    }
}

/// A fraction's part: the formula inside a group's brackets, which asciimath drops there.
fn argument_of_script_func(script_func: &ScriptFunc<'_>) -> String {
    match script_func {
        ScriptFunc::Simple(SimpleScript {
            simple,
            script: Script::None,
        }) => argument(simple),
        other => script_func_text(other),
    }
}

fn script_text(script: &Script<'_>) -> String {
    match script {
        Script::None => String::new(),
        Script::Sub(sub) => format!("_({})", argument(sub)),
        Script::Super(sup) => format!("^({})", argument(sup)),
        Script::Subsuper(sub, sup) => format!("_({})^({})", argument(sub), argument(sup)),
    }
}

/// An argument: a group's brackets are dropped, as asciimath drops them around an argument.
fn argument(simple: &Simple<'_>) -> String {
    match simple {
        Simple::Group(Group {
            left_bracket,
            expr,
            right_bracket,
        }) if matches!(
            (*left_bracket, *right_bracket),
            ("(", ")") | ("[", "]") | ("{", "}") | ("{:", ":}")
        ) =>
        {
            expression_text(expr).trim().to_owned()
        }
        other => simple_text(other),
    }
}

fn simple_text(simple: &Simple<'_>) -> String {
    match simple {
        Simple::Missing => String::new(),
        Simple::Number(number) => (*number).to_owned(),
        Simple::Text(text) => quoted(text),
        Simple::Ident(ident) => {
            // Letters written together are each a variable.
            let mut out = String::new();
            for character in ident.chars() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&escaped(character));
            }
            out
        }
        Simple::Operator(operator) => operator.chars().map(escaped).collect(),
        Simple::Symbol(symbol) => symbol_text(symbol.text),
        Simple::Sign(sign) => sign_text(sign),
        Simple::Signed(signed) => {
            format!("{}{}", sign_text(signed.sign), simple_text(&signed.operand))
        }
        Simple::Unary(unary) => unary_text(unary),
        Simple::Func(func) => {
            let argument = simple_text(&func.arg);
            format!("{} {argument}", function_name(func.func))
        }
        Simple::Binary(binary) => binary_text(binary),
        Simple::Group(group) => group_text(group),
        Simple::Matrix(matrix) => matrix_text(matrix),
    }
}

fn unary_text(unary: &SimpleUnary<'_>) -> String {
    if matches!(unary.op, "text" | "mbox") {
        let text = match unary.arg.as_ref() {
            Simple::Group(group) => source_text(&group.expr),
            Simple::Text(text) => (*text).to_owned(),
            other => simple_text(other),
        };
        return quoted(&text);
    }
    let argument = argument(&unary.arg);
    let function = match unary.op {
        "sqrt" => "sqrt",
        "abs" | "Abs" => "abs",
        "norm" => "norm",
        "floor" => "floor",
        "ceil" => "ceil",
        "hat" => "hat",
        "bar" | "overline" => "overline",
        "vec" => "arrow",
        "dot" => "dot",
        "ddot" => "dot.double",
        "overarc" | "overparen" => "overparen",
        "ul" | "underline" => "underline",
        "ubrace" | "underbrace" => "underbrace",
        "obrace" | "overbrace" => "overbrace",
        "cancel" => "cancel",
        "tilde" => "tilde",
        "bb" | "mathbf" | "bold" => "bold",
        "sf" | "mathsf" => "sans",
        "bbb" | "mathbb" => "bb",
        "cc" | "mathcal" => "cal",
        "tt" | "mathtt" => "mono",
        "fr" | "mathfrak" => "frak",
        "mathit" | "italic" => "italic",
        "bbit" => return format!("bold(italic({argument}))"),
        "bbsf" => return format!("bold(sans({argument}))"),
        "sfit" => return format!("italic(sans({argument}))"),
        "bbsfit" => return format!("bold(italic(sans({argument})))"),
        "bbcc" => return format!("bold(cal({argument}))"),
        "bbfr" => return format!("bold(frak({argument}))"),
        _ => return argument,
    };
    format!("{function}({argument})")
}

fn binary_text(binary: &SimpleBinary<'_>) -> String {
    let first = argument(&binary.first);
    let second = argument(&binary.second);
    match binary.op {
        "frac" => format!("frac({first}, {second})"),
        "root" => format!("root({first}, {second})"),
        "stackrel" | "overset" => format!("limits({second})^({first})"),
        "underset" => format!("limits({second})_({first})"),
        // Colour and the HTML-only `id` and `class` keep only the formula they mark.
        _ => second,
    }
}

fn group_text(group: &Group<'_>) -> String {
    let inner = expression_text(&group.expr).trim().to_owned();
    format!(
        "{}{inner}{}",
        bracket(group.left_bracket),
        bracket(group.right_bracket)
    )
}

fn bracket(bracket: &str) -> &'static str {
    match bracket {
        "(" | "left(" => "(",
        ")" | "right)" => ")",
        "[" | "left[" => "[",
        "]" | "right]" => "]",
        "{" => "\\{",
        "}" => "\\}",
        "|" | "|:" | ":|" => "|",
        "||" => "‖",
        "(:" | "<<" | "langle" => "⟨",
        ":)" | ">>" | "rangle" => "⟩",
        "|__" | "lfloor" => "⌊",
        "__|" | "rfloor" => "⌋",
        "|~" | "lceiling" => "⌈",
        "~|" | "rceiling" => "⌉",
        // `{:` and `:}` are invisible.
        _ => "",
    }
}

fn matrix_text(matrix: &Matrix<'_>) -> String {
    let rows: Vec<String> = matrix
        .rows()
        .map(|row| {
            row.iter()
                .map(|cell| expression_text(cell).trim().to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect();
    let cells = rows.join("; ");
    match (matrix.left_bracket, matrix.right_bracket) {
        // `{(x, x > 0), (-x, x < 0):}` is a case distinction.
        ("{", ":}") => {
            let rows: Vec<String> = matrix
                .rows()
                .map(|row| {
                    row.iter()
                        .map(|cell| expression_text(cell).trim().to_owned())
                        .collect::<Vec<_>>()
                        .join(" & ")
                })
                .collect();
            format!("cases({})", rows.join(", "))
        }
        (left, _) => {
            let delimiter = match left {
                "[" => "[",
                "{" => "{",
                "|" => "|",
                "||" => "||",
                _ => "(",
            };
            format!("mat(delim: \"{delimiter}\", {cells})")
        }
    }
}

fn sign_text(sign: &str) -> String {
    match sign {
        "+-" | "pm" => "±",
        "-+" | "mp" => "∓",
        other => other,
    }
    .to_owned()
}

/// A standard function, upright: Typst's own where it has one.
fn function_name(name: &str) -> String {
    const TYPST: &[&str] = &[
        "sin", "cos", "tan", "sinh", "cosh", "tanh", "cot", "sec", "csc", "arcsin", "arccos",
        "arctan", "coth", "exp", "log", "ln", "det", "gcd", "lcm", "sech", "csch",
    ];
    if TYPST.contains(&name) {
        name.to_owned()
    } else if name.chars().count() == 1 {
        // `f` and `g` are functions to asciimath, variables to the reader.
        name.to_owned()
    } else {
        format!("upright({})", quoted(name))
    }
}

/// The source text of an expression, for `text(...)`.
fn source_text(expression: &Expression<'_>) -> String {
    fn simple_source(simple: &Simple<'_>, out: &mut String) {
        match simple {
            Simple::Number(text)
            | Simple::Text(text)
            | Simple::Ident(text)
            | Simple::Operator(text)
            | Simple::Sign(text) => out.push_str(text),
            Simple::Symbol(symbol) => out.push_str(symbol.text),
            Simple::Group(group) => {
                out.push_str(group.left_bracket);
                walk(&group.expr, out);
                out.push_str(group.right_bracket);
            }
            _ => {}
        }
    }
    fn walk(expression: &Expression<'_>, out: &mut String) {
        for intermediate in expression.iter() {
            match intermediate {
                Intermediate::Space(space) => out.push_str(space),
                Intermediate::ScriptFunc(ScriptFunc::Simple(SimpleScript { simple, .. })) => {
                    simple_source(simple, out);
                }
                _ => {}
            }
        }
    }
    let mut out = String::new();
    walk(expression, &mut out);
    out
}

/// A Typst string literal of `text`.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// A character as Typst maths writes it: those with a meaning in maths markup escaped.
fn escaped(character: char) -> String {
    if matches!(
        character,
        '#' | '$'
            | '\\'
            | '_'
            | '^'
            | '&'
            | '"'
            | '/'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | ';'
            | '@'
            | '\''
            | '*'
            | '~'
            | '`'
    ) {
        format!("\\{character}")
    } else {
        character.to_string()
    }
}

/// A symbol's Unicode character, from asciimath's symbol names.
fn symbol_text(name: &str) -> String {
    let glyph = match name {
        // Greek
        "alpha" => "α",
        "beta" => "β",
        "chi" => "χ",
        "delta" => "δ",
        "Delta" => "Δ",
        "epsi" | "epsilon" => "ε",
        "varepsilon" => "ɛ",
        "eta" => "η",
        "gamma" => "γ",
        "Gamma" => "Γ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" | "lamda" => "λ",
        "Lambda" | "Lamda" => "Λ",
        "mu" => "μ",
        "nu" => "ν",
        "omega" => "ω",
        "Omega" => "Ω",
        "phi" => "ϕ",
        "varphi" => "φ",
        "Phi" => "Φ",
        "pi" => "π",
        "Pi" => "Π",
        "psi" => "ψ",
        "Psi" => "Ψ",
        "rho" => "ρ",
        "sigma" => "σ",
        "Sigma" => "Σ",
        "tau" => "τ",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "Theta" => "Θ",
        "upsilon" => "υ",
        "xi" => "ξ",
        "Xi" => "Ξ",
        "zeta" => "ζ",
        // Operations
        "*" | "cdot" => "⋅",
        "**" | "ast" => "∗",
        "***" | "star" => "⋆",
        "//" => "\\/",
        "\\\\" | "backslash" | "setminus" => "∖",
        "xx" | "times" => "×",
        "|><" | "ltimes" => "⋉",
        "><|" | "rtimes" => "⋊",
        "|><|" | "bowtie" => "⋈",
        "-:" | "div" | "divide" => "÷",
        "@" | "circ" => "∘",
        "o+" | "oplus" => "⊕",
        "ox" | "otimes" => "⊗",
        "o." | "odot" => "⊙",
        "^^" | "wedge" => "∧",
        "vv" | "vee" => "∨",
        "nn" | "cap" => "∩",
        "uu" | "cup" => "∪",
        "o-" | "ominus" => "⊖",
        "dag" | "dagger" => "†",
        "ddag" | "ddagger" => "‡",
        // Big operations
        "sum" => "∑",
        "prod" => "∏",
        "^^^" | "bigwedge" => "⋀",
        "vvv" | "bigvee" => "⋁",
        "nnn" | "bigcap" => "⋂",
        "uuu" | "bigcup" => "⋃",
        // Relations
        "=" => "=",
        "!=" | "ne" => "≠",
        ":=" => "≔",
        "<" | "lt" => "<",
        "<=" | "le" | "lt=" | "leq" => "≤",
        ">" | "gt" => ">",
        "mlt" | "ll" => "≪",
        ">=" | "ge" | "gt=" | "geq" => "≥",
        "mgt" | "gg" => "≫",
        "-<" | "prec" | "-lt" => "≺",
        ">-" | "succ" => "≻",
        "-<=" | "preceq" => "⪯",
        ">-=" | "succeq" => "⪰",
        "in" => "∈",
        "!in" | "notin" => "∉",
        "sub" | "subset" => "⊂",
        "sup" | "supset" => "⊃",
        "sube" | "subseteq" => "⊆",
        "supe" | "supseteq" => "⊇",
        "!sub" | "notsubset" => "⊄",
        "!sube" | "notsubseteq" => "⊈",
        "!sup" | "notsupset" => "⊅",
        "!supe" | "notsupseteq" => "⊉",
        "-=" | "equiv" => "≡",
        "!-=" | "notequiv" => "≢",
        "~=" | "cong" => "≅",
        "~~" | "approx" => "≈",
        "~" | "sim" => "∼",
        "prop" | "propto" => "∝",
        // Logic, written as words with space around them
        "and" | "or" | "if" => return format!("space {} space", quoted(name)),
        "not" | "neg" => "¬",
        "AA" | "forall" => "∀",
        "EE" | "exists" => "∃",
        "=>" | "implies" => "⇒",
        "<=>" | "iff" => "⇔",
        "|--" | "vdash" => "⊢",
        "|==" | "models" => "⊨",
        "_|_" | "bot" => "⊥",
        "TT" | "top" => "⊤",
        // Miscellaneous
        ":|:" => "∣",
        ":." | "therefore" => "∴",
        ":'" | "because" => "∵",
        "diamond" => "⋄",
        "frown" => "⌢",
        "int" => "∫",
        "oint" => "∮",
        "\\ " => "space",
        "quad" => "quad",
        "qquad" => "wide",
        "enspace" => "space.en",
        "thinspace" => "thin",
        "del" | "partial" => "∂",
        "grad" | "nabla" => "∇",
        "O/" | "emptyset" => "∅",
        "oo" | "infty" => "∞",
        "aleph" => "ℵ",
        "..." | "ldots" => "…",
        "/_" | "angle" => "∠",
        "/_\\" | "triangle" => "△",
        "'" | "prime" => "′",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "square" => "□",
        "CC" => "ℂ",
        "NN" => "ℕ",
        "QQ" => "ℚ",
        "RR" => "ℝ",
        "ZZ" => "ℤ",
        "hbar" => "ℏ",
        "!" => "!",
        "%" => "%",
        // Inside a call a bare comma would separate arguments.
        "," => "\\,",
        // Limits and the like, Typst's own operators where it has them
        "lim" | "dim" | "min" | "max" | "mod" => name,
        "Lim" | "lub" | "glb" => return format!("upright({})", quoted(name)),
        // Arrows
        "uarr" | "uparrow" => "↑",
        "darr" | "downarrow" => "↓",
        "rarr" | "rightarrow" | "->" | "to" => "→",
        ">->" | "rightarrowtail" => "↣",
        "->>" | "twoheadrightarrow" => "↠",
        ">->>" | "twoheadrightarrowtail" => "⤖",
        "|->" | "mapsto" => "↦",
        "larr" | "leftarrow" => "←",
        "harr" | "leftrightarrow" => "↔",
        "rArr" | "Rightarrow" => "⇒",
        "lArr" | "Leftarrow" => "⇐",
        "hArr" | "Leftrightarrow" => "⇔",
        "dArr" | "Downarrow" => "⇓",
        "rightleftharpoons" => "⇌",
        // Differentials
        "dx" | "dy" | "dz" | "dt" => {
            return format!("dif {}", &name[1..]);
        }
        other => return quoted(other),
    };
    glyph.to_owned()
}
