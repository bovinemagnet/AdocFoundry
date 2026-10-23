//! asciidoctor-pdf themes: YAML files with inheritance, variables, arithmetic and units,
//! loaded as asciidoctor-pdf 2.3's `ThemeLoader` loads them, so a theme written for
//! asciidoctor-pdf loads here without edits.

mod value;
mod yaml;

use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use regex::Regex;

use value::{evaluate_math, to_color};

/// The leading number in `text`, as Ruby's `String#to_f` reads it.
pub(crate) fn value_number(text: &str) -> f64 {
    value::ruby_to_f(text)
}
pub use value::{to_pt, Color, ThemeValue};
use yaml::Yaml;

/// The themes bundled with asciidoctor-pdf that are bundled here, by name.
const BUNDLED_THEMES: &[(&str, &str)] = &[
    ("base", include_str!("../../themes/base-theme.yml")),
    ("default", include_str!("../../themes/default-theme.yml")),
    (
        "default-sans",
        include_str!("../../themes/default-sans-theme.yml"),
    ),
    (
        "default-for-print",
        include_str!("../../themes/default-for-print-theme.yml"),
    ),
];

/// asciidoctor-pdf's other bundled themes add fallback fonts that are not bundled here; they
/// load as the theme they add the fonts to, with a warning.
const FALLBACK_THEMES: &[(&str, &str)] = &[
    ("default-with-font-fallbacks", "default"),
    ("default-with-fallback-font", "default"),
    ("default-sans-with-font-fallbacks", "default-sans"),
    ("default-for-print-with-font-fallbacks", "default-for-print"),
    ("default-for-print-with-fallback-font", "default-for-print"),
];

/// Where a theme comes from.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ThemeSource {
    /// A bundled theme by name: `default`, `default-sans`, `default-for-print` or `base`.
    Bundled(String),
    /// A theme file.
    File(PathBuf),
}

impl ThemeSource {
    /// A theme named as asciidoctor-pdf's `pdf-theme` attribute names one: a path when it ends
    /// in `.yml`, a bundled theme otherwise, or `<name>-theme.yml` in `themes_dir` when given.
    #[must_use]
    pub fn named(name: &str, themes_dir: Option<&Path>) -> Self {
        if name.ends_with(".yml") {
            let path = Path::new(name);
            return Self::File(match themes_dir {
                Some(dir) if path.is_relative() => dir.join(path),
                _ => path.to_path_buf(),
            });
        }
        match themes_dir {
            Some(dir) if !is_bundled(name) => Self::File(dir.join(format!("{name}-theme.yml"))),
            _ => Self::Bundled(name.to_owned()),
        }
    }
}

fn is_bundled(name: &str) -> bool {
    BUNDLED_THEMES.iter().any(|(bundled, _)| *bundled == name)
        || FALLBACK_THEMES.iter().any(|(bundled, _)| *bundled == name)
}

/// A font file named by a theme's catalogue.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FontFile {
    /// One of the bundled fonts (`GEM_FONTS_DIR/<name>`).
    Bundled(String),
    /// A path, absolute or relative to the fonts directories.
    Path(PathBuf),
}

/// The four styles of one font family.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FontFaces {
    pub normal: Option<FontFile>,
    pub bold: Option<FontFile>,
    pub italic: Option<FontFile>,
    pub bold_italic: Option<FontFile>,
}

/// `font_catalog`: family name to its faces, in definition order.
pub type FontCatalog = Vec<(String, FontFaces)>;

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeWarning {
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ThemeError {
    NotFound(PathBuf),
    UnknownTheme(String),
    Read {
        path: PathBuf,
        message: String,
    },
    Yaml {
        path: Option<PathBuf>,
        message: String,
    },
}

impl fmt::Display for ThemeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(formatter, "theme {} not found", path.display()),
            Self::UnknownTheme(name) => write!(formatter, "no bundled theme named `{name}`"),
            Self::Read { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::Yaml { path, message } => match path {
                Some(path) => write!(formatter, "{}: {message}", path.display()),
                None => formatter.write_str(message),
            },
        }
    }
}

impl std::error::Error for ThemeError {}

/// The flattened theme keys, in the order they were first defined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeKeys {
    entries: Vec<(String, ThemeValue)>,
    index: HashMap<String, usize>,
}

impl ThemeKeys {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ThemeValue> {
        self.index.get(key).map(|index| &self.entries[*index].1)
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// Sets `key`, keeping its position when it is already defined.
    pub fn insert(&mut self, key: String, value: ThemeValue) {
        match self.index.get(&key) {
            Some(index) => self.entries[*index].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ThemeValue)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sets `key` unless it holds a value already, as Ruby's `||=` does.
    fn insert_default(&mut self, key: &str, value: ThemeValue) {
        if self.get(key).is_none_or(ThemeValue::is_null) {
            self.insert(key.to_owned(), value);
        }
    }
}

/// A theme, loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedTheme {
    pub keys: ThemeKeys,
    pub fonts: FontCatalog,
    /// `font_fallbacks`: families tried for glyphs the font in use lacks.
    pub fallbacks: Vec<String>,
    /// The directory relative theme assets (images, fonts) resolve against.
    pub dir: Option<PathBuf>,
    pub warnings: Vec<ThemeWarning>,
}

/// Loads a theme and everything it extends.
pub fn load_theme(source: &ThemeSource) -> Result<LoadedTheme, ThemeError> {
    let mut loader = Loader::default();
    // A theme starts from nothing but the base font size, as in asciidoctor-pdf.
    loader
        .keys
        .insert("base_font_size".to_owned(), ThemeValue::Number(12.0));
    let (file, dir) = match source {
        ThemeSource::Bundled(name) => (ThemeFile::Bundled(loader.bundled_name(name)?), None),
        ThemeSource::File(path) => {
            if !path.is_file() {
                return Err(ThemeError::NotFound(path.clone()));
            }
            let dir = path.parent().map(Path::to_path_buf);
            (ThemeFile::Path(path.clone()), dir)
        }
    };
    if matches!(&file, ThemeFile::Bundled(name) if name == "base") {
        loader.load_base();
    } else {
        loader.load_file(&file, dir.as_deref())?;
    }
    if let ThemeFile::Path(_) = file {
        // A theme outside the bundled directory gets the defaults asciidoctor-pdf fills in.
        let keys = &mut loader.keys;
        keys.insert_default("base_text_align", ThemeValue::Text("left".into()));
        keys.insert_default("base_line_height", ThemeValue::Number(1.0));
        keys.insert_default("base_font_color", ThemeValue::Color(Color::Rgb([0, 0, 0])));
        let codespan = keys
            .get("codespan_font_family")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| ThemeValue::Text("Courier".into()));
        keys.insert_default("code_font_family", codespan.clone());
        keys.insert_default("conum_font_family", codespan);
        if let Some(heading) = keys
            .get("heading_font_family")
            .filter(|v| !v.is_null())
            .cloned()
        {
            keys.insert_default("abstract_title_font_family", heading.clone());
            keys.insert_default("sidebar_title_font_family", heading);
        }
    }
    let unknown: Vec<String> = unknown_categories(&loader.keys);
    for category in unknown {
        loader.warnings.push(ThemeWarning {
            path: None,
            message: format!(
                "theme category `{category}` is not used by the converter (its keys can still be referred to as variables)"
            ),
        });
    }
    Ok(LoadedTheme {
        keys: loader.keys,
        fonts: loader.fonts,
        fallbacks: loader.fallbacks,
        dir,
        warnings: loader.warnings,
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ThemeFile {
    Bundled(String),
    Path(PathBuf),
}

#[derive(Default)]
struct Loader {
    keys: ThemeKeys,
    fonts: FontCatalog,
    fallbacks: Vec<String>,
    warnings: Vec<ThemeWarning>,
    loaded: HashSet<ThemeFile>,
    /// The file being read, for warnings.
    current: Option<PathBuf>,
}

/// The theme categories asciidoctor-pdf's converter reads.
const KNOWN_CATEGORIES: &[&str] = &[
    "abstract",
    "admonition",
    "base",
    "block",
    "button",
    "callout_list",
    "caption",
    "code",
    "codespan",
    "conum",
    "cover",
    "description_list",
    "example",
    "footer",
    "footnotes",
    "header",
    "heading",
    "horizontal_rhythm",
    "image",
    "index",
    "kbd",
    "link",
    "list",
    "mark",
    "menu",
    "olist",
    "page",
    "page_numbering",
    "prose",
    "quote",
    "role",
    "running_content",
    "sidebar",
    "svg",
    "table",
    "thematic_break",
    "title_page",
    "toc",
    "ulist",
    "verse",
    "vertical_rhythm",
];

/// The categories of `keys` the converter does not read, each once, in order.
fn unknown_categories(keys: &ThemeKeys) -> Vec<String> {
    let mut unknown: Vec<String> = Vec::new();
    for (key, _) in keys.iter() {
        let known = KNOWN_CATEGORIES
            .iter()
            .any(|category| key == *category || key.starts_with(&format!("{category}_")));
        if !known {
            let category = key.split('_').next().unwrap_or(key).to_owned();
            if !unknown.contains(&category) {
                unknown.push(category);
            }
        }
    }
    unknown
}

/// Categories asciidoctor-pdf renamed, with their new names.
const DEPRECATED_CATEGORIES: &[(&str, &str)] = &[
    ("blockquote", "quote"),
    ("key", "kbd"),
    ("literal", "codespan"),
    ("outline_list", "list"),
];

/// Keys whose padding hack asciidoctor-pdf undoes: a bottom padding of zero or less takes the
/// top padding.
const PADDING_HACK_KEYS: &[&str] = &[
    "example_padding",
    "quote_padding",
    "sidebar_padding",
    "verse_padding",
];

/// Prefixes whose `_align` key became `_text_align`.
const ALIGN_PREFIXES: &[&str] = &[
    "base",
    "heading",
    "heading_h1",
    "heading_h2",
    "heading_h3",
    "heading_h4",
    "heading_h5",
    "heading_h6",
    "title_page",
    "abstract",
    "abstract_title",
    "admonition_label",
    "sidebar_title",
    "toc_title",
];

fn deprecated_key(key: &str) -> Option<String> {
    if key == "table_caption_side" {
        return Some("table_caption_end".to_owned());
    }
    let prefix = key.strip_suffix("_align")?;
    if ALIGN_PREFIXES.contains(&prefix) {
        return Some(format!("{prefix}_text_align"));
    }
    if key.starts_with("role_") && !key.ends_with("_text_align") {
        return Some(format!("{prefix}_text_align"));
    }
    None
}

impl Loader {
    fn warn(&mut self, message: String) {
        self.warnings.push(ThemeWarning {
            path: self.current.clone(),
            message,
        });
    }

    fn bundled_name(&mut self, name: &str) -> Result<String, ThemeError> {
        if BUNDLED_THEMES.iter().any(|(bundled, _)| *bundled == name) {
            return Ok(name.to_owned());
        }
        match FALLBACK_THEMES.iter().find(|(bundled, _)| *bundled == name) {
            Some((_, instead)) => {
                self.warn(format!(
                    "the `{name}` theme's fallback fonts are not bundled; using `{instead}`"
                ));
                Ok((*instead).to_owned())
            }
            None => Err(ThemeError::UnknownTheme(name.to_owned())),
        }
    }

    /// `base` is read as is: its keys are already flat and its values are not evaluated.
    fn load_base(&mut self) {
        let text = BUNDLED_THEMES[0].1;
        let Ok(Yaml::Map(entries)) = yaml::parse(text) else {
            return;
        };
        for (key, value) in entries {
            let value = plain(value);
            let value = if key.ends_with("_color") {
                to_color(&value).map_or(ThemeValue::Null, ThemeValue::Color)
            } else {
                value
            };
            self.keys.insert(key, value);
        }
    }

    fn load_file(&mut self, file: &ThemeFile, theme_dir: Option<&Path>) -> Result<(), ThemeError> {
        self.loaded.insert(file.clone());
        let previous = self.current.clone();
        let (text, path) = match file {
            ThemeFile::Bundled(name) => {
                let text = BUNDLED_THEMES
                    .iter()
                    .find(|(bundled, _)| bundled == name)
                    .map(|(_, text)| (*text).to_owned())
                    .ok_or_else(|| ThemeError::UnknownTheme(name.clone()))?;
                (text, None)
            }
            ThemeFile::Path(path) => {
                let text = std::fs::read_to_string(path).map_err(|error| ThemeError::Read {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
                // Hex colours are quoted so YAML keeps them as written, as asciidoctor-pdf does
                // for every theme outside its own directory.
                (
                    quote_hex_colours(&text.replace("\r\n", "\n")),
                    Some(path.clone()),
                )
            }
        };
        self.current = path.clone();
        let parsed = yaml::parse(&text).map_err(|message| ThemeError::Yaml {
            path: path.clone(),
            message,
        })?;
        let mut entries = match parsed {
            Yaml::Map(entries) => entries,
            Yaml::Null => Vec::new(),
            _ => {
                return Err(ThemeError::Yaml {
                    path,
                    message: "a theme must be a mapping of keys".to_owned(),
                })
            }
        };
        if let Some(position) = entries.iter().position(|(key, _)| key == "extends") {
            let (_, extends) = entries.remove(position);
            let extends: Vec<String> = match extends {
                Yaml::Seq(items) => items
                    .into_iter()
                    .map(|item| plain(item).to_string())
                    .collect(),
                Yaml::Null => Vec::new(),
                other => vec![plain(other).to_string()],
            };
            for extend in extends {
                let (extend, force) = match extend.strip_suffix(" !important") {
                    Some(name) => (name.to_owned(), true),
                    None => (extend, false),
                };
                if extend == "base" {
                    if self.loaded.insert(ThemeFile::Bundled("base".into())) || force {
                        self.load_base();
                    }
                    continue;
                }
                let (target, dir) = if is_bundled(&extend) {
                    (ThemeFile::Bundled(self.bundled_name(&extend)?), None)
                } else {
                    let base = if extend.starts_with("./") {
                        path.as_deref().and_then(Path::parent)
                    } else {
                        theme_dir
                    };
                    let base = base.unwrap_or_else(|| Path::new("."));
                    let file = if extend.ends_with(".yml") {
                        base.join(&extend)
                    } else {
                        base.join(format!("{extend}-theme.yml"))
                    };
                    let dir = file.parent().map(Path::to_path_buf);
                    (ThemeFile::Path(lexical(&file)), dir)
                };
                if let ThemeFile::Path(file) = &target {
                    if !file.is_file() {
                        return Err(ThemeError::NotFound(file.clone()));
                    }
                }
                if !self.loaded.contains(&target) || force {
                    let dir = dir.or_else(|| theme_dir.map(Path::to_path_buf));
                    self.load_file(&target, dir.as_deref())?;
                    self.current = path.clone();
                }
            }
        }
        for (key, value) in entries {
            self.process_entry(&key, value, true);
        }
        self.current = previous;
        Ok(())
    }

    fn process_entry(&mut self, key: &str, value: Yaml, normalise: bool) {
        let mut key = if normalise {
            key.replace('-', "_")
        } else {
            key.to_owned()
        };
        if key == "font" {
            if let Yaml::Map(entries) = value {
                for (subkey, subvalue) in entries {
                    if subkey == "catalog" || subkey == "fallbacks" {
                        self.process_entry(&format!("font_{subkey}"), subvalue, false);
                    }
                }
            }
            return;
        }
        if key == "font_catalog" {
            self.font_catalog(value);
            return;
        }
        if key == "font_fallbacks" {
            self.fallbacks = match value {
                Yaml::Seq(items) => items
                    .into_iter()
                    .map(|item| self.expand_vars(&plain(item).to_string()).to_string())
                    .collect(),
                _ => Vec::new(),
            };
            return;
        }
        if key.starts_with("admonition_icon_") {
            let entries = match value {
                Yaml::Map(entries) => entries,
                _ => return,
            };
            let mut icon = Vec::new();
            for (subkey, subvalue) in entries {
                let subkey = subkey.replace('-', "_");
                let subvalue = plain(subvalue);
                let subvalue = if subkey.ends_with("_color") {
                    self.colour(&subvalue)
                } else {
                    self.evaluate(&subvalue, true)
                };
                icon.push((subkey, subvalue));
            }
            self.keys.insert(key, ThemeValue::Map(icon));
            return;
        }
        if let Yaml::Map(entries) = value {
            if let Some((old, new)) = DEPRECATED_CATEGORIES.iter().find(|(old, _)| *old == key) {
                self.warn(format!(
                    "the {} theme category is deprecated; use the {} category instead",
                    old.replace('_', "-"),
                    new.replace('_', "-")
                ));
                key = (*new).to_owned();
            }
            for (subkey, subvalue) in entries {
                let subkey = if key == "role" || !subkey.contains('-') {
                    subkey
                } else {
                    subkey.replace('-', "_")
                };
                self.process_entry(&format!("{key}_{subkey}"), subvalue, false);
            }
            return;
        }
        let value = plain(value);
        if let Some(renamed) = deprecated_key(&key) {
            let value = self.evaluate(&value, false);
            self.keys.insert(renamed, value);
            return;
        }
        if PADDING_HACK_KEYS.contains(&key.as_str()) {
            let mut value = self.evaluate(&value, true);
            if let ThemeValue::List(items) = &mut value {
                let number = |item: &ThemeValue| item.as_number().unwrap_or(0.0);
                if items.len() > 2 && number(&items[0]) >= 0.0 && number(&items[2]) <= 0.0 {
                    items[2] = items[0].clone();
                }
            }
            self.keys.insert(key, value);
            return;
        }
        if key.ends_with("_color") {
            let multiple = if key == "table_border_color" {
                matches!(value, ThemeValue::List(_))
            } else {
                key == "table_grid_color"
                    && matches!(&value, ThemeValue::List(items) if items.len() == 2)
            };
            let value = if multiple {
                let ThemeValue::List(items) = value else {
                    unreachable!()
                };
                ThemeValue::List(items.iter().map(|item| self.colour(item)).collect())
            } else {
                self.colour(&value)
            };
            self.keys.insert(key, value);
            return;
        }
        if key.ends_with("_content") {
            let value = match &value {
                ThemeValue::Null => ThemeValue::Text(String::new()),
                other => ThemeValue::Text(self.expand_vars(&other.to_string()).to_string()),
            };
            self.keys.insert(key, value);
            return;
        }
        let value = self.evaluate(&value, true);
        self.keys.insert(key, value);
    }

    fn colour(&mut self, value: &ThemeValue) -> ThemeValue {
        let value = self.evaluate(value, false);
        to_color(&value).map_or(ThemeValue::Null, ThemeValue::Color)
    }

    fn font_catalog(&mut self, value: Yaml) {
        let Yaml::Map(mut entries) = value else {
            self.fonts.clear();
            return;
        };
        let merge = entries
            .iter()
            .position(|(key, _)| key == "merge")
            .map(|position| entries.remove(position).1)
            .is_some_and(|merge| !matches!(merge, Yaml::Null | Yaml::Bool(false)));
        if !merge {
            self.fonts.clear();
        }
        for (name, styles) in entries {
            let styles = match styles {
                Yaml::Str(path) => vec![("*".to_owned(), Yaml::Str(path))],
                Yaml::Map(styles) => styles,
                _ => continue,
            };
            let mut faces = FontFaces::default();
            for (style, path) in styles {
                let path = self.expand_vars(&plain(path).to_string()).to_string();
                let file = font_file(&path);
                match style.as_str() {
                    "*" => {
                        faces = FontFaces {
                            normal: Some(file.clone()),
                            bold: Some(file.clone()),
                            italic: Some(file.clone()),
                            bold_italic: Some(file),
                        };
                    }
                    "normal" | "regular" => faces.normal = Some(file),
                    "bold" => faces.bold = Some(file),
                    "italic" => faces.italic = Some(file),
                    "bold_italic" => faces.bold_italic = Some(file),
                    other => self.warn(format!("unknown font style `{other}` for {name}")),
                }
            }
            match self.fonts.iter_mut().find(|(family, _)| *family == name) {
                Some(entry) => entry.1 = faces,
                None => self.fonts.push((name, faces)),
            }
        }
    }

    fn evaluate(&mut self, value: &ThemeValue, math: bool) -> ThemeValue {
        match value {
            ThemeValue::Text(text) => {
                let expanded = self.expand_vars(text);
                match expanded {
                    ThemeValue::Text(text) if math => evaluate_math(&text),
                    other => other,
                }
            }
            ThemeValue::List(items) => {
                ThemeValue::List(items.iter().map(|item| self.evaluate(item, math)).collect())
            }
            other => other.clone(),
        }
    }

    /// Replaces `$name` references with the values of keys defined so far. A reference that
    /// is the whole value yields that value as is; otherwise its text is substituted.
    fn expand_vars(&mut self, expr: &str) -> ThemeValue {
        static VARIABLE: OnceLock<Regex> = OnceLock::new();
        let variable = VARIABLE.get_or_init(|| Regex::new(r"\$([a-z0-9_-]+)").unwrap());
        let Some(index) = expr.find('$') else {
            return ThemeValue::Text(expr.to_owned());
        };
        let lone = |text: &str| {
            variable
                .captures(text)
                .filter(|captures| captures.get(0).unwrap().as_str() == text)
                .map(|captures| captures[1].to_owned())
        };
        if index == 0 {
            if let Some(name) = lone(expr) {
                return self.resolve_var(expr, &name);
            }
        } else if index == 1 && expr.starts_with('-') {
            if let Some(name) = lone(&expr[1..]) {
                return match self.resolve_var(&expr[1..], &name) {
                    ThemeValue::Number(number) => ThemeValue::Number(-number),
                    other => ThemeValue::Text(format!("-{other}")),
                };
            }
        }
        let mut out = String::new();
        let mut last = 0;
        let captures: Vec<(std::ops::Range<usize>, String)> = variable
            .captures_iter(expr)
            .map(|captures| (captures.get(0).unwrap().range(), captures[1].to_owned()))
            .collect();
        for (range, name) in captures {
            out.push_str(&expr[last..range.start]);
            let resolved = self.resolve_var(&expr[range.clone()], &name);
            out.push_str(&resolved.to_string());
            last = range.end;
        }
        out.push_str(&expr[last..]);
        ThemeValue::Text(out)
    }

    fn resolve_var(&mut self, reference: &str, name: &str) -> ThemeValue {
        let name = name.replace('-', "_");
        if let Some(value) = self.keys.get(&name) {
            return value.clone();
        }
        for (old, new) in DEPRECATED_CATEGORIES {
            if let Some(rest) = name.strip_prefix(&format!("{old}_")) {
                if let Some(value) = self.keys.get(&format!("{new}_{rest}")) {
                    return value.clone();
                }
            }
        }
        if let Some(value) = deprecated_key(&name).and_then(|renamed| self.keys.get(&renamed)) {
            return value.clone();
        }
        self.warn(format!(
            "unknown variable reference in PDF theme: {reference}"
        ));
        ThemeValue::Text(reference.to_owned())
    }
}

/// A YAML value as a theme value, before evaluation.
fn plain(value: Yaml) -> ThemeValue {
    match value {
        Yaml::Null => ThemeValue::Null,
        Yaml::Bool(value) => ThemeValue::Bool(value),
        Yaml::Int(value) => ThemeValue::Number(value as f64),
        Yaml::Float(value) => ThemeValue::Number(value),
        Yaml::Str(value) => ThemeValue::Text(value),
        Yaml::Seq(items) => ThemeValue::List(items.into_iter().map(plain).collect()),
        Yaml::Map(entries) => ThemeValue::List(
            entries
                .into_iter()
                .map(|(key, value)| ThemeValue::List(vec![ThemeValue::Text(key), plain(value)]))
                .collect(),
        ),
    }
}

fn font_file(path: &str) -> FontFile {
    match path
        .strip_prefix("GEM_FONTS_DIR")
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
    {
        Some(name) => FontFile::Bundled(name.to_owned()),
        None => FontFile::Path(PathBuf::from(path)),
    }
}

/// Removes `.` and `..` components without touching the file system.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Quotes hex colour values so YAML reads them as text, as asciidoctor-pdf does line by line:
/// `font-color: 000000` becomes `font-color: '000000'`, and a leading `#` is dropped.
fn quote_hex_colours(text: &str) -> String {
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    let entry = ENTRY.get_or_init(|| {
        Regex::new(
            r#"^(?P<k> *[^\s]+): +(?:"(?P<h1>#)?(?P<v1>[0-9A-Fa-f]{3,6})"|'(?P<h2>#)?(?P<v2>[0-9A-Fa-f]{3,6})'|(?P<h3>#)?(?P<v3>[0-9A-Fa-f]{3,6})) *(?:#.*)?$"#,
        )
        .unwrap()
    });
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, ending) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        match entry.captures(body) {
            Some(captures) if !body.trim_end().ends_with(": null") => {
                let key = &captures["k"];
                let hash = captures
                    .name("h1")
                    .or(captures.name("h2"))
                    .or(captures.name("h3"));
                let value = captures
                    .name("v1")
                    .or(captures.name("v2"))
                    .or(captures.name("v3"))
                    .map_or("", |value| value.as_str());
                if hash.is_some() || key.ends_with("color") {
                    out.push_str(&format!("{key}: '{value}'"));
                } else {
                    out.push_str(&format!("{key}: {value}"));
                }
            }
            _ => out.push_str(body),
        }
        out.push_str(ending);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::quote_hex_colours;

    #[test]
    fn hex_colours_are_quoted() {
        let text = "a:\n  font-color: 000000\n  background_color: '#F5F5F5' # grey\n  size: 123\n  hash: #abc\n";
        assert_eq!(
            quote_hex_colours(text),
            "a:\n  font-color: '000000'\n  background_color: 'F5F5F5'\n  size: 123\n  hash: 'abc'\n"
        );
    }
}
