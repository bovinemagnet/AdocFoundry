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

use std::{collections::BTreeMap, path::PathBuf};

use adoc_theme::{LoadedTheme, ThemeValue};

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
    /// Only on the left, as a quote's bar; all round otherwise.
    pub left_only: bool,
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
    /// The covers when the document names none.
    pub front_cover: Option<PathBuf>,
    pub back_cover: Option<PathBuf>,
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
    /// The look of a PDF theme, as far as Word can show it.
    #[must_use]
    pub fn from_theme(theme: &LoadedTheme) -> Self {
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
                    left_only: false,
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
                        left_only: true,
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
