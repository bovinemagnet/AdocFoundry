//! `[docx]`: how Word documents are laid out and styled, and `[docx.profiles.<name>]`, Word
//! publications saved under a name. Nothing here is in `[pdf]`, and nothing of `[pdf]` (its
//! theme, fonts or standards) is read for Word: the two formats are set apart, though a profile
//! names its content as a PDF profile does (`page`, `component` or `nav`), resolved the same way.
//!
//! ```toml
//! [docx]
//! paper = "a4"              # or "letter"
//! orientation = "portrait"  # or "landscape"
//! logo = "images/logo.png"  # what `{logo}` shows, relative to the first workspace root
//! odd_even = true           # odd and even pages' own headers and footers
//!
//! [docx.margins]            # millimetres, centimetres, inches or points; a number is points
//! top = "25mm"
//! header = "12mm"
//!
//! [docx.header]             # each slot holds text and `{page-number}`, `{page-count}`,
//! left = "{document-title}" # `{document-title}`, `{chapter-title}` and `{logo}`
//! right = "{logo}"
//! [docx.header.even]        # and `[docx.header.first]`; likewise `[docx.footer]`
//!
//! [docx.styles]
//! body_font = "Calibri"
//! heading_font = "Calibri Light"
//! code_font = "Consolas"
//! body_size = 11            # points
//! heading_colour = "1F3864" # six hexadecimal digits
//! link_colour = "0563C1"
//!
//! [docx.profiles.manual]
//! component = "2.0@guide"   # or `page = "…"`, or `nav = ["…", "…"]`
//! output = "build/manual.docx"
//! orientation = "landscape" # and `paper`
//! attributes = { author = "Paul Snow" }
//! ```
//!
//! The settings files' layers apply in turn: a key the workspace file sets replaces the user's,
//! a `[docx.header]` or `[docx.footer]` table replaces that one's lines whole, and a profile
//! replaces one of the same name.

use crate::profiles::{profile_attributes, profile_content, profile_text, ProfileContent};

/// The paper a Word document is laid out on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DocxPaper {
    #[default]
    A4,
    Letter,
}

/// Margins and the header's and footer's distance from the page's edge, in twentieths of a
/// point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocxMargins {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
    pub header: u32,
    pub footer: u32,
}

impl Default for DocxMargins {
    fn default() -> Self {
        Self {
            top: 1440,
            right: 1440,
            bottom: 1440,
            left: 1440,
            header: 708,
            footer: 708,
        }
    }
}

/// One line of a header or footer: its left, centre and right slots.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocxLine {
    pub left: String,
    pub centre: String,
    pub right: String,
}

/// A header's or footer's lines: on odd pages (every page, when odd and even do not differ),
/// even pages and the body's first page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocxSlots {
    pub odd: DocxLine,
    pub even: Option<DocxLine>,
    pub first: Option<DocxLine>,
}

/// Changes to the built-in Word styles; the size in half-points.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocxStyles {
    pub body_font: Option<String>,
    pub heading_font: Option<String>,
    pub code_font: Option<String>,
    pub body_size: Option<u32>,
    pub heading_colour: Option<String>,
    pub link_colour: Option<String>,
}

/// A Word publication saved under a name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocxProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Where the document is written, relative to the first workspace root and inside it;
    /// `<name>.docx` there when unset.
    pub output: Option<String>,
    /// Attributes every page is read with before its own header.
    pub attributes: Vec<(String, String)>,
    /// The profile's own paper and orientation, in place of `[docx]`'s.
    pub paper: Option<DocxPaper>,
    pub landscape: Option<bool>,
}

/// The `[docx]` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocxSettings {
    pub paper: DocxPaper,
    pub landscape: bool,
    pub margins: DocxMargins,
    pub header: DocxSlots,
    pub footer: DocxSlots,
    pub odd_even: bool,
    /// The image `{logo}` shows, relative to the first workspace root.
    pub logo: Option<String>,
    pub styles: DocxStyles,
    pub profiles: Vec<DocxProfile>,
}

impl Default for DocxSettings {
    /// A4, portrait, an inch of margin, and asciidoctor-pdf's default theme's running content:
    /// the page number in the footer, at the right of odd pages and the left of even ones.
    fn default() -> Self {
        Self {
            paper: DocxPaper::A4,
            landscape: false,
            margins: DocxMargins::default(),
            header: DocxSlots::default(),
            footer: DocxSlots {
                odd: DocxLine {
                    right: "{page-number}".to_owned(),
                    ..DocxLine::default()
                },
                even: Some(DocxLine {
                    left: "{page-number}".to_owned(),
                    ..DocxLine::default()
                }),
                first: None,
            },
            odd_even: true,
            logo: None,
            styles: DocxStyles::default(),
            profiles: Vec::new(),
        }
    }
}

fn paper(value: &toml::Value) -> Result<DocxPaper, String> {
    match value.as_str().map(str::to_ascii_lowercase).as_deref() {
        Some("a4") => Ok(DocxPaper::A4),
        Some("letter") => Ok(DocxPaper::Letter),
        _ => Err("expected \"a4\" or \"letter\"".to_owned()),
    }
}

fn landscape(value: &toml::Value) -> Result<bool, String> {
    match value.as_str() {
        Some("portrait") => Ok(false),
        Some("landscape") => Ok(true),
        _ => Err("expected \"portrait\" or \"landscape\"".to_owned()),
    }
}

/// A length in twentieths of a point: `"25mm"`, `"2.5cm"`, `"1in"`, `"72pt"`, or a number of
/// points; at most 10 inches.
fn length(value: &toml::Value) -> Result<u32, String> {
    const PROBLEM: &str = "expected a length such as \"25mm\", \"2.5cm\", \"1in\" or \"72pt\"";
    let points = match value {
        toml::Value::Integer(points) => *points as f64,
        toml::Value::Float(points) => *points,
        toml::Value::String(text) => {
            let text = text.trim();
            let (number, unit) = text
                .find(|c: char| c.is_ascii_alphabetic())
                .map_or((text, "pt"), |at| (&text[..at], &text[at..]));
            let number: f64 = number.trim().parse().map_err(|_| PROBLEM)?;
            match unit {
                "mm" => number * 72.0 / 25.4,
                "cm" => number * 72.0 / 2.54,
                "in" => number * 72.0,
                "pt" => number,
                _ => return Err(PROBLEM.to_owned()),
            }
        }
        _ => return Err(PROBLEM.to_owned()),
    };
    if !points.is_finite() || !(0.0..=720.0).contains(&points) {
        return Err("expected a length between 0 and 10 inches".to_owned());
    }
    Ok((points * 20.0).round() as u32)
}

fn text(value: &toml::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "expected a string".to_owned())
}

/// A font family: a name Word can look up, at most 31 characters.
fn font(value: &toml::Value) -> Result<String, String> {
    let name = text(value)?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 31 || name.chars().any(char::is_control) {
        return Err("expected a font family name of 1 to 31 characters".to_owned());
    }
    Ok(name.to_owned())
}

/// A colour as six hexadecimal digits, with or without `#`.
fn colour(value: &toml::Value) -> Result<String, String> {
    let colour = text(value)?;
    let colour = colour.trim().trim_start_matches('#');
    if colour.len() != 6 || !colour.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("expected six hexadecimal digits, such as \"1F3864\"".to_owned());
    }
    Ok(colour.to_ascii_uppercase())
}

/// A body size in points, from 6 to 72, as half-points.
fn size(value: &toml::Value) -> Result<u32, String> {
    let points = match value {
        toml::Value::Integer(points) => *points as f64,
        toml::Value::Float(points) => *points,
        _ => return Err("expected a size in points".to_owned()),
    };
    if !(6.0..=72.0).contains(&points) {
        return Err("expected a size from 6 to 72 points".to_owned());
    }
    Ok((points * 2.0).round() as u32)
}

/// A header's or footer's line: `left`, `centre` and `right`, and its other keys, to be read by
/// the caller.
fn line(
    table: &toml::Table,
    name: &str,
    problems: &mut Vec<(String, String)>,
    nested: &[&str],
) -> DocxLine {
    let mut line = DocxLine::default();
    for (key, value) in table {
        let slot = match key.as_str() {
            "left" => &mut line.left,
            "centre" | "center" => &mut line.centre,
            "right" => &mut line.right,
            key if nested.contains(&key) => continue,
            key => {
                problems.push((format!("{name}.{key}"), "unknown setting".to_owned()));
                continue;
            }
        };
        match text(value) {
            Ok(value) => *slot = value,
            Err(problem) => problems.push((format!("{name}.{key}"), problem)),
        }
    }
    line
}

/// `[docx.header]` or `[docx.footer]`: its lines, replacing the earlier ones whole.
fn slots(
    value: &toml::Value,
    name: &str,
    problems: &mut Vec<(String, String)>,
) -> Option<DocxSlots> {
    let Some(table) = value.as_table() else {
        problems.push((name.to_owned(), "expected a table".to_owned()));
        return None;
    };
    let mut slots = DocxSlots {
        odd: line(table, name, problems, &["even", "first"]),
        even: None,
        first: None,
    };
    for (key, target) in [("even", &mut slots.even), ("first", &mut slots.first)] {
        if let Some(value) = table.get(key) {
            let name = format!("{name}.{key}");
            match value.as_table() {
                Some(table) => *target = Some(line(table, &name, problems, &[])),
                None => problems.push((name, "expected a table".to_owned())),
            }
        }
    }
    Some(slots)
}

const PROFILE_KEYS: &[&str] = &[
    "page",
    "component",
    "nav",
    "output",
    "attributes",
    "paper",
    "orientation",
];

/// A profile from its `[docx.profiles.<name>]` table.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<DocxProfile, String> {
    if let Some(unknown) = table
        .keys()
        .find(|key| !PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown key `{unknown}`"));
    }
    let output = profile_text(table, "output")?;
    if let Some(output) = &output {
        let docx = std::path::Path::new(output)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("docx"));
        if !docx {
            return Err(format!("`output`: {output} is not a .docx file"));
        }
    }
    Ok(DocxProfile {
        name: name.to_owned(),
        content: profile_content(table)?,
        output,
        attributes: profile_attributes(table)?,
        paper: table
            .get("paper")
            .map(paper)
            .transpose()
            .map_err(|problem| format!("`paper`: {problem}"))?,
        landscape: table
            .get("orientation")
            .map(landscape)
            .transpose()
            .map_err(|problem| format!("`orientation`: {problem}"))?,
    })
}

/// Applies the `[docx]` key `key`; each problem's setting name and what is wrong with it.
pub(crate) fn apply(
    settings: &mut DocxSettings,
    key: &str,
    value: &toml::Value,
) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let name = format!("docx.{key}");
    let mut set = |result: Result<(), String>| {
        if let Err(problem) = result {
            problems.push((name.clone(), problem));
        }
    };
    match key {
        "paper" => set(paper(value).map(|paper| settings.paper = paper)),
        "orientation" => set(landscape(value).map(|landscape| settings.landscape = landscape)),
        "logo" => set(text(value).map(|logo| settings.logo = Some(logo))),
        "odd_even" => set(value
            .as_bool()
            .map(|odd_even| settings.odd_even = odd_even)
            .ok_or_else(|| "expected true or false".to_owned())),
        "margins" => match value.as_table() {
            None => set(Err("expected a table".to_owned())),
            Some(table) => {
                for (side, value) in table {
                    let margins = &mut settings.margins;
                    let target = match side.as_str() {
                        "top" => &mut margins.top,
                        "right" => &mut margins.right,
                        "bottom" => &mut margins.bottom,
                        "left" => &mut margins.left,
                        "header" => &mut margins.header,
                        "footer" => &mut margins.footer,
                        _ => {
                            problems.push((
                                format!("docx.margins.{side}"),
                                "unknown setting".to_owned(),
                            ));
                            continue;
                        }
                    };
                    match length(value) {
                        Ok(length) => *target = length,
                        Err(problem) => problems.push((format!("docx.margins.{side}"), problem)),
                    }
                }
            }
        },
        "header" => {
            if let Some(slots) = slots(value, &name, &mut problems) {
                settings.header = slots;
            }
        }
        "footer" => {
            if let Some(slots) = slots(value, &name, &mut problems) {
                settings.footer = slots;
            }
        }
        "styles" => match value.as_table() {
            None => set(Err("expected a table".to_owned())),
            Some(table) => {
                for (key, value) in table {
                    let styles = &mut settings.styles;
                    let result = match key.as_str() {
                        "body_font" => font(value).map(|font| styles.body_font = Some(font)),
                        "heading_font" => font(value).map(|font| styles.heading_font = Some(font)),
                        "code_font" => font(value).map(|font| styles.code_font = Some(font)),
                        "body_size" => size(value).map(|size| styles.body_size = Some(size)),
                        "heading_colour" | "heading_color" => {
                            colour(value).map(|colour| styles.heading_colour = Some(colour))
                        }
                        "link_colour" | "link_color" => {
                            colour(value).map(|colour| styles.link_colour = Some(colour))
                        }
                        _ => Err("unknown setting".to_owned()),
                    };
                    if let Err(problem) = result {
                        problems.push((format!("docx.styles.{key}"), problem));
                    }
                }
            }
        },
        "profiles" => match value.as_table() {
            None => set(Err("expected a table of profiles".to_owned())),
            Some(profiles) => {
                for (profile_name, table) in profiles {
                    let parsed = table
                        .as_table()
                        .ok_or_else(|| "expected a table".to_owned())
                        .and_then(|table| parse_profile(profile_name, table));
                    match parsed {
                        Ok(profile) => {
                            settings.profiles.retain(|known| known.name != profile.name);
                            settings.profiles.push(profile);
                            settings
                                .profiles
                                .sort_by(|left, right| left.name.cmp(&right.name));
                        }
                        Err(problem) => {
                            problems.push((format!("docx.profiles.{profile_name}"), problem));
                        }
                    }
                }
            }
        },
        _ => set(Err("unknown setting".to_owned())),
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Settings, WORKSPACE_SETTINGS_FILE};

    fn load(user: Option<&str>, workspace: &str) -> Settings {
        let directory = tempfile::tempdir().unwrap();
        let user = user.map(|text| {
            let path = directory.path().join("user.toml");
            std::fs::write(&path, text).unwrap();
            path
        });
        std::fs::write(directory.path().join(WORKSPACE_SETTINGS_FILE), workspace).unwrap();
        Settings::load(user.as_deref(), Some(directory.path())).unwrap()
    }

    #[test]
    fn the_defaults_are_asciidoctor_pdf_s() {
        let defaults = Settings::default().docx;
        assert_eq!(defaults, DocxSettings::default());
        assert_eq!(defaults.paper, DocxPaper::A4);
        assert_eq!(defaults.footer.odd.right, "{page-number}");
        assert_eq!(defaults.footer.even.unwrap().left, "{page-number}");
        assert!(defaults.header.odd.left.is_empty());
    }

    #[test]
    fn every_setting_is_read() {
        let settings = load(
            None,
            "[docx]\npaper = \"Letter\"\norientation = \"landscape\"\nlogo = \"images/logo.png\"\n\
             odd_even = false\n\
             [docx.margins]\ntop = \"25.4mm\"\nright = \"2.54cm\"\nbottom = \"1in\"\nleft = 72\n\
             header = \"36pt\"\nfooter = 18.5\n\
             [docx.header]\nleft = \"{document-title}\"\ncenter = \"Draft\"\nright = \"{logo}\"\n\
             [docx.header.first]\ncentre = \"\"\n\
             [docx.footer]\nright = \"{page-number} of {page-count}\"\n\
             [docx.footer.even]\nleft = \"{page-number}\"\n\
             [docx.styles]\nbody_font = \"Georgia\"\nheading_font = \"Gill Sans\"\ncode_font = \"Menlo\"\n\
             body_size = 10.5\nheading_colour = \"#1f3864\"\nlink_color = \"C00000\"\n",
        );
        assert_eq!(settings.warnings, Vec::<String>::new());
        let docx = settings.docx;
        assert_eq!(docx.paper, DocxPaper::Letter);
        assert!(docx.landscape);
        assert!(!docx.odd_even);
        assert_eq!(docx.logo.as_deref(), Some("images/logo.png"));
        assert_eq!(
            docx.margins,
            DocxMargins {
                top: 1440,
                right: 1440,
                bottom: 1440,
                left: 1440,
                header: 720,
                footer: 370,
            }
        );
        assert_eq!(docx.header.odd.centre, "Draft");
        assert_eq!(docx.header.first, Some(DocxLine::default()));
        assert_eq!(docx.header.even, None);
        assert_eq!(docx.footer.odd.right, "{page-number} of {page-count}");
        assert_eq!(docx.footer.even.unwrap().left, "{page-number}");
        assert_eq!(
            docx.styles,
            DocxStyles {
                body_font: Some("Georgia".to_owned()),
                heading_font: Some("Gill Sans".to_owned()),
                code_font: Some("Menlo".to_owned()),
                body_size: Some(21),
                heading_colour: Some("1F3864".to_owned()),
                link_colour: Some("C00000".to_owned()),
            }
        );
    }

    #[test]
    fn the_workspace_layer_wins_key_by_key_and_table_by_table() {
        let settings = load(
            Some("[docx]\npaper = \"letter\"\norientation = \"landscape\"\n[docx.margins]\ntop = \"10mm\"\nleft = \"10mm\"\n[docx.footer]\ncentre = \"user\"\n"),
            "[docx]\npaper = \"a4\"\n[docx.margins]\ntop = \"20mm\"\n[docx.footer]\nright = \"workspace\"\n",
        );
        let docx = settings.docx;
        assert_eq!(docx.paper, DocxPaper::A4);
        assert!(docx.landscape, "the user's, which the workspace leaves");
        assert_eq!(docx.margins.top, 1134);
        assert_eq!(docx.margins.left, 567);
        // A footer table replaces the footer whole.
        assert_eq!(docx.footer.odd.centre, "");
        assert_eq!(docx.footer.odd.right, "workspace");
        assert_eq!(docx.footer.even, None);
    }

    #[test]
    fn what_cannot_be_used_is_warned_about() {
        let settings = load(
            None,
            "[docx]\npaper = \"a3\"\norientation = \"sideways\"\nodd_even = \"yes\"\ntheme = \"acme\"\n\
             [docx.margins]\ntop = \"3 parsecs\"\nmiddle = \"1in\"\nleft = \"20in\"\n\
             [docx.header]\nleft = 3\nmiddle = \"x\"\neven = \"x\"\n\
             [docx.styles]\nbody_font = \"\"\nheading_colour = \"blue\"\nbody_size = 300\nshadow = true\n",
        );
        let named: std::collections::BTreeSet<&str> = settings
            .warnings
            .iter()
            .map(|warning| warning.split('`').nth(1).unwrap())
            .collect();
        assert_eq!(
            named,
            std::collections::BTreeSet::from([
                "docx.paper",
                "docx.orientation",
                "docx.odd_even",
                "docx.theme",
                "docx.margins.top",
                "docx.margins.middle",
                "docx.margins.left",
                "docx.header.left",
                "docx.header.middle",
                "docx.header.even",
                "docx.styles.body_font",
                "docx.styles.heading_colour",
                "docx.styles.body_size",
                "docx.styles.shadow",
            ])
        );
        assert_eq!(settings.warnings.len(), named.len());
        // What could be used still is, and the rest keeps its default.
        assert_eq!(settings.docx.paper, DocxPaper::A4);
        assert_eq!(settings.docx.margins, DocxMargins::default());
    }

    #[test]
    fn profiles_are_read_and_checked() {
        let settings = load(
            None,
            "[docx.profiles.page]\npage = \"modules/ROOT/pages/index.adoc\"\n\
             [docx.profiles.book]\ncomponent = \"2.0@guide\"\noutput = \"build/Guide.DOCX\"\n\
             paper = \"letter\"\norientation = \"landscape\"\nattributes = { author = \"Paul Snow\", draft = true }\n\
             [docx.profiles.selected]\nnav = [\"modules/a/nav.adoc\", \"modules/b/nav.adoc\"]\n\
             [docx.profiles.two]\npage = \"a.adoc\"\ncomponent = \"guide\"\n\
             [docx.profiles.pdf]\npage = \"a.adoc\"\noutput = \"manual.pdf\"\n\
             [docx.profiles.themed]\npage = \"a.adoc\"\ntheme = \"acme\"\n\
             [docx.profiles.paper]\npage = \"a.adoc\"\npaper = \"a5\"\n",
        );
        let names: Vec<&str> = settings
            .docx
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        assert_eq!(names, ["book", "page", "selected"]);
        let book = &settings.docx.profiles[0];
        assert_eq!(
            book.content,
            ProfileContent::Component("2.0@guide".to_owned())
        );
        assert_eq!(book.output.as_deref(), Some("build/Guide.DOCX"));
        assert_eq!(book.paper, Some(DocxPaper::Letter));
        assert_eq!(book.landscape, Some(true));
        assert_eq!(
            book.attributes,
            [
                ("author".to_owned(), "Paul Snow".to_owned()),
                ("draft".to_owned(), String::new()),
            ]
        );
        let mut warnings = settings.warnings.clone();
        warnings.sort();
        assert_eq!(
            warnings,
            [
                "docx.profiles.paper` : `paper`: expected \"a4\" or \"letter\"",
                "docx.profiles.pdf` : `output`: manual.pdf is not a .docx file",
                "docx.profiles.themed` : unknown key `theme`",
                "docx.profiles.two` : expected one of `page`, `component` or `nav`",
            ]
            .map(|tail| {
                let (name, problem) = tail.split_once("` : ").unwrap();
                format!(
                    "{}: `{name}`: {problem}",
                    settings.workspace_file().unwrap().display()
                )
            })
        );
        // A PDF profile of the same name is its own.
        assert!(settings.pdf.profiles.is_empty());
    }

    #[test]
    fn docx_settings_leave_pdf_unchanged() {
        let pdf = "[pdf]\ntheme = \"acme\"\nstandards = [\"pdf-a-2a\"]\n\
                   [pdf.profiles.book]\ncomponent = \"guide\"\n";
        let without = load(None, pdf);
        let with = load(
            None,
            &format!(
                "{pdf}[docx]\npaper = \"letter\"\n[docx.profiles.book]\ncomponent = \"guide\"\n"
            ),
        );
        assert_eq!(with.pdf, without.pdf);
        assert_eq!(with.warnings, Vec::<String>::new());
        assert_eq!(
            with.pdf.profiles[0].output, None,
            "still `book.pdf` by default"
        );
        assert_eq!(with.docx.profiles[0].name, "book");
        // `[pdf]` says nothing to Word.
        assert_eq!(without.docx, DocxSettings::default());
    }
}
