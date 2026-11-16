//! A document's look: what a PDF theme (or, later, a Word template) sets, laid over the
//! writer's own styles. `[docx.styles]` written in the settings is laid over this in turn
//! ([`crate::StyleOverrides`]). The Word looks PRD (`docs/prd/PRD-AdocFoundry_DOCX_Themes.md`),
//! §5.2.
//!
//! A theme's keys reach Word as styles a reader can still change: the body as the document's
//! defaults and `Normal`, `heading_h<n>` as `Heading<n>`, code as `SourceCode`, `LiteralBlock`
//! and `CodeChar`, links as `Hyperlink`, captions as `Caption`, quotes as `Quote`, and the
//! table, admonition, sidebar and example boxes as their table styles. The title page takes
//! the theme's alignment, title, subtitle and author styles and its logo; the covers take
//! `cover_front_image` and `cover_back_image` when the page names none. [`MAPPED_KEYS`] lists
//! every key read.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_theme::{LoadedTheme, ThemeValue};

use crate::{Line, Slots};

/// What a style sets, each property only when given; laid over the writer's own definition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleLook {
    pub font: Option<String>,
    /// In half-points.
    pub size: Option<u32>,
    /// Six hexadecimal digits.
    pub colour: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub caps: Option<bool>,
    pub underline: Option<bool>,
    /// Space before and after, in twentieths of a point.
    pub before: Option<u32>,
    pub after: Option<u32>,
    /// Line spacing in 240ths of a line.
    pub line: Option<u32>,
    /// `left`, `center`, `right` or `both`.
    pub align: Option<String>,
    /// A paragraph's (or a run's) background.
    pub shading: Option<String>,
    /// A border round a paragraph, or on its left only.
    pub border: Option<Border>,
}

impl StyleLook {
    /// This look with `over`'s properties where `over` gives them.
    #[must_use]
    pub fn with(&self, over: &Self) -> Self {
        Self {
            font: over.font.clone().or_else(|| self.font.clone()),
            size: over.size.or(self.size),
            colour: over.colour.clone().or_else(|| self.colour.clone()),
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            caps: over.caps.or(self.caps),
            underline: over.underline.or(self.underline),
            before: over.before.or(self.before),
            after: over.after.or(self.after),
            line: over.line.or(self.line),
            align: over.align.clone().or_else(|| self.align.clone()),
            shading: over.shading.clone().or_else(|| self.shading.clone()),
            border: over.border.clone().or_else(|| self.border.clone()),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A paragraph border.
#[derive(Clone, Debug, PartialEq)]
pub struct Border {
    pub colour: String,
    /// In eighths of a point.
    pub width: u32,
    pub sides: BorderSides,
}

/// Where a paragraph's border is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BorderSides {
    /// All round, as round code.
    All,
    /// On the left, as a quote's bar.
    Left,
    /// Above, as a footer's rule.
    Top,
    /// Below, as a header's rule.
    Bottom,
}

impl BorderSides {
    pub(crate) fn names(self) -> &'static [&'static str] {
        match self {
            Self::All => &["top", "left", "bottom", "right"],
            Self::Left => &["left"],
            Self::Top => &["top"],
            Self::Bottom => &["bottom"],
        }
    }
}

/// A theme's headers and footers. A periphery the theme does not have (no `header_height`) is
/// `None`, and shown empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunningLook {
    pub header: Option<Slots>,
    pub footer: Option<Slots>,
    /// How far the header's text is from the page's top edge, and the footer's from its
    /// bottom edge, in twentieths of a point.
    pub header_distance: Option<u32>,
    pub footer_distance: Option<u32>,
}

/// A box drawn as a table (a table, an admonition, a sidebar or an example).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxLook {
    pub border_colour: Option<String>,
    /// In eighths of a point.
    pub border_width: Option<u32>,
    pub background: Option<String>,
    /// A table's header row.
    pub head_background: Option<String>,
    pub head_colour: Option<String>,
    pub head_bold: Option<bool>,
    /// Every other body row, from the second.
    pub stripe: Option<String>,
}

/// The title page's own look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitlePageLook {
    /// `left`, `center` or `right`.
    pub align: Option<String>,
    pub logo: Option<PathBuf>,
    pub logo_align: Option<String>,
}

/// Everything a look sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WordLook {
    /// The document's defaults and `Normal`.
    pub defaults: StyleLook,
    /// Styles by id (`Heading2`, `SourceCode`, `Hyperlink`).
    pub styles: BTreeMap<String, StyleLook>,
    /// Table styles by id (`AdocTable`, `AdocAdmonition`, `AdocSidebar`, `AdocExample`).
    pub boxes: BTreeMap<String, BoxLook>,
    pub title_page: TitlePageLook,
    /// The theme's headers and footers; `None` for a look without them.
    pub running: Option<RunningLook>,
    /// The covers when the document names none.
    pub front_cover: Option<PathBuf>,
    pub back_cover: Option<PathBuf>,
    /// The fonts the look's families are, embedded where its styles name them.
    pub fonts: Vec<crate::FontFamily>,
    /// A Word template's styles, numbering, theme, fonts, headers and footers.
    pub template: Option<std::sync::Arc<crate::Template>>,
}

/// The theme keys a Word export reads, with the `<n>` of `heading_h<n>` written as `<n>`;
/// any other key is ignored.
pub const MAPPED_KEYS: &[&str] = &[
    "page_size",
    "page_layout",
    "page_margin",
    "base_font_family",
    "base_font_size",
    "base_font_color",
    "base_line_height",
    "base_text_align",
    "heading_font_family",
    "heading_font_color",
    "heading_font_style",
    "heading_text_transform",
    "heading_margin_top",
    "heading_margin_bottom",
    "heading_h<n>_font_family",
    "heading_h<n>_font_size",
    "heading_h<n>_font_color",
    "heading_h<n>_font_style",
    "heading_h<n>_text_transform",
    "heading_h<n>_margin_top",
    "heading_h<n>_margin_bottom",
    "link_font_color",
    "link_text_decoration",
    "code_font_family",
    "code_font_size",
    "code_font_color",
    "code_background_color",
    "code_border_color",
    "code_border_width",
    "codespan_font_family",
    "codespan_font_color",
    "codespan_background_color",
    "caption_font_color",
    "caption_font_size",
    "caption_font_style",
    "caption_text_align",
    "caption_align",
    "quote_font_color",
    "quote_font_size",
    "quote_font_style",
    "quote_border_color",
    "quote_border_left_width",
    "admonition_background_color",
    "admonition_column_rule_color",
    "admonition_column_rule_width",
    "sidebar_background_color",
    "sidebar_border_color",
    "sidebar_border_width",
    "example_background_color",
    "example_border_color",
    "example_border_width",
    "table_border_color",
    "table_border_width",
    "table_background_color",
    "table_head_background_color",
    "table_head_font_color",
    "table_head_font_style",
    "table_body_stripe_background_color",
    "title_page_text_align",
    "title_page_title_font_family",
    "title_page_title_font_size",
    "title_page_title_font_color",
    "title_page_title_font_style",
    "title_page_subtitle_font_family",
    "title_page_subtitle_font_size",
    "title_page_subtitle_font_color",
    "title_page_subtitle_font_style",
    "title_page_authors_font_size",
    "title_page_authors_font_color",
    "title_page_logo_image",
    "title_page_logo_align",
    "cover_front_image",
    "cover_back_image",
    "header_height",
    "header_font_family",
    "header_font_size",
    "header_font_color",
    "header_font_style",
    "header_border_color",
    "header_border_width",
    "header_padding",
    "header_vertical_align",
    "header_recto_left_content",
    "header_recto_center_content",
    "header_recto_right_content",
    "header_verso_left_content",
    "header_verso_center_content",
    "header_verso_right_content",
    "footer_height",
    "footer_font_family",
    "footer_font_size",
    "footer_font_color",
    "footer_font_style",
    "footer_border_color",
    "footer_border_width",
    "footer_padding",
    "footer_vertical_align",
    "footer_recto_left_content",
    "footer_recto_center_content",
    "footer_recto_right_content",
    "footer_verso_left_content",
    "footer_verso_center_content",
    "footer_verso_right_content",
];

/// The theme's keys, read as Word wants them.
struct Keys<'a> {
    theme: &'a LoadedTheme,
}

impl Keys<'_> {
    fn get(&self, key: &str) -> Option<&ThemeValue> {
        self.theme.keys.get(key)
    }

    fn number(&self, key: &str) -> Option<f64> {
        self.get(key)
            .and_then(ThemeValue::as_number)
            .filter(|value| value.is_finite() && *value >= 0.0)
    }

    fn text(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(ThemeValue::as_text)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }

    fn colour(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(ThemeValue::as_color)
            .and_then(|colour| colour.rgb())
            .map(|[r, g, b]| format!("{r:02X}{g:02X}{b:02X}"))
    }

    /// A font size in points as half-points.
    fn size(&self, key: &str) -> Option<u32> {
        self.number(key)
            .map(|points| (points * 2.0).round().clamp(2.0, 3276.0) as u32)
    }

    /// A length in points as twentieths of a point.
    fn twips(&self, key: &str) -> Option<u32> {
        self.number(key)
            .map(|points| (points * 20.0).round().min(31_680.0) as u32)
    }

    /// A border width in points as eighths of a point, as Word allows them (2 to 96).
    fn eighths(&self, key: &str) -> Option<u32> {
        self.number(key)
            .map(|points| (points * 8.0).round().clamp(2.0, 96.0) as u32)
    }

    /// `font_style`: bold and italic.
    fn style(&self, key: &str) -> (Option<bool>, Option<bool>) {
        match self.text(key).as_deref() {
            Some("bold") => (Some(true), Some(false)),
            Some("italic") => (Some(false), Some(true)),
            Some("bold_italic") => (Some(true), Some(true)),
            Some("normal") => (Some(false), Some(false)),
            Some("normal_italic") => (Some(false), Some(true)),
            _ => (None, None),
        }
    }

    /// `text_transform`: capitals for `uppercase`; nothing else Word can show.
    fn caps(&self, key: &str) -> Option<bool> {
        self.text(key).map(|transform| transform == "uppercase")
    }

    fn align(&self, key: &str) -> Option<String> {
        self.text(key).and_then(|align| match align.as_str() {
            "left" => Some("left".to_owned()),
            "center" => Some("center".to_owned()),
            "right" => Some("right".to_owned()),
            "justify" => Some("both".to_owned()),
            _ => None,
        })
    }

    /// A font, size, colour and style under `prefix` (`code_font_*`).
    fn font(&self, prefix: &str) -> StyleLook {
        let (bold, italic) = self.style(&format!("{prefix}_font_style"));
        StyleLook {
            font: self.text(&format!("{prefix}_font_family")),
            size: self.size(&format!("{prefix}_font_size")),
            colour: self.colour(&format!("{prefix}_font_color")),
            bold,
            italic,
            ..StyleLook::default()
        }
    }

    /// An image key (`image:logo.png[]` or a path), relative to the theme's folder.
    fn image(&self, key: &str) -> Option<PathBuf> {
        let value = self.text(key)?;
        let target = value
            .strip_prefix("image:")
            .and_then(|rest| rest.split_once('[').map(|(target, _)| target))
            .unwrap_or(&value)
            .trim();
        if target.is_empty() || target.contains("://") {
            return None;
        }
        let path = PathBuf::from(target);
        Some(match &self.theme.dir {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        })
    }
}

impl WordLook {
    /// The look of a Word template: its own styles, numbering, page, headers and footers,
    /// with the export's styles added where it has none.
    #[must_use]
    pub fn from_template(template: crate::Template) -> Self {
        Self {
            template: Some(std::sync::Arc::new(template)),
            ..Self::default()
        }
    }

    /// The look of a PDF theme, as far as Word can show it; `attributes` are the document's,
    /// which its running content may name.
    #[must_use]
    pub fn from_theme(theme: &LoadedTheme, attributes: &BTreeMap<String, String>) -> Self {
        let keys = Keys { theme };
        let mut look = Self {
            defaults: StyleLook {
                line: keys
                    .number("base_line_height")
                    .map(|height| (height * 240.0).round().clamp(120.0, 1200.0) as u32),
                align: keys.align("base_text_align"),
                ..keys.font("base")
            },
            ..Self::default()
        };
        let mut style = |id: &str, style: StyleLook| {
            if !style.is_empty() {
                look.styles.insert(id.to_owned(), style);
            }
        };

        // Headings: each level's own keys over the shared ones.
        let shared = StyleLook {
            caps: keys.caps("heading_text_transform"),
            before: keys.twips("heading_margin_top"),
            after: keys.twips("heading_margin_bottom"),
            ..keys.font("heading")
        };
        for level in 1..=6 {
            let prefix = format!("heading_h{level}");
            let own = StyleLook {
                caps: keys.caps(&format!("{prefix}_text_transform")),
                before: keys.twips(&format!("{prefix}_margin_top")),
                after: keys.twips(&format!("{prefix}_margin_bottom")),
                ..keys.font(&prefix)
            };
            style(&format!("Heading{level}"), shared.with(&own));
        }

        style(
            "Hyperlink",
            StyleLook {
                colour: keys.colour("link_font_color"),
                underline: keys
                    .text("link_text_decoration")
                    .map(|decoration| decoration == "underline"),
                ..StyleLook::default()
            },
        );
        let code = StyleLook {
            shading: keys.colour("code_background_color"),
            border: keys.colour("code_border_color").and_then(|colour| {
                keys.eighths("code_border_width").map(|width| Border {
                    colour,
                    width,
                    sides: BorderSides::All,
                })
            }),
            ..keys.font("code")
        };
        style("SourceCode", code.clone());
        style("LiteralBlock", code);
        style(
            "CodeChar",
            StyleLook {
                shading: keys.colour("codespan_background_color"),
                ..keys.font("codespan")
            },
        );
        style(
            "Caption",
            StyleLook {
                // As the PDF reads it: `caption_text_align`, else the older `caption_align`.
                align: keys
                    .align("caption_text_align")
                    .or_else(|| keys.align("caption_align")),
                ..keys.font("caption")
            },
        );
        style(
            "Quote",
            StyleLook {
                border: keys
                    .colour("quote_border_color")
                    .zip(keys.eighths("quote_border_left_width"))
                    .map(|(colour, width)| Border {
                        colour,
                        width,
                        sides: BorderSides::Left,
                    }),
                ..keys.font("quote")
            },
        );

        // The title page.
        // As the PDF reads it: `title_page_text_align`, else the body's alignment.
        let title_align = keys
            .align("title_page_text_align")
            .or_else(|| keys.align("base_text_align"));
        style(
            "Title",
            StyleLook {
                align: title_align.clone(),
                ..keys.font("title_page_title")
            },
        );
        style(
            "Subtitle",
            StyleLook {
                align: title_align.clone(),
                ..keys.font("title_page_subtitle")
            },
        );
        style(
            "TitlePageDetail",
            StyleLook {
                align: title_align.clone(),
                ..keys.font("title_page_authors")
            },
        );
        look.title_page = TitlePageLook {
            align: title_align,
            logo: keys.image("title_page_logo_image"),
            logo_align: keys.align("title_page_logo_align"),
        };
        look.running = Some(running(&keys, attributes, &mut look.styles));
        look.front_cover = keys.image("cover_front_image");
        look.back_cover = keys.image("cover_back_image");

        // Boxes.
        let mut boxes = |id: &str, look_of: BoxLook| {
            if look_of != BoxLook::default() {
                look.boxes.insert(id.to_owned(), look_of);
            }
        };
        let (head_bold, _) = keys.style("table_head_font_style");
        boxes(
            "AdocTable",
            BoxLook {
                border_colour: keys.colour("table_border_color"),
                border_width: keys.eighths("table_border_width"),
                background: keys.colour("table_background_color"),
                head_background: keys.colour("table_head_background_color"),
                head_colour: keys.colour("table_head_font_color"),
                head_bold,
                stripe: keys.colour("table_body_stripe_background_color"),
            },
        );
        boxes(
            "AdocAdmonition",
            BoxLook {
                border_colour: keys.colour("admonition_column_rule_color"),
                border_width: keys.eighths("admonition_column_rule_width"),
                background: keys.colour("admonition_background_color"),
                ..BoxLook::default()
            },
        );
        for (id, prefix) in [("AdocSidebar", "sidebar"), ("AdocExample", "example")] {
            boxes(
                id,
                BoxLook {
                    border_colour: keys.colour(&format!("{prefix}_border_color")),
                    border_width: keys.eighths(&format!("{prefix}_border_width")),
                    background: keys.colour(&format!("{prefix}_background_color")),
                    ..BoxLook::default()
                },
            );
        }
        look
    }

    /// The look of style `id`, if it has one.
    pub(crate) fn style(&self, id: &str) -> Option<&StyleLook> {
        self.styles.get(id)
    }
}

/// Attributes running content can name that change from page to page; the writer shows them
/// as fields or as the document's title.
const DYNAMIC: &[&str] = &[
    "page-number",
    "page-count",
    "document-title",
    "part-title",
    "chapter-title",
    "section-title",
    "section-or-chapter-title",
];

/// The theme's headers and footers, and their styles in `styles` (`Header`, `Footer`).
fn running(
    keys: &Keys<'_>,
    attributes: &BTreeMap<String, String>,
    styles: &mut BTreeMap<String, StyleLook>,
) -> RunningLook {
    let mut look = RunningLook::default();
    for periphery in ["header", "footer"] {
        // As the PDF: a periphery with no height is not drawn.
        let Some(height) = keys.number(&format!("{periphery}_height")) else {
            continue;
        };
        let line = |side: &str| Line {
            left: slot(
                keys,
                attributes,
                &format!("{periphery}_{side}_left_content"),
            ),
            centre: slot(
                keys,
                attributes,
                &format!("{periphery}_{side}_center_content"),
            ),
            right: slot(
                keys,
                attributes,
                &format!("{periphery}_{side}_right_content"),
            ),
        };
        // Even (verso) pages show their own content only, as the PDF does.
        let slots = Slots {
            odd: line("recto"),
            even: Some(line("verso")),
            first: None,
        };
        let size = keys
            .number(&format!("{periphery}_font_size"))
            .or_else(|| keys.number("base_font_size"))
            .unwrap_or(10.0);
        let padding = adoc_theme::sides(keys.get(&format!("{periphery}_padding")), 0.0);
        let text = size * 1.2;
        let align = keys.text(&format!("{periphery}_vertical_align"));
        // From the page's edge to the text's nearer side, as the PDF places it in its box.
        let distance = match (periphery, align.as_deref()) {
            ("header", Some("top")) => padding[0],
            ("header", Some("bottom")) => height - padding[2] - text,
            ("footer", Some("top")) => height - padding[0] - text,
            ("footer", Some("bottom")) => padding[2],
            _ => (height - text) / 2.0,
        };
        let distance = Some((distance.max(0.0) * 20.0).round() as u32);
        let drawn = keys
            .number(&format!("{periphery}_border_width"))
            .is_some_and(|width| width > 0.0);
        let rule = keys
            .colour(&format!("{periphery}_border_color"))
            .or_else(|| keys.colour("base_border_color"))
            .zip(keys.eighths(&format!("{periphery}_border_width")))
            .filter(|_| drawn)
            .map(|(colour, width)| Border {
                colour,
                width,
                sides: if periphery == "header" {
                    BorderSides::Bottom
                } else {
                    BorderSides::Top
                },
            });
        let style = StyleLook {
            border: rule,
            ..keys.font(periphery)
        };
        if periphery == "header" {
            look.header = Some(slots);
            look.header_distance = distance;
            styles.insert("Header".to_owned(), style);
        } else {
            look.footer = Some(slots);
            look.footer_distance = distance;
            styles.insert("Footer".to_owned(), style);
        }
    }
    look
}

/// A slot's content in the writer's own placeholders: `{page-number}` and the other page
/// values as they are, the document's attributes substituted (a line naming one that is not set
/// is dropped, as asciidoctor-pdf drops it), bold, italic and monospace marks left out with
/// their text kept, character references as characters, and an `image:` macro standing alone
/// as `{image:<path>|<width in points>}`, found beside the theme.
fn slot(keys: &Keys<'_>, attributes: &BTreeMap<String, String>, key: &str) -> String {
    let Some(content) = keys
        .get(key)
        .filter(|value| !value.is_null())
        .map(ToString::to_string)
    else {
        return String::new();
    };
    let content = content.trim();
    if let Some(image) = image_macro(content, keys.theme.dir.as_deref(), attributes) {
        return image;
    }
    content
        .split('\n')
        .filter_map(|line| line_text(line, attributes))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `image:<target>[<attributes>]` standing alone, as the writer's image placeholder.
fn image_macro(
    content: &str,
    theme_dir: Option<&Path>,
    attributes: &BTreeMap<String, String>,
) -> Option<String> {
    let rest = content.strip_prefix("image:")?;
    let (target, attrs) = rest.strip_suffix(']')?.split_once('[')?;
    // `{imagesdir}/logo.png` and the like: a leading attribute reference is the theme's folder.
    let target = match target
        .strip_prefix('{')
        .and_then(|rest| rest.split_once("}/"))
    {
        Some((_, target)) => target,
        None => target,
    };
    let target = substitute(target, attributes);
    if target.is_empty() || target.contains(char::is_whitespace) || target.contains("://") {
        return None;
    }
    let path = PathBuf::from(&target);
    let path = match theme_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path,
    };
    let width = ["pdfwidth", "scaledwidth", "width"]
        .into_iter()
        .find_map(|name| {
            attrs.split(',').find_map(|attribute| {
                let (key, value) = attribute.split_once('=')?;
                (key.trim() == name).then(|| points(value.trim())).flatten()
            })
        });
    Some(match width {
        Some(width) => format!("{{image:{}|{width}}}", path.display()),
        None => format!("{{image:{}}}", path.display()),
    })
}

/// A length with an absolute unit (`in`, `mm`, `cm`, `pt`, `px`, `pc`), in points.
fn points(value: &str) -> Option<f64> {
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    adoc_theme::to_pt(number.parse().ok()?, unit)
        .filter(|points| points.is_finite() && *points > 0.0)
}

/// Attribute references replaced; unknown ones left as they are.
fn substitute(text: &str, attributes: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}').map(|end| start + end) else {
            break;
        };
        out.push_str(&rest[..start]);
        match attributes.get(&rest[start + 1..end]) {
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[start..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// One line of running content in the writer's placeholders, or `None` when it names an
/// attribute that is not set.
fn line_text(line: &str, attributes: &BTreeMap<String, String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}').map(|end| start + end) else {
            break;
        };
        out.push_str(&rest[..start]);
        let name = &rest[start + 1..end];
        let is_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !is_name || DYNAMIC.contains(&name) {
            out.push_str(&rest[start..=end]);
        } else {
            out.push_str(attributes.get(name)?);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(without_marks(&character_references(&out)))
}

/// `&#169;` and `&#xA9;` as the characters they name.
fn character_references(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("&#") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let decoded = after.find(';').and_then(|end| {
            let code = &after[..end];
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => code.parse().ok(),
            };
            value.and_then(char::from_u32).map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("&#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Constrained `*bold*`, `_italic_` and `` `monospace` `` with their marks left out.
fn without_marks(text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for mark in ['*', '_', '`'] {
        let mut index = 0;
        while index < chars.len() {
            let opens = chars[index] == mark
                && (index == 0 || !chars[index - 1].is_alphanumeric())
                && chars
                    .get(index + 1)
                    .is_some_and(|next| !next.is_whitespace() && *next != mark);
            if !opens {
                index += 1;
                continue;
            }
            let close = (index + 2..chars.len()).find(|&end| {
                chars[end] == mark
                    && !chars[end - 1].is_whitespace()
                    && chars
                        .get(end + 1)
                        .is_none_or(|next| !next.is_alphanumeric())
            });
            match close {
                Some(close) => {
                    chars.remove(close);
                    chars.remove(index);
                }
                None => index += 1,
            }
        }
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use super::{image_macro, line_text, without_marks};

    #[test]
    fn running_content_is_read_as_the_pdf_reads_it() {
        let attributes: BTreeMap<String, String> =
            [("revnumber".to_owned(), "2.0".to_owned())].into();
        // Page values kept as placeholders, attributes substituted, marks left out.
        assert_eq!(
            line_text("*{page-number}* of {page-count}, v{revnumber}", &attributes).as_deref(),
            Some("{page-number} of {page-count}, v2.0")
        );
        // A line naming an attribute that is not set is dropped.
        assert_eq!(line_text("Draft {unset}", &attributes), None);
        // Half-written or odd text is kept as it is, never a panic.
        for text in [
            "{",
            "{page-number",
            "a } b",
            "{ spaced }",
            "&#;",
            "&#xZZ;",
            "*",
            "_x",
            "`",
        ] {
            assert!(line_text(text, &attributes).is_some(), "{text}");
        }
        assert_eq!(
            line_text("&#169; &#xA9;", &attributes).as_deref(),
            Some("© ©")
        );
        // Marks inside a word are text.
        assert_eq!(
            without_marks("snake_case_name and *bold* 2*3*4"),
            "snake_case_name and bold 2*3*4"
        );

        // An image macro standing alone, beside the theme, with an absolute width.
        let dir = Path::new("/themes");
        // As the platform joins paths.
        let logo = dir.join("logo.png").display().to_string();
        assert_eq!(
            image_macro("image:logo.png[pdfwidth=1in]", Some(dir), &attributes),
            Some(format!("{{image:{logo}|72}}"))
        );
        assert_eq!(
            image_macro("image:{imagesdir}/logo.png[]", Some(dir), &attributes),
            Some(format!("{{image:{logo}}}"))
        );
        // A relative width is not a size Word can take; a remote image is not drawn.
        assert_eq!(
            image_macro("image:logo.png[pdfwidth=50%]", Some(dir), &attributes),
            Some(format!("{{image:{logo}}}"))
        );
        assert_eq!(
            image_macro("image:https://example.com/a.png[]", Some(dir), &attributes),
            None
        );
        assert_eq!(
            image_macro("text image:a.png[]", Some(dir), &attributes),
            None
        );
    }
}
