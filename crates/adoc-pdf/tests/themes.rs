//! The theme loader against asciidoctor-pdf 2.3.15's: `tests/fixtures/pdf/themes/expected`
//! holds the key maps its loader produced for the bundled themes and a custom theme.

use std::path::{Path, PathBuf};

use adoc_pdf::{load_theme, Color, FontFile, LoadedTheme, ThemeError, ThemeSource, ThemeValue};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pdf/themes")
}

fn expected_in(dir: &Path, name: &str) -> Vec<(String, Value)> {
    let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
    let pairs: Vec<(String, Value)> = serde_json::from_str(&text).unwrap();
    pairs
}

/// Whether a loaded value is the one asciidoctor-pdf produced.
fn same(actual: &ThemeValue, expected: &Value) -> bool {
    match (actual, expected) {
        (ThemeValue::Null, Value::Null) => true,
        (ThemeValue::Bool(left), Value::Bool(right)) => left == right,
        (ThemeValue::Number(left), Value::Number(right)) => {
            (left - right.as_f64().unwrap()).abs() < 1e-9
        }
        (ThemeValue::Text(left), Value::String(right)) => left == right,
        (ThemeValue::Color(color), Value::Object(object)) => match color {
            Color::Rgb(_) => object.get("color").and_then(Value::as_str) == color.hex().as_deref(),
            Color::Cmyk(values) => {
                object
                    .get("cmyk")
                    .and_then(Value::as_array)
                    .is_some_and(|cmyk| {
                        cmyk.iter()
                            .zip(values)
                            .all(|(left, right)| (left.as_f64().unwrap() - right).abs() < 1e-9)
                    })
            }
            Color::Transparent => object.contains_key("transparent"),
        },
        // `base` is read as is, so its colours are plain strings there.
        (ThemeValue::Color(color), Value::String(text)) => color.hex().as_deref() == Some(text),
        (ThemeValue::Map(entries), Value::Object(object)) => {
            entries.len() == object.len()
                && entries
                    .iter()
                    .all(|(key, value)| object.get(key).is_some_and(|wanted| same(value, wanted)))
        }
        (ThemeValue::List(items), Value::Array(values)) => {
            items.len() == values.len() && items.iter().zip(values).all(|(l, r)| same(l, r))
        }
        _ => false,
    }
}

fn font_path(file: &FontFile) -> String {
    match file {
        FontFile::Bundled(name) => format!("GEM_FONTS_DIR/{name}"),
        FontFile::Path(path) => path.to_string_lossy().into_owned(),
    }
}

/// Compares every key, the font catalogue and the fallbacks with asciidoctor-pdf's.
fn assert_matches(theme: &LoadedTheme, name: &str) {
    assert_matches_in(theme, name, &fixtures().join("expected"));
}

fn assert_matches_in(theme: &LoadedTheme, name: &str, dir: &Path) {
    let mut mismatches = Vec::new();
    let expected = expected_in(dir, name);
    for (key, value) in &expected {
        match key.as_str() {
            "font_catalog" => {
                let object = value.as_object().unwrap();
                let families: Vec<&str> =
                    theme.fonts.iter().map(|(name, _)| name.as_str()).collect();
                let wanted: Vec<&str> = object.keys().map(String::as_str).collect();
                let mut sorted_families = families.clone();
                sorted_families.sort_unstable();
                let mut sorted_wanted = wanted.clone();
                sorted_wanted.sort_unstable();
                if sorted_families != sorted_wanted {
                    mismatches.push(format!("font_catalog families {families:?} != {wanted:?}"));
                    continue;
                }
                for (family, faces) in &theme.fonts {
                    let styles = &object[family];
                    for (style, file) in [
                        ("normal", &faces.normal),
                        ("bold", &faces.bold),
                        ("italic", &faces.italic),
                        ("bold_italic", &faces.bold_italic),
                    ] {
                        let got = file.as_ref().map(font_path);
                        let want = styles.get(style).and_then(Value::as_str).map(str::to_owned);
                        if got != want {
                            mismatches.push(format!("{family} {style}: {got:?} != {want:?}"));
                        }
                    }
                }
            }
            "font_fallbacks" => {
                let wanted: Vec<String> = value
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                if theme.fallbacks != wanted {
                    mismatches.push(format!("fallbacks {:?} != {wanted:?}", theme.fallbacks));
                }
            }
            _ => match theme.keys.get(key) {
                Some(actual) if same(actual, value) => {}
                actual => mismatches.push(format!("{key}: {actual:?} != {value}")),
            },
        }
    }
    let known: std::collections::HashSet<&str> = expected.iter().map(|(k, _)| k.as_str()).collect();
    for (key, _) in theme.keys.iter() {
        if !known.contains(key) {
            mismatches.push(format!("{key}: not in asciidoctor-pdf's theme"));
        }
    }
    assert!(mismatches.is_empty(), "{name}:\n{}", mismatches.join("\n"));
}

#[test]
fn loads_every_bundled_theme() {
    for name in ["base", "default", "default-sans", "default-for-print"] {
        let theme = load_theme(&ThemeSource::Bundled(name.into())).unwrap();
        assert!(!theme.keys.is_empty(), "{name}");
        assert!(theme.warnings.is_empty(), "{name}: {:?}", theme.warnings);
    }
}

#[test]
fn default_theme_values_match_asciidoctor_pdf() {
    for name in ["base", "default", "default-sans", "default-for-print"] {
        let theme = load_theme(&ThemeSource::Bundled(name.into())).unwrap();
        assert_matches(&theme, name);
    }
}

#[test]
fn custom_theme_loads_without_edits() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    assert_matches(&theme, "acme");
    // The deprecated `blockquote` category is renamed, with a warning, as asciidoctor-pdf does.
    assert!(theme.warnings.iter().any(|warning| warning
        .message
        .contains("blockquote theme category is deprecated")));
}

#[test]
fn extends_applies_in_order() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    // `default` sets the base font colour, `acme-base` replaces it, `acme` keeps it.
    assert_eq!(
        theme.keys.get("base_font_color"),
        Some(&ThemeValue::Color(Color::Rgb([0x22, 0x22, 0x22])))
    );
    // A key evaluated in `default` keeps the value it had then, before `acme` changed the size.
    assert_eq!(
        theme.keys.get("base_font_size_min"),
        Some(&ThemeValue::Number(7.875))
    );
}

#[test]
fn variables_and_arithmetic_evaluate() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    let number = |key: &str| theme.keys.get(key).and_then(ThemeValue::as_number).unwrap();
    assert!((number("base_line_height") - 14.0 / 11.0).abs() < 1e-12);
    assert_eq!(number("base_font_size_large"), 14.0);
    assert_eq!(number("heading_h3_font_size"), 18.5);
    assert_eq!(number("heading_margin_bottom"), -5.5);
    assert_eq!(number("code_border_radius"), 4.0);
}

#[test]
fn units_normalise_to_points() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    let margin: Vec<f64> = theme
        .keys
        .get("page_margin")
        .and_then(ThemeValue::as_list)
        .unwrap()
        .iter()
        .map(|value| value.as_number().unwrap())
        .collect();
    assert_eq!(
        margin,
        vec![54.0, 72.0, 20.0 * (72.0 / 25.4), 2.5 * (720.0 / 25.4)]
    );
    assert_eq!(
        theme.keys.get("role_big_font_size"),
        Some(&ThemeValue::Text("1.2em".into()))
    );
}

#[test]
fn colours_normalise() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    let colour = |key: &str| theme.keys.get(key).cloned();
    assert_eq!(
        colour("title_page_title_font_color"),
        Some(ThemeValue::Color(Color::Rgb([0, 0, 0])))
    );
    assert_eq!(
        colour("page_background_color"),
        Some(ThemeValue::Color(Color::Transparent))
    );
    assert_eq!(colour("codespan_font_color"), Some(ThemeValue::Null));
    assert_eq!(
        colour("role_big-blue_font_color"),
        Some(ThemeValue::Color(Color::Cmyk([100.0, 50.0, 0.0, 0.0])))
    );
}

#[test]
fn font_catalogue_merges_and_resolves() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    let families: Vec<&str> = theme.fonts.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        families,
        ["Noto Serif", "M+ 1mn", "Acme Mono", "Acme Serif"]
    );
    assert_eq!(theme.fallbacks, ["Noto Serif"]);
    assert_eq!(theme.dir.as_deref(), Some(fixtures().as_path()));
}

#[test]
fn unknown_variables_warn_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken-theme.yml");
    std::fs::write(&path, "base:\n  font-size: $nowhere * 2\n").unwrap();
    let theme = load_theme(&ThemeSource::File(path)).unwrap();
    assert!(theme.warnings.iter().any(|warning| warning
        .message
        .contains("unknown variable reference in PDF theme: $nowhere")));
}

#[test]
fn unknown_keys_warn_and_load() {
    let theme = load_theme(&ThemeSource::File(fixtures().join("acme-theme.yml"))).unwrap();
    let unknown: Vec<&str> = theme
        .warnings
        .iter()
        .filter(|warning| warning.message.contains("is not used by the converter"))
        .map(|warning| warning.message.as_str())
        .collect();
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    assert!(unknown[0].contains("`brand`") && unknown[1].contains("`mystery`"));
    // The keys are kept, so variables can still refer to them.
    assert_eq!(
        theme.keys.get("mystery_key"),
        Some(&ThemeValue::Number(1.0))
    );
}

#[test]
fn malformed_theme_reports_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad-theme.yml");
    std::fs::write(&path, "base: [unclosed\n").unwrap();
    assert!(matches!(
        load_theme(&ThemeSource::File(path)),
        Err(ThemeError::Yaml { .. })
    ));
    assert!(matches!(
        load_theme(&ThemeSource::File(dir.path().join("missing-theme.yml"))),
        Err(ThemeError::NotFound(_))
    ));
    assert!(matches!(
        load_theme(&ThemeSource::Bundled("nope".into())),
        Err(ThemeError::UnknownTheme(_))
    ));
}

/// Themes outside the repository against asciidoctor-pdf's key maps for them:
/// `ADOC_PDF_THEME_CORPUS=<themes dir>:<expected dir>`, with `<name>.json` in the second for each
/// `<name>-theme.yml` in the first, written as `expected/README.md` describes.
#[test]
#[ignore = "needs ADOC_PDF_THEME_CORPUS"]
fn corpus_themes_match_asciidoctor_pdf() {
    let corpus = std::env::var("ADOC_PDF_THEME_CORPUS").expect("ADOC_PDF_THEME_CORPUS");
    let (themes, expected) = corpus.split_once(':').expect("<themes dir>:<expected dir>");
    let mut checked = 0;
    for entry in std::fs::read_dir(themes).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix("-theme.yml"))
        else {
            continue;
        };
        let theme = load_theme(&ThemeSource::File(path.clone())).unwrap();
        assert_matches_in(&theme, name, Path::new(expected));
        checked += 1;
    }
    assert!(checked > 0, "no themes in {themes}");
}
