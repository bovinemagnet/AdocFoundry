//! LaTeX to Typst maths through `mitex`, with the few constructs it writes as code or as calls
//! that need its own Typst package turned into plain maths: `\text{…}` becomes a string,
//! `\sqrt[n]{…}` a `root`, and `\operatorname{…}` an `op`.

use crate::MathError;

/// `source` as Typst maths.
pub fn convert(source: &str) -> Result<String, MathError> {
    let converted = mitex::convert_math(source, None).map_err(MathError::new)?;
    let converted = texts_as_strings(&converted)?;
    let converted = roots(&converted);
    Ok(operator_names(&converted))
}

/// `#textmath[…];`, mitex's text in maths, as a Typst string: its escapes undone, then the
/// string's own applied.
fn texts_as_strings(converted: &str) -> Result<String, MathError> {
    const START: &str = "#textmath[";
    let mut out = String::with_capacity(converted.len());
    let mut rest = converted;
    while let Some(start) = rest.find(START) {
        out.push_str(&rest[..start]);
        let mut text = String::new();
        let mut characters = rest[start + START.len()..].char_indices();
        let mut end = None;
        while let Some((index, character)) = characters.next() {
            match character {
                '\\' => {
                    if let Some((_, escaped)) = characters.next() {
                        text.push(escaped);
                    }
                }
                ']' => {
                    end = Some(start + START.len() + index + 1);
                    break;
                }
                other => text.push(other),
            }
        }
        let end = end.ok_or_else(|| MathError::new("text in the formula is not closed"))?;
        out.push('"');
        for character in text.chars() {
            if matches!(character, '"' | '\\') {
                out.push('\\');
            }
            out.push(character);
        }
        out.push('"');
        rest = rest[end..].strip_prefix(';').unwrap_or(&rest[end..]);
    }
    out.push_str(rest);
    Ok(out)
}

/// `mitexsqrt(\[n \],x )` as `root(n ,x )`, and `mitexsqrt(x )` as `sqrt(x )`.
fn roots(converted: &str) -> String {
    const CALL: &str = "mitexsqrt(";
    let mut out = String::with_capacity(converted.len());
    let mut rest = converted;
    while let Some(start) = rest.find(CALL) {
        out.push_str(&rest[..start]);
        let after = &rest[start + CALL.len()..];
        match after.strip_prefix("\\[").and_then(|index| {
            index
                .find("\\],")
                .map(|end| (&index[..end], &index[end + "\\],".len()..]))
        }) {
            Some((degree, tail)) => {
                out.push_str("root(");
                out.push_str(degree);
                out.push(',');
                rest = tail;
            }
            None => {
                out.push_str("sqrt(");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `operatorname(r a n k )`, a name mitex spells out letter by letter, as `op("rank")`, and
/// `operatornamewithlimits(…)` as `limits(op("…"))`.
fn operator_names(converted: &str) -> String {
    let mut out = String::with_capacity(converted.len());
    let mut rest = converted;
    loop {
        let found = ["operatornamewithlimits(", "operatorname("]
            .iter()
            .filter_map(|call| rest.find(call).map(|at| (at, *call)))
            .min();
        let Some((start, call)) = found else {
            break;
        };
        let after = &rest[start + call.len()..];
        let Some(end) = after.find(')') else {
            break;
        };
        let name: String = after[..end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        out.push_str(&rest[..start]);
        let name = format!("op(\"{}\")", name.replace(['"', '\\'], ""));
        if call.starts_with("operatornamewithlimits") {
            out.push_str(&format!("limits({name})"));
        } else {
            out.push_str(&name);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}
