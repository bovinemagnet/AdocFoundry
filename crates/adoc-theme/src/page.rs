//! A page's size, orientation and margins from a theme and a document's `pdf-page-size`,
//! `pdf-page-layout` and `pdf-page-margin`, as asciidoctor-pdf reads them: shared by PDF export
//! and Word export, so both give a page the same paper.

use std::collections::BTreeMap;

use crate::{LoadedTheme, ThemeValue};

/// A page's size, in points, its margins and its orientation, as asciidoctor-pdf works them out.
#[derive(Clone, Debug, PartialEq)]
pub struct PageGeometry {
    pub width: f64,
    pub height: f64,
    /// Top, right, bottom, left.
    pub margin: [f64; 4],
    pub landscape: bool,
    /// A size by a name that is not known, given as A4.
    pub warnings: Vec<String>,
}

/// Page sizes asciidoctor-pdf knows by name, in points: Prawn's `PageGeometry::SIZES`.
const PAPER: &[(&str, f64, f64)] = &[
    ("4A0", 4767.87, 6740.79),
    ("2A0", 3370.39, 4767.87),
    ("A0", 2383.94, 3370.39),
    ("A1", 1683.78, 2383.94),
    ("A2", 1190.55, 1683.78),
    ("A3", 841.89, 1190.55),
    ("A4", 595.28, 841.89),
    ("A5", 419.53, 595.28),
    ("A6", 297.64, 419.53),
    ("A7", 209.76, 297.64),
    ("A8", 147.40, 209.76),
    ("A9", 104.88, 147.40),
    ("A10", 73.70, 104.88),
    ("B0", 2834.65, 4008.19),
    ("B1", 2004.09, 2834.65),
    ("B2", 1417.32, 2004.09),
    ("B3", 1000.63, 1417.32),
    ("B4", 708.66, 1000.63),
    ("B5", 498.90, 708.66),
    ("B6", 354.33, 498.90),
    ("B7", 249.45, 354.33),
    ("B8", 175.75, 249.45),
    ("B9", 124.72, 175.75),
    ("B10", 87.87, 124.72),
    ("C0", 2599.37, 3676.54),
    ("C1", 1836.85, 2599.37),
    ("C2", 1298.27, 1836.85),
    ("C3", 918.43, 1298.27),
    ("C4", 649.13, 918.43),
    ("C5", 459.21, 649.13),
    ("C6", 323.15, 459.21),
    ("C7", 229.61, 323.15),
    ("C8", 161.57, 229.61),
    ("C9", 113.39, 161.57),
    ("C10", 79.37, 113.39),
    ("RA0", 2437.80, 3458.27),
    ("RA1", 1729.13, 2437.80),
    ("RA2", 1218.90, 1729.13),
    ("RA3", 864.57, 1218.90),
    ("RA4", 609.45, 864.57),
    ("SRA0", 2551.18, 3628.35),
    ("SRA1", 1814.17, 2551.18),
    ("SRA2", 1275.59, 1814.17),
    ("SRA3", 907.09, 1275.59),
    ("SRA4", 637.80, 907.09),
    ("EXECUTIVE", 521.86, 756.00),
    ("FOLIO", 612.00, 936.00),
    ("LEGAL", 612.00, 1008.00),
    ("LETTER", 612.00, 792.00),
    ("TABLOID", 792.00, 1224.00),
];

/// The page's size and margins: the document's `pdf-page-size`, `pdf-page-layout` and
/// `pdf-page-margin`, else the theme's `page_size`, `page_layout` and `page_margin`, as
/// asciidoctor-pdf reads them. A size by a name neither knows is A4, with a warning.
#[must_use]
pub fn page_geometry(theme: &LoadedTheme, attributes: &BTreeMap<String, String>) -> PageGeometry {
    let keys = &theme.keys;
    let attribute = |name: &str| {
        attributes
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let mut warnings = Vec::new();
    let mut unknown = |size: &str| {
        warnings.push(format!("unknown page size {size}; A4 is used"));
        A4
    };
    let (mut width, mut height) = match attribute("pdf-page-size") {
        Some(size) => page_size(size).unwrap_or_else(|| unknown(size)),
        None => match keys.get("page_size") {
            Some(ThemeValue::Text(name)) => paper(name).unwrap_or_else(|| unknown(name)),
            Some(ThemeValue::List(items)) if items.len() == 2 => (
                items[0].as_number().unwrap_or(A4.0),
                items[1].as_number().unwrap_or(A4.1),
            ),
            _ => A4,
        },
    };
    let landscape = match attribute("pdf-page-layout") {
        Some(layout) => layout == "landscape",
        None => {
            matches!(keys.get("page_layout"), Some(ThemeValue::Text(layout)) if layout == "landscape")
        }
    };
    if landscape {
        std::mem::swap(&mut width, &mut height);
    }
    let margin = match attribute("pdf-page-margin") {
        Some(margin) => sides(Some(&page_margin(margin)), 36.0),
        None => sides(keys.get("page_margin"), 36.0),
    };
    PageGeometry {
        width,
        height,
        margin,
        landscape,
        warnings,
    }
}

const A4: (f64, f64) = (595.28, 841.89);

/// A page size by name, in either case.
fn paper(name: &str) -> Option<(f64, f64)> {
    PAPER
        .iter()
        .find(|(paper, _, _)| paper.eq_ignore_ascii_case(name.trim()))
        .map(|(_, width, height)| (*width, *height))
}

/// A `pdf-page-size`: a name (`Letter`), `[8.5in, 11in]` or `8.5in x 11in`; `None` for a
/// size asciidoctor-pdf would not read.
fn page_size(value: &str) -> Option<(f64, f64)> {
    let pair = value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .and_then(|inner| inner.split_once(','))
        .or_else(|| value.split_once('x'))
        .and_then(|(width, height)| Some((measurement(width)?, measurement(height)?)));
    match pair {
        Some((width, height)) => (width > 0.0 && height > 0.0).then_some((width, height)),
        // A name, `Executive` among them.
        None => paper(value),
    }
}

/// A length with an optional unit (`in`, `mm`, `cm`, `pt`, `px`, `pc`), in points.
fn measurement(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    crate::to_pt(number.parse().ok()?, unit).filter(|points| points.is_finite())
}

/// A `pdf-page-margin`: one length, or up to four in brackets (`[0.5in, 0.67in]`), as
/// asciidoctor-pdf reads it; a value it cannot read counts as 0, as there.
fn page_margin(value: &str) -> ThemeValue {
    let points = |part: &str| ThemeValue::Number(measurement(part).unwrap_or(0.0));
    match value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) => ThemeValue::List(inner.split(',').take(4).map(points).collect()),
        None => points(value),
    }
}

/// A CSS-style shorthand of one to four numbers as top, right, bottom, left.
#[must_use]
pub fn sides(value: Option<&ThemeValue>, default: f64) -> [f64; 4] {
    let number = |value: &ThemeValue| value.as_number().unwrap_or(default);
    match value {
        Some(ThemeValue::Number(value)) => [*value; 4],
        Some(ThemeValue::List(items)) => match items.as_slice() {
            [all] => [number(all); 4],
            [vertical, horizontal] => {
                let (v, h) = (number(vertical), number(horizontal));
                [v, h, v, h]
            }
            [top, horizontal, bottom] => {
                let h = number(horizontal);
                [number(top), h, number(bottom), h]
            }
            [top, right, bottom, left, ..] => {
                [number(top), number(right), number(bottom), number(left)]
            }
            [] => [default; 4],
        },
        _ => [default; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::measurement;

    #[test]
    fn measurements_are_an_unsigned_number_and_a_theme_unit() {
        assert_eq!(measurement("1in"), Some(72.0));
        assert_eq!(measurement("8.5in"), Some(612.0));
        assert_eq!(measurement(".5in"), Some(36.0));
        assert_eq!(measurement("2pc"), Some(24.0));
        assert_eq!(measurement("4px"), Some(3.0));
        assert_eq!(measurement("10pt"), Some(10.0));
        assert!((measurement("25.4mm").unwrap() - 72.0).abs() < 1e-9);
        assert!((measurement("2.54cm").unwrap() - 72.0).abs() < 1e-9);
        // No unit is points.
        assert_eq!(measurement("10"), Some(10.0));
        assert_eq!(measurement("0"), Some(0.0));
        // Space around the whole is trimmed; space before the unit is not.
        assert_eq!(measurement(" 1in "), Some(72.0));
        assert_eq!(measurement("1 in"), None);
        // An unknown unit, a sign, no number or two points.
        assert_eq!(measurement("10em"), None);
        assert_eq!(measurement("10%"), None);
        assert_eq!(measurement("-1in"), None);
        assert_eq!(measurement("+1in"), None);
        assert_eq!(measurement("in"), None);
        assert_eq!(measurement(""), None);
        assert_eq!(measurement("1.2.3in"), None);
        assert_eq!(measurement("1e2"), None);
    }
}
