//! The parts every document has: the main document around its body, the styles and the core
//! properties.

use crate::{xml::Xml, StyleOverrides};

pub(crate) const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub(crate) const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(crate) const DOCUMENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
pub(crate) const STYLES_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
pub(crate) const CORE_TYPE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
pub(crate) const SETTINGS_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
pub(crate) const NUMBERING_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
pub(crate) const FOOTNOTES_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

pub(crate) const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
pub(crate) const CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
pub(crate) const STYLES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
pub(crate) const SETTINGS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
pub(crate) const NUMBERING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
pub(crate) const FOOTNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
pub(crate) const HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
pub(crate) const IMAGE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// The namespaces of a part holding body content: WordprocessingML, relationships, and the
/// DrawingML its pictures are written in.
pub(crate) const NAMESPACES: [(&str, &str); 5] = [
    ("xmlns:w", W),
    ("xmlns:r", R),
    (
        "xmlns:wp",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    ),
    (
        "xmlns:a",
        "http://schemas.openxmlformats.org/drawingml/2006/main",
    ),
    (
        "xmlns:pic",
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
    ),
];

/// The deepest heading style: Word has nine.
pub(crate) const DEEPEST_HEADING: u8 = 9;

/// `word/document.xml`: the body, then the last section's properties.
pub(crate) fn document(body: Xml, last_section: Xml) -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open("w:document", &NAMESPACES);
    xml.open("w:body", &[]);
    xml.append(body);
    xml.append(last_section);
    xml.close("w:body");
    xml.close("w:document");
    xml.into_bytes()
}

/// A style's definition: its kind, id and name, what it is based on, and the inside of its
/// paragraph, run, table and cell properties, each written in the schema's order.
struct Style<'a> {
    kind: &'a str,
    id: &'a str,
    name: &'a str,
    based_on: Option<&'a str>,
    next: Option<&'a str>,
    hidden: bool,
    paragraph: &'a [(&'a str, &'a [(&'a str, &'a str)])],
    run: &'a [(&'a str, &'a [(&'a str, &'a str)])],
    table: &'a [&'a str],
    cell: &'a [(&'a str, &'a [(&'a str, &'a str)])],
}

impl Default for Style<'_> {
    fn default() -> Self {
        Self {
            kind: "paragraph",
            id: "",
            name: "",
            based_on: Some("Normal"),
            next: None,
            hidden: false,
            paragraph: &[],
            run: &[],
            table: &[],
            cell: &[],
        }
    }
}

/// A run property: its element and attributes.
type Property<'a> = (&'a str, &'a [(&'a str, &'a str)]);

/// A heading's run properties in the schema's order: its font, bold, its colour, its size.
fn heading_run<'a>(
    fonts: Option<&'a [(&'a str, &'a str)]>,
    bold: bool,
    colour: Option<&'a [(&'a str, &'a str)]>,
    size: &'a [(&'a str, &'a str)],
) -> Vec<Property<'a>> {
    let mut run = Vec::new();
    if let Some(fonts) = fonts {
        run.push(("w:rFonts", fonts));
    }
    if bold {
        run.extend(BOLD);
    }
    if let Some(colour) = colour {
        run.push(("w:color", colour));
    }
    run.push(("w:sz", size));
    run.push(("w:szCs", size));
    run
}
const BOLD: [(&str, &[(&str, &str)]); 2] = [("w:b", &[]), ("w:bCs", &[])];
const ITALIC: [(&str, &[(&str, &str)]); 2] = [("w:i", &[]), ("w:iCs", &[])];
/// A light border, on every side of a table and between its cells.
const BORDERS: &str = "top left bottom right insideH insideV";

fn style(xml: &mut Xml, style: &Style<'_>) {
    xml.open(
        "w:style",
        &[("w:type", style.kind), ("w:styleId", style.id)],
    );
    xml.empty("w:name", &[("w:val", style.name)]);
    if let Some(based_on) = style.based_on {
        xml.empty("w:basedOn", &[("w:val", based_on)]);
    }
    if let Some(next) = style.next {
        xml.empty("w:next", &[("w:val", next)]);
    }
    if style.hidden {
        xml.empty("w:uiPriority", &[("w:val", "99")]);
        xml.empty("w:semiHidden", &[]);
        xml.empty("w:unhideWhenUsed", &[]);
    } else {
        xml.empty("w:qFormat", &[]);
    }
    for (element, properties) in [("w:pPr", style.paragraph), ("w:rPr", style.run)] {
        if !properties.is_empty() {
            xml.open(element, &[]);
            for (name, attributes) in properties {
                xml.empty(name, attributes);
            }
            xml.close(element);
        }
    }
    if !style.table.is_empty() {
        xml.open("w:tblPr", &[]);
        for borders in style.table {
            xml.open("w:tblBorders", &[]);
            for (index, side) in BORDERS.split(' ').enumerate() {
                let (value, size, colour) = match *borders {
                    "all" => ("single", "4", "BFBFBF"),
                    "box" if index < 4 => ("single", "4", "BFBFBF"),
                    "left-bar" if side == "left" => ("single", "24", "7F7F7F"),
                    _ => ("nil", "0", "auto"),
                };
                xml.empty(
                    &format!("w:{side}"),
                    &[
                        ("w:val", value),
                        ("w:sz", size),
                        ("w:space", "0"),
                        ("w:color", colour),
                    ],
                );
            }
            xml.close("w:tblBorders");
        }
        xml.open("w:tblCellMar", &[]);
        for (side, size) in [
            ("top", "57"),
            ("left", "108"),
            ("bottom", "57"),
            ("right", "108"),
        ] {
            xml.empty(&format!("w:{side}"), &[("w:w", size), ("w:type", "dxa")]);
        }
        xml.close("w:tblCellMar");
        xml.close("w:tblPr");
    }
    if !style.cell.is_empty() {
        xml.open("w:tcPr", &[]);
        for (name, attributes) in style.cell {
            xml.empty(name, attributes);
        }
        xml.close("w:tcPr");
    }
    xml.close("w:style");
}

/// `word/styles.xml`. Headings carry Word's built-in names (`heading 1`), so Word treats them
/// as its own, outline levels and table of contents included; every other style the body uses
/// is named for what it holds, so a reader can restyle it in Word. `width` is the text's, in
/// twentieths of a point: headers and footers have their centre and right tab stops by it, and
/// the table of contents its page numbers.
pub(crate) fn styles(width: u32, overrides: &StyleOverrides) -> Vec<u8> {
    let body_font = overrides.body_font.as_deref().unwrap_or("Calibri");
    let code_font = overrides.code_font.as_deref().unwrap_or("Consolas");
    let body_size = overrides.body_size.unwrap_or(22).to_string();
    let code_fonts = [
        ("w:ascii", code_font),
        ("w:hAnsi", code_font),
        ("w:cs", code_font),
    ];
    let mono: Property = ("w:rFonts", &code_fonts);
    let heading_fonts = overrides
        .heading_font
        .as_deref()
        .map(|font| [("w:ascii", font), ("w:hAnsi", font), ("w:cs", font)]);
    let heading_fonts = heading_fonts.as_ref().map(|fonts| &fonts[..]);
    let heading_colour = overrides
        .heading_colour
        .as_deref()
        .map(|colour| [("w:val", colour)]);
    let heading_colour = heading_colour.as_ref().map(|colour| &colour[..]);
    let link_colour = [(
        "w:val",
        overrides.link_colour.as_deref().unwrap_or("0563C1"),
    )];
    let mut xml = Xml::part();
    xml.open("w:styles", &[("xmlns:w", W)]);
    xml.open("w:docDefaults", &[]);
    xml.open("w:rPrDefault", &[]);
    xml.open("w:rPr", &[]);
    xml.empty(
        "w:rFonts",
        &[
            ("w:ascii", body_font),
            ("w:hAnsi", body_font),
            ("w:eastAsia", body_font),
            ("w:cs", body_font),
        ],
    );
    xml.empty("w:sz", &[("w:val", &body_size)]);
    xml.empty("w:szCs", &[("w:val", &body_size)]);
    xml.close("w:rPr");
    xml.close("w:rPrDefault");
    xml.open("w:pPrDefault", &[]);
    xml.open("w:pPr", &[]);
    xml.empty(
        "w:spacing",
        &[
            ("w:after", "160"),
            ("w:line", "259"),
            ("w:lineRule", "auto"),
        ],
    );
    xml.close("w:pPr");
    xml.close("w:pPrDefault");
    xml.close("w:docDefaults");

    xml.open(
        "w:style",
        &[
            ("w:type", "paragraph"),
            ("w:default", "1"),
            ("w:styleId", "Normal"),
        ],
    );
    xml.empty("w:name", &[("w:val", "Normal")]);
    xml.empty("w:qFormat", &[]);
    xml.close("w:style");
    style(
        &mut xml,
        &Style {
            kind: "character",
            id: "DefaultParagraphFont",
            name: "Default Paragraph Font",
            based_on: None,
            hidden: true,
            ..Style::default()
        },
    );
    xml.open(
        "w:style",
        &[
            ("w:type", "table"),
            ("w:default", "1"),
            ("w:styleId", "TableNormal"),
        ],
    );
    xml.empty("w:name", &[("w:val", "Normal Table")]);
    xml.empty("w:uiPriority", &[("w:val", "99")]);
    xml.empty("w:semiHidden", &[]);
    xml.empty("w:unhideWhenUsed", &[]);
    xml.open("w:tblPr", &[]);
    xml.empty("w:tblInd", &[("w:w", "0"), ("w:type", "dxa")]);
    xml.close("w:tblPr");
    xml.close("w:style");

    let keep_next: (&str, &[(&str, &str)]) = ("w:keepNext", &[]);
    style(
        &mut xml,
        &Style {
            id: "Title",
            name: "Title",
            next: Some("Normal"),
            paragraph: &[("w:spacing", &[("w:before", "0"), ("w:after", "120")])],
            run: &heading_run(heading_fonts, false, heading_colour, &[("w:val", "56")]),
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "Subtitle",
            name: "Subtitle",
            next: Some("Normal"),
            paragraph: &[("w:spacing", &[("w:before", "0"), ("w:after", "240")])],
            run: &heading_run(heading_fonts, false, heading_colour, &[("w:val", "30")]),
            ..Style::default()
        },
    );
    // Sizes in half-points, from 16 pt down to the body's 11 pt.
    let sizes = ["32", "28", "26", "24", "22", "22", "22", "22", "22"];
    for level in 1..=DEEPEST_HEADING {
        let outline = (level - 1).to_string();
        let size = sizes[usize::from(level - 1)];
        style(
            &mut xml,
            &Style {
                id: &format!("Heading{level}"),
                name: &format!("heading {level}"),
                next: Some("Normal"),
                paragraph: &[
                    keep_next,
                    ("w:spacing", &[("w:before", "240"), ("w:after", "80")]),
                    ("w:outlineLvl", &[("w:val", &outline)]),
                ],
                run: &heading_run(heading_fonts, true, heading_colour, &[("w:val", size)]),
                ..Style::default()
            },
        );
    }
    let small: [(&str, &[(&str, &str)]); 2] =
        [("w:sz", &[("w:val", "20")]), ("w:szCs", &[("w:val", "20")])];
    style(
        &mut xml,
        &Style {
            id: "Caption",
            name: "caption",
            next: Some("Normal"),
            paragraph: &[keep_next, ("w:spacing", &[("w:after", "80")])],
            run: &[ITALIC[0], ITALIC[1], small[0], small[1]],
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "Quote",
            name: "Quote",
            paragraph: &[("w:ind", &[("w:left", "720"), ("w:right", "720")])],
            run: &ITALIC,
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "QuoteAttribution",
            name: "Quote Attribution",
            paragraph: &[
                ("w:ind", &[("w:left", "720"), ("w:right", "720")]),
                ("w:jc", &[("w:val", "right")]),
            ],
            run: &small,
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "Verse",
            name: "Verse",
            paragraph: &[("w:ind", &[("w:left", "720")])],
            ..Style::default()
        },
    );
    let code_paragraph: [(&str, &[(&str, &str)]); 3] = [
        ("w:keepLines", &[]),
        (
            "w:shd",
            &[
                ("w:val", "clear"),
                ("w:color", "auto"),
                ("w:fill", "F5F5F5"),
            ],
        ),
        ("w:spacing", &[("w:line", "240"), ("w:lineRule", "auto")]),
    ];
    let code_run: [(&str, &[(&str, &str)]); 3] = [
        mono,
        ("w:sz", &[("w:val", "19")]),
        ("w:szCs", &[("w:val", "19")]),
    ];
    for (id, name) in [
        ("SourceCode", "Source Code"),
        ("LiteralBlock", "Literal Block"),
    ] {
        style(
            &mut xml,
            &Style {
                id,
                name,
                paragraph: &code_paragraph,
                run: &code_run,
                ..Style::default()
            },
        );
    }
    style(
        &mut xml,
        &Style {
            id: "DescriptionTerm",
            name: "Description Term",
            next: Some("Normal"),
            paragraph: &[keep_next, ("w:spacing", &[("w:after", "40")])],
            run: &BOLD,
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "FootnoteText",
            name: "footnote text",
            paragraph: &[(
                "w:spacing",
                &[("w:after", "0"), ("w:line", "240"), ("w:lineRule", "auto")],
            )],
            run: &small,
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "TableSpacer",
            name: "Table Spacer",
            hidden: true,
            paragraph: &[(
                "w:spacing",
                &[("w:after", "0"), ("w:line", "120"), ("w:lineRule", "exact")],
            )],
            run: &[("w:sz", &[("w:val", "4")]), ("w:szCs", &[("w:val", "4")])],
            ..Style::default()
        },
    );
    // Headers and footers: a centre and a right tab stop, for their three slots.
    let centre = (width / 2).to_string();
    let right = width.to_string();
    for (id, name) in [("Header", "header"), ("Footer", "footer")] {
        xml.open("w:style", &[("w:type", "paragraph"), ("w:styleId", id)]);
        xml.empty("w:name", &[("w:val", name)]);
        xml.empty("w:basedOn", &[("w:val", "Normal")]);
        xml.empty("w:uiPriority", &[("w:val", "99")]);
        xml.empty("w:unhideWhenUsed", &[]);
        xml.open("w:pPr", &[]);
        xml.open("w:tabs", &[]);
        xml.empty("w:tab", &[("w:val", "center"), ("w:pos", &centre)]);
        xml.empty("w:tab", &[("w:val", "right"), ("w:pos", &right)]);
        xml.close("w:tabs");
        xml.empty(
            "w:spacing",
            &[("w:after", "0"), ("w:line", "240"), ("w:lineRule", "auto")],
        );
        xml.close("w:pPr");
        xml.open("w:rPr", &[]);
        xml.empty("w:sz", &[("w:val", "18")]);
        xml.empty("w:szCs", &[("w:val", "18")]);
        xml.close("w:rPr");
        xml.close("w:style");
    }
    // The table of contents: its heading (not itself listed), and an entry style per level,
    // its page number at the right after a row of dots.
    style(
        &mut xml,
        &Style {
            id: "TOCHeading",
            name: "TOC Heading",
            next: Some("Normal"),
            paragraph: &[
                keep_next,
                ("w:spacing", &[("w:before", "240"), ("w:after", "120")]),
            ],
            run: &heading_run(heading_fonts, true, heading_colour, &[("w:val", "32")]),
            ..Style::default()
        },
    );
    for level in 1..=DEEPEST_HEADING {
        let indent = (220 * u32::from(level - 1)).to_string();
        xml.open(
            "w:style",
            &[
                ("w:type", "paragraph"),
                ("w:styleId", &format!("TOC{level}")),
            ],
        );
        xml.empty("w:name", &[("w:val", &format!("toc {level}"))]);
        xml.empty("w:basedOn", &[("w:val", "Normal")]);
        xml.empty("w:next", &[("w:val", "Normal")]);
        xml.empty("w:uiPriority", &[("w:val", "39")]);
        xml.empty("w:unhideWhenUsed", &[]);
        xml.open("w:pPr", &[]);
        xml.open("w:tabs", &[]);
        xml.empty(
            "w:tab",
            &[("w:val", "right"), ("w:leader", "dot"), ("w:pos", &right)],
        );
        xml.close("w:tabs");
        xml.empty("w:spacing", &[("w:after", "60")]);
        xml.empty("w:ind", &[("w:left", &indent)]);
        xml.close("w:pPr");
        xml.close("w:style");
    }
    style(
        &mut xml,
        &Style {
            id: "CoverImage",
            name: "Cover Image",
            hidden: true,
            paragraph: &[
                (
                    "w:spacing",
                    &[
                        ("w:before", "0"),
                        ("w:after", "0"),
                        ("w:line", "240"),
                        ("w:lineRule", "auto"),
                    ],
                ),
                ("w:jc", &[("w:val", "center")]),
            ],
            ..Style::default()
        },
    );
    style(
        &mut xml,
        &Style {
            id: "TitlePageDetail",
            name: "Title Page Detail",
            paragraph: &[("w:spacing", &[("w:after", "80")])],
            run: &[("w:sz", &[("w:val", "24")]), ("w:szCs", &[("w:val", "24")])],
            ..Style::default()
        },
    );
    for (id, name, run) in [
        ("CodeChar", "Code", &[mono][..]),
        (
            "Keyboard",
            "Keyboard",
            &[
                mono,
                (
                    "w:bdr",
                    &[
                        ("w:val", "single"),
                        ("w:sz", "4"),
                        ("w:space", "0"),
                        ("w:color", "BFBFBF"),
                    ],
                ),
            ][..],
        ),
        (
            "Hyperlink",
            "Hyperlink",
            &[
                ("w:color", &link_colour[..]),
                ("w:u", &[("w:val", "single")][..]),
            ][..],
        ),
        (
            "FootnoteReference",
            "footnote reference",
            &[("w:vertAlign", &[("w:val", "superscript")][..])][..],
        ),
    ] {
        style(
            &mut xml,
            &Style {
                kind: "character",
                id,
                name,
                based_on: Some("DefaultParagraphFont"),
                run,
                ..Style::default()
            },
        );
    }
    for (id, name, borders, cell) in [
        ("AdocTable", "AsciiDoc Table", "all", &[][..]),
        ("AdocExample", "AsciiDoc Example", "box", &[][..]),
        (
            "AdocSidebar",
            "AsciiDoc Sidebar",
            "box",
            &[(
                "w:shd",
                &[
                    ("w:val", "clear"),
                    ("w:color", "auto"),
                    ("w:fill", "F2F2F2"),
                ][..],
            )][..],
        ),
        ("AdocAdmonition", "AsciiDoc Admonition", "left-bar", &[][..]),
    ] {
        style(
            &mut xml,
            &Style {
                kind: "table",
                id,
                name,
                based_on: Some("TableNormal"),
                paragraph: &[("w:spacing", &[("w:after", "80")])],
                table: &[borders],
                cell,
                ..Style::default()
            },
        );
    }
    xml.close("w:styles");
    xml.into_bytes()
}

/// `word/settings.xml`: odd and even pages' own headers and footers, when asked for; whether
/// Word updates the fields on opening (when there is a table of contents, whose page numbers
/// only Word can work out, it asks); the footnotes' separators; and Word's current
/// compatibility mode, so it opens the document without "compatibility mode".
pub(crate) fn settings(footnotes: bool, odd_even: bool, update_fields: bool) -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open("w:settings", &[("xmlns:w", W)]);
    if odd_even {
        xml.empty("w:evenAndOddHeaders", &[]);
    }
    if update_fields {
        xml.empty("w:updateFields", &[("w:val", "true")]);
    }
    if footnotes {
        xml.open("w:footnotePr", &[]);
        xml.empty("w:footnote", &[("w:id", "-1")]);
        xml.empty("w:footnote", &[("w:id", "0")]);
        xml.close("w:footnotePr");
    }
    xml.open("w:compat", &[]);
    xml.empty(
        "w:compatSetting",
        &[
            ("w:name", "compatibilityMode"),
            ("w:uri", "http://schemas.microsoft.com/office/word"),
            ("w:val", "15"),
        ],
    );
    xml.close("w:compat");
    xml.close("w:settings");
    xml.into_bytes()
}

/// The facts `docProps/core.xml` holds. No date: the same document gives the same bytes.
#[derive(Default)]
pub(crate) struct Facts {
    pub(crate) title: Option<String>,
    pub(crate) creator: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) keywords: Option<String>,
}

pub(crate) fn core(facts: &Facts) -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open(
        "cp:coreProperties",
        &[
            (
                "xmlns:cp",
                "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
            ),
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
            ("xmlns:dcterms", "http://purl.org/dc/terms/"),
        ],
    );
    for (name, value) in [
        ("dc:title", &facts.title),
        ("dc:creator", &facts.creator),
        ("dc:description", &facts.description),
        ("cp:keywords", &facts.keywords),
    ] {
        if let Some(value) = value.as_deref().filter(|value| !value.trim().is_empty()) {
            xml.element(name, &[], value);
        }
    }
    xml.close("cp:coreProperties");
    xml.into_bytes()
}
