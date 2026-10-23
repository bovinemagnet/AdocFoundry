//! Theme values and their evaluation: variables, arithmetic, measurements and colours, as
//! asciidoctor-pdf 2.3's theme loader evaluates them.

use std::{fmt, sync::OnceLock};

use regex::Regex;

/// One theme value, after evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum ThemeValue {
    Null,
    Bool(bool),
    /// A number; measurements written with absolute units are already in points.
    Number(f64),
    /// Text, including relative lengths (`1.2em`, `10%`, `50vw`) and running content.
    Text(String),
    Color(Color),
    List(Vec<ThemeValue>),
    /// The settings of one admonition icon (`admonition_icon_note`), kept together as
    /// asciidoctor-pdf keeps them.
    Map(Vec<(String, ThemeValue)>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Rgb([u8; 3]),
    /// Percentages, 0 to 100.
    Cmyk([f64; 4]),
    Transparent,
}

impl Color {
    /// The colour as RGB, CMYK converted naively.
    #[must_use]
    pub fn rgb(&self) -> Option<[u8; 3]> {
        match self {
            Self::Rgb(rgb) => Some(*rgb),
            Self::Cmyk([c, m, y, k]) => {
                let channel =
                    |value: f64| (255.0 * (1.0 - value / 100.0) * (1.0 - k / 100.0)).round() as u8;
                Some([channel(*c), channel(*m), channel(*y)])
            }
            Self::Transparent => None,
        }
    }

    /// Six hex digits for an RGB colour.
    #[must_use]
    pub fn hex(&self) -> Option<String> {
        match self {
            Self::Rgb([r, g, b]) => Some(format!("{r:02X}{g:02X}{b:02X}")),
            _ => None,
        }
    }
}

impl fmt::Display for ThemeValue {
    /// The value as Ruby would interpolate it into a string.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => Ok(()),
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Number(value) => formatter.write_str(&number_text(*value)),
            Self::Text(text) => formatter.write_str(text),
            Self::Color(Color::Rgb(_)) => {
                formatter.write_str(&self.as_color().and_then(Color::hex).unwrap_or_default())
            }
            Self::Color(Color::Cmyk(values)) => {
                let parts: Vec<String> = values.iter().map(|value| number_text(*value)).collect();
                write!(formatter, "[{}]", parts.join(", "))
            }
            Self::Color(Color::Transparent) => formatter.write_str("transparent"),
            Self::List(items) => {
                let parts: Vec<String> = items.iter().map(ToString::to_string).collect();
                write!(formatter, "[{}]", parts.join(", "))
            }
            Self::Map(entries) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect();
                write!(formatter, "{{{}}}", parts.join(", "))
            }
        }
    }
}

impl ThemeValue {
    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_color(&self) -> Option<&Color> {
        match self {
            Self::Color(color) => Some(color),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[ThemeValue]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// An entry of a map value.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<&ThemeValue> {
        match self {
            Self::Map(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// A number as Ruby prints the integers and floats the loader produces: whole values without a
/// fraction.
pub(crate) fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Ruby's `String#to_f`: the leading decimal number, or 0.
pub(crate) fn ruby_to_f(text: &str) -> f64 {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let number =
        NUMBER.get_or_init(|| Regex::new(r"^\s*[-+]?(\d+(\.\d+)?|\.\d+)([eE][-+]?\d+)?").unwrap());
    number
        .find(text)
        .and_then(|found| found.as_str().trim().parse().ok())
        .unwrap_or(0.0)
}

/// Ruby's `String#to_i`: the leading integer, or 0.
fn ruby_to_i(text: &str) -> f64 {
    static INTEGER: OnceLock<Regex> = OnceLock::new();
    let integer = INTEGER.get_or_init(|| Regex::new(r"^\s*[-+]?\d+").unwrap());
    integer
        .find(text)
        .and_then(|found| found.as_str().trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Converts `value` from `unit` to points, as asciidoctor-pdf's `to_pt` does.
#[must_use]
pub fn to_pt(value: f64, unit: &str) -> Option<f64> {
    Some(match unit {
        "" | "pt" => value,
        "in" => value * 72.0,
        "mm" => value * (72.0 / 25.4),
        "cm" => value * (720.0 / 25.4),
        // A 96 dpi canvas.
        "px" => value * 0.75,
        "pc" => value * 12.0,
        _ => return None,
    })
}

/// Replaces every measurement in `expr` that stands alone (`0.5in`, `(2cm`) with its value in
/// points.
fn resolve_measurements(expr: &str) -> String {
    static HINT: OnceLock<Regex> = OnceLock::new();
    static VALUE: OnceLock<Regex> = OnceLock::new();
    let hint = HINT.get_or_init(|| Regex::new(r"\d(in|mm|cm|p[txc])").unwrap());
    if !hint.is_match(expr) {
        return expr.to_owned();
    }
    let value = VALUE.get_or_init(|| Regex::new(r"(-?\d+(?:\.\d+)?)(in|mm|cm|p[txc])").unwrap());
    let mut out = String::new();
    let mut last = 0;
    for captures in value.captures_iter(expr) {
        let whole = captures.get(0).unwrap();
        let before = expr[..whole.start()].chars().next_back();
        let after = expr[whole.end()..].chars().next();
        let bounded = before.is_none_or(|c| c == ' ' || c == '(')
            && after.is_none_or(|c| c == ' ' || c == ')');
        if !bounded {
            continue;
        }
        let number: f64 = captures[1].parse().unwrap_or(0.0);
        let points = to_pt(number, &captures[2]).unwrap_or(number);
        out.push_str(&expr[last..whole.start()]);
        out.push_str(&float_text(points));
        last = whole.end();
    }
    out.push_str(&expr[last..]);
    out
}

/// A float as Ruby's `Float#to_s` would substitute it into an expression.
fn float_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// Evaluates the arithmetic in `expr` as asciidoctor-pdf's `evaluate_math` does: measurements
/// to points, then `*`, `/` and `^` left to right, then `+` and `-`, then a `round()`,
/// `floor()` or `ceil()` around the whole. An expression with nothing to evaluate stays text.
pub(crate) fn evaluate_math(expr: &str) -> ThemeValue {
    static MULTIPLY: OnceLock<Regex> = OnceLock::new();
    static ADD: OnceLock<Regex> = OnceLock::new();
    static PRECISION: OnceLock<Regex> = OnceLock::new();
    let multiply = MULTIPLY
        .get_or_init(|| Regex::new(r"(-?\d+(?:\.\d+)?) +([*/^]) +(-?\d+(?:\.\d+)?)").unwrap());
    let add =
        ADD.get_or_init(|| Regex::new(r"(-?\d+(?:\.\d+)?) +([+-]) +(-?\d+(?:\.\d+)?)").unwrap());
    let precision = PRECISION.get_or_init(|| Regex::new(r"^(round|floor|ceil)\(").unwrap());

    let original = expr;
    let mut expr = resolve_measurements(expr);
    let apply = |regex: &Regex, expr: &str| {
        regex
            .replace_all(expr, |captures: &regex::Captures<'_>| {
                let left: f64 = captures[1].parse().unwrap_or(0.0);
                let right: f64 = captures[3].parse().unwrap_or(0.0);
                let result = match &captures[2] {
                    "*" => left * right,
                    "/" => left / right,
                    "^" => left.powf(right),
                    "+" => left + right,
                    _ => left - right,
                };
                float_text(result)
            })
            .into_owned()
    };
    if expr.contains(['*', '/', '^']) {
        loop {
            let result = apply(multiply, &expr);
            if result == expr {
                break;
            }
            expr = result;
        }
    }
    if expr.contains(['+', '-']) {
        loop {
            let result = apply(add, &expr);
            if result == expr {
                break;
            }
            expr = result;
        }
    }
    let mut number = None;
    if expr.ends_with(')') {
        if let Some(found) = precision.captures(&expr) {
            let op = found[1].to_owned();
            let inner = ruby_to_f(&expr[op.len() + 1..expr.len() - 1]);
            number = Some(match op.as_str() {
                "round" => inner.round(),
                "floor" => inner.floor(),
                _ => inner.ceil(),
            });
        }
    }
    if let Some(number) = number {
        return ThemeValue::Number(number);
    }
    if expr == original {
        return ThemeValue::Text(original.to_owned());
    }
    let (integer, float) = (ruby_to_i(&expr), ruby_to_f(&expr));
    ThemeValue::Number(if integer == float { integer } else { float })
}

/// Normalises a colour as asciidoctor-pdf's `to_color` does. `None` for a null colour.
pub(crate) fn to_color(value: &ThemeValue) -> Option<Color> {
    let text = match value {
        ThemeValue::Color(color) => return Some(color.clone()),
        ThemeValue::Null => return None,
        ThemeValue::List(items) if items.len() == 4 => {
            let values: Vec<f64> = items
                .iter()
                .map(|item| match item {
                    ThemeValue::Number(number) if *number <= 1.0 => number * 100.0,
                    ThemeValue::Number(number) => *number,
                    other => ruby_to_f(other.to_string().trim_end_matches('%')),
                })
                .collect();
            let values = [values[0], values[1], values[2], values[3]];
            return Some(match values {
                [0.0, 0.0, 0.0, 0.0] => Color::Rgb([255, 255, 255]),
                [100.0, 100.0, 100.0, 100.0] => Color::Rgb([0, 0, 0]),
                values => Color::Cmyk(values),
            });
        }
        ThemeValue::List(items) if items.len() == 3 => {
            let channel =
                |item: &ThemeValue| item.as_number().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
            return Some(Color::Rgb([
                channel(&items[0]),
                channel(&items[1]),
                channel(&items[2]),
            ]));
        }
        ThemeValue::List(items) => items.iter().map(ToString::to_string).collect::<String>(),
        ThemeValue::Text(text) if text == "transparent" => return Some(Color::Transparent),
        other => other.to_string(),
    };
    // A leading `#` is accepted wherever it appears, not only where the line rewrite finds it.
    let text = text.trim().trim_start_matches('#');
    let hex = match text.len() {
        6 => text.to_owned(),
        3 => text.chars().flat_map(|c| [c, c]).collect(),
        _ => {
            let head: String = text.chars().take(6).collect();
            format!("{head:0>6}")
        }
    };
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok();
    Some(Color::Rgb([channel(0..2)?, channel(2..4)?, channel(4..6)?]))
}

#[cfg(test)]
mod tests {
    use super::{evaluate_math, to_color, Color, ThemeValue};

    fn number(expr: &str) -> f64 {
        match evaluate_math(expr) {
            ThemeValue::Number(value) => value,
            other => panic!("{expr} gave {other:?}"),
        }
    }

    #[test]
    fn arithmetic_evaluates_as_asciidoctor_pdf_does() {
        assert_eq!(number("12 / 10.5"), 12.0 / 10.5);
        assert_eq!(number("round(10.5 * 1.25)"), 13.0);
        assert_eq!(number("floor(10.5 * 2.6)"), 27.0);
        assert_eq!(number("ceil(10.5)"), 11.0);
        assert_eq!(number("11 * 1.5 + 2"), 18.5);
        assert_eq!(number("-11 / 2"), -5.5);
        assert_eq!(number("2 ^ 2"), 4.0);
        assert_eq!(number("2 * 3 * 4"), 24.0);
        assert_eq!(number("12 + 6 / 2"), 15.0);
    }

    #[test]
    fn measurements_become_points() {
        assert_eq!(number("0.5in"), 36.0);
        assert_eq!(number("0.5in / 4"), 9.0);
        assert_eq!(number("6pt * 2"), 12.0);
        assert_eq!(number("20mm"), 20.0 * (72.0 / 25.4));
        assert_eq!(number("2.5cm"), 2.5 * (720.0 / 25.4));
        assert_eq!(number("16px"), 12.0);
        assert_eq!(evaluate_math("1.2em"), ThemeValue::Text("1.2em".into()));
        assert_eq!(evaluate_math("10%"), ThemeValue::Text("10%".into()));
        assert_eq!(evaluate_math("FitH"), ThemeValue::Text("FitH".into()));
    }

    #[test]
    fn colours_normalise() {
        let text = |value: &str| to_color(&ThemeValue::Text(value.into()));
        assert_eq!(text("333333"), Some(Color::Rgb([0x33, 0x33, 0x33])));
        assert_eq!(text("f00"), Some(Color::Rgb([255, 0, 0])));
        assert_eq!(text("#B12146"), Some(Color::Rgb([0xB1, 0x21, 0x46])));
        assert_eq!(text("ff"), Some(Color::Rgb([0, 0, 255])));
        assert_eq!(text("transparent"), Some(Color::Transparent));
        assert_eq!(
            to_color(&ThemeValue::Number(0.0)),
            Some(Color::Rgb([0, 0, 0]))
        );
        assert_eq!(
            to_color(&ThemeValue::Number(181_818.0)),
            Some(Color::Rgb([0x18, 0x18, 0x18]))
        );
        let list = |items: &[f64]| {
            to_color(&ThemeValue::List(
                items.iter().map(|item| ThemeValue::Number(*item)).collect(),
            ))
        };
        assert_eq!(
            list(&[204.0, 204.0, 204.0]),
            Some(Color::Rgb([204, 204, 204]))
        );
        assert_eq!(
            list(&[1.0, 0.5, 0.0, 0.0]),
            Some(Color::Cmyk([100.0, 50.0, 0.0, 0.0]))
        );
        assert_eq!(
            list(&[0.0, 0.0, 0.0, 0.0]),
            Some(Color::Rgb([255, 255, 255]))
        );
        assert_eq!(to_color(&ThemeValue::Null), None);
    }
}
