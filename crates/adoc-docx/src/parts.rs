//! The parts every document has: the main document around its body, the styles and the core
//! properties.

use crate::{
    look::{BoxLook, StyleLook, WordLook},
    xml::Xml,
    StyleOverrides,
};

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

/// An element of a style's properties, with its attributes and any elements inside it.
#[derive(Clone, Debug)]
struct Prop {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Prop>,
}

impl Prop {
    fn new(name: &str, attributes: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_owned(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            children: Vec::new(),
        }
    }

    fn write(&self, xml: &mut Xml) {
        let attributes: Vec<(&str, &str)> = self
            .attributes
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        if self.children.is_empty() {
            xml.empty(&self.name, &attributes);
        } else {
            xml.open(&self.name, &attributes);
            for child in &self.children {
                child.write(xml);
            }
            xml.close(&self.name);
        }
    }
}

/// The order the schema gives a paragraph's and a run's properties: Word refuses a document
/// whose properties are out of it.
const PARAGRAPH_ORDER: &[&str] = &[
    "w:pStyle",
    "w:keepNext",
    "w:keepLines",
    "w:pageBreakBefore",
    "w:framePr",
    "w:widowControl",
    "w:numPr",
    "w:suppressLineNumbers",
    "w:pBdr",
    "w:shd",
    "w:tabs",
    "w:suppressAutoHyphens",
    "w:spacing",
    "w:ind",
    "w:contextualSpacing",
    "w:jc",
    "w:textAlignment",
    "w:outlineLvl",
];
const RUN_ORDER: &[&str] = &[
    "w:rStyle",
    "w:rFonts",
    "w:b",
    "w:bCs",
    "w:i",
    "w:iCs",
    "w:caps",
    "w:smallCaps",
    "w:strike",
    "w:color",
    "w:spacing",
    "w:position",
    "w:sz",
    "w:szCs",
    "w:highlight",
    "w:u",
    "w:bdr",
    "w:shd",
    "w:vertAlign",
];

/// `props` in `order`, those it does not list last, as they came.
fn ordered(mut props: Vec<Prop>, order: &[&str]) -> Vec<Prop> {
    props.sort_by_key(|prop| {
        order
            .iter()
            .position(|name| *name == prop.name)
            .unwrap_or(order.len())
    });
    props
}

/// Puts `prop` in place of any property of its name.
fn set(props: &mut Vec<Prop>, prop: Prop) {
    props.retain(|known| known.name != prop.name);
    props.push(prop);
}

/// Sets attribute `key` of the property `name`, adding the property if it is not there.
fn set_attribute(props: &mut Vec<Prop>, name: &str, key: &str, value: &str) {
    let prop = match props.iter_mut().position(|prop| prop.name == name) {
        Some(index) => &mut props[index],
        None => {
            props.push(Prop::new(name, &[]));
            props.last_mut().expect("just pushed")
        }
    };
    prop.attributes.retain(|(known, _)| known != key);
    prop.attributes.push((key.to_owned(), value.to_owned()));
}

/// A run's properties with `look` laid over them.
fn run_props(props: &[Property<'_>], look: Option<&StyleLook>) -> Vec<Prop> {
    let mut props: Vec<Prop> = props
        .iter()
        .map(|(name, attributes)| Prop::new(name, attributes))
        .collect();
    if let Some(look) = look {
        if let Some(font) = &look.font {
            let font = font.as_str();
            set(
                &mut props,
                Prop::new(
                    "w:rFonts",
                    &[
                        ("w:ascii", font),
                        ("w:hAnsi", font),
                        ("w:eastAsia", font),
                        ("w:cs", font),
                    ],
                ),
            );
        }
        let switch = |props: &mut Vec<Prop>, names: [&str; 2], on: Option<bool>| {
            if let Some(on) = on {
                for name in names {
                    let attributes: &[(&str, &str)] = if on { &[] } else { &[("w:val", "0")] };
                    set(props, Prop::new(name, attributes));
                }
            }
        };
        switch(&mut props, ["w:b", "w:bCs"], look.bold);
        switch(&mut props, ["w:i", "w:iCs"], look.italic);
        if let Some(caps) = look.caps {
            let attributes: &[(&str, &str)] = if caps { &[] } else { &[("w:val", "0")] };
            set(&mut props, Prop::new("w:caps", attributes));
        }
        if let Some(colour) = &look.colour {
            set(&mut props, Prop::new("w:color", &[("w:val", colour)]));
        }
        if let Some(size) = look.size {
            let size = size.to_string();
            set(&mut props, Prop::new("w:sz", &[("w:val", &size)]));
            set(&mut props, Prop::new("w:szCs", &[("w:val", &size)]));
        }
        if let Some(underline) = look.underline {
            let value = if underline { "single" } else { "none" };
            set(&mut props, Prop::new("w:u", &[("w:val", value)]));
        }
    }
    ordered(props, RUN_ORDER)
}

/// The `w:shd` of a background.
fn shading(fill: &str) -> Prop {
    Prop::new(
        "w:shd",
        &[("w:val", "clear"), ("w:color", "auto"), ("w:fill", fill)],
    )
}

/// A paragraph's properties with `look` laid over them.
fn paragraph_props(props: &[Property<'_>], look: Option<&StyleLook>) -> Vec<Prop> {
    let mut props: Vec<Prop> = props
        .iter()
        .map(|(name, attributes)| Prop::new(name, attributes))
        .collect();
    if let Some(look) = look {
        if let Some(before) = look.before {
            set_attribute(&mut props, "w:spacing", "w:before", &before.to_string());
        }
        if let Some(after) = look.after {
            set_attribute(&mut props, "w:spacing", "w:after", &after.to_string());
        }
        if let Some(line) = look.line {
            set_attribute(&mut props, "w:spacing", "w:line", &line.to_string());
            set_attribute(&mut props, "w:spacing", "w:lineRule", "auto");
        }
        if let Some(align) = &look.align {
            set(&mut props, Prop::new("w:jc", &[("w:val", align)]));
        }
        if let Some(fill) = &look.shading {
            set(&mut props, shading(fill));
        }
        if let Some(border) = &look.border {
            let width = border.width.to_string();
            let sides: &[&str] = if border.left_only {
                &["left"]
            } else {
                &["top", "left", "bottom", "right"]
            };
            let mut box_prop = Prop::new("w:pBdr", &[]);
            for side in sides {
                box_prop.children.push(Prop::new(
                    &format!("w:{side}"),
                    &[
                        ("w:val", "single"),
                        ("w:sz", &width),
                        ("w:space", "4"),
                        ("w:color", &border.colour),
                    ],
                ));
            }
            set(&mut props, box_prop);
        }
    }
    ordered(props, PARAGRAPH_ORDER)
}

fn write_style(
    xml: &mut Xml,
    style: &Style<'_>,
    look: Option<&StyleLook>,
    boxed: Option<&BoxLook>,
) {
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
    // A character style's look is its run's: a background is the run's shading.
    let paragraph = if style.kind == "character" {
        Vec::new()
    } else {
        paragraph_props(style.paragraph, look)
    };
    let mut run = run_props(style.run, look);
    if style.kind == "character" {
        if let Some(fill) = look.and_then(|look| look.shading.as_ref()) {
            set(&mut run, shading(fill));
            run = ordered(run, RUN_ORDER);
        }
    }
    for (element, properties) in [("w:pPr", &paragraph), ("w:rPr", &run)] {
        if !properties.is_empty() {
            xml.open(element, &[]);
            for property in properties {
                property.write(xml);
            }
            xml.close(element);
        }
    }
    if !style.table.is_empty() {
        let boxed = boxed.cloned().unwrap_or_default();
        xml.open("w:tblPr", &[]);
        if boxed.stripe.is_some() {
            xml.empty("w:tblStyleRowBandSize", &[("w:val", "1")]);
        }
        for borders in style.table {
            xml.open("w:tblBorders", &[]);
            for (index, side) in BORDERS.split(' ').enumerate() {
                let (value, size, colour) = match *borders {
                    "all" => ("single", "4", "BFBFBF"),
                    "box" if index < 4 => ("single", "4", "BFBFBF"),
                    "left-bar" if side == "left" => ("single", "24", "7F7F7F"),
                    _ => ("nil", "0", "auto"),
                };
                let size = match (value, boxed.border_width) {
                    ("single", Some(width)) => width.to_string(),
                    _ => size.to_owned(),
                };
                let colour = match (value, &boxed.border_colour) {
                    ("single", Some(colour)) => colour.as_str(),
                    _ => colour,
                };
                xml.empty(
                    &format!("w:{side}"),
                    &[
                        ("w:val", value),
                        ("w:sz", &size),
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
        let mut cell: Vec<Prop> = style
            .cell
            .iter()
            .map(|(name, attributes)| Prop::new(name, attributes))
            .collect();
        if let Some(fill) = &boxed.background {
            set(&mut cell, shading(fill));
        }
        if !cell.is_empty() {
            xml.open("w:tcPr", &[]);
            for property in &cell {
                property.write(xml);
            }
            xml.close("w:tcPr");
        }
        // The header row and the stripes, as Word's conditional formatting.
        if boxed.head_background.is_some()
            || boxed.head_colour.is_some()
            || boxed.head_bold.is_some()
        {
            xml.open("w:tblStylePr", &[("w:type", "firstRow")]);
            let head = StyleLook {
                colour: boxed.head_colour.clone(),
                bold: boxed.head_bold,
                ..StyleLook::default()
            };
            let run = run_props(&[], Some(&head));
            if !run.is_empty() {
                xml.open("w:rPr", &[]);
                for property in &run {
                    property.write(xml);
                }
                xml.close("w:rPr");
            }
            if let Some(fill) = &boxed.head_background {
                xml.open("w:tcPr", &[]);
                shading(fill).write(xml);
                xml.close("w:tcPr");
            }
            xml.close("w:tblStylePr");
        }
        if let Some(fill) = &boxed.stripe {
            xml.open("w:tblStylePr", &[("w:type", "band2Horz")]);
            xml.open("w:tcPr", &[]);
            shading(fill).write(xml);
            xml.close("w:tcPr");
            xml.close("w:tblStylePr");
        }
    } else if !style.cell.is_empty() {
        xml.open("w:tcPr", &[]);
        for (name, attributes) in style.cell {
            xml.empty(name, attributes);
        }
        xml.close("w:tcPr");
    }
    xml.close("w:style");
}

/// The look written `[docx.styles]` keys give each style, laid over a theme's.
fn written_look(overrides: &StyleOverrides, id: &str) -> StyleLook {
    let heading = id == "Title"
        || id == "Subtitle"
        || id == "TOCHeading"
        || id
            .strip_prefix("Heading")
            .is_some_and(|level| level.parse::<u8>().is_ok());
    let code = matches!(id, "CodeChar" | "Keyboard" | "SourceCode" | "LiteralBlock");
    StyleLook {
        font: if heading {
            overrides.heading_font.clone()
        } else if code {
            overrides.code_font.clone()
        } else {
            None
        },
        colour: if heading {
            overrides.heading_colour.clone()
        } else if id == "Hyperlink" {
            overrides.link_colour.clone()
        } else {
            None
        },
        ..StyleLook::default()
    }
}

/// `word/styles.xml`. Headings carry Word's built-in names (`heading 1`), so Word treats them
/// as its own, outline levels and table of contents included; every other style the body uses
/// is named for what it holds, so a reader can restyle it in Word. `width` is the text's, in
/// twentieths of a point: headers and footers have their centre and right tab stops by it, and
/// the table of contents its page numbers.
pub(crate) fn styles(width: u32, overrides: &StyleOverrides, look: &WordLook) -> Vec<u8> {
    // Each style's look: the theme's, then what `[docx.styles]` wrote.
    let style = |xml: &mut Xml, style: &Style<'_>| {
        let theme = look.style(style.id).cloned().unwrap_or_default();
        let effective = theme.with(&written_look(overrides, style.id));
        let effective = (!effective.is_empty()).then_some(effective);
        write_style(xml, style, effective.as_ref(), look.boxes.get(style.id));
    };
    // The document's defaults: the theme's body, then what `[docx.styles]` wrote.
    let defaults = look.defaults.with(&StyleLook {
        font: overrides.body_font.clone(),
        size: overrides.body_size,
        ..StyleLook::default()
    });
    let body_font = defaults.font.as_deref().unwrap_or("Calibri");
    let code_font = overrides.code_font.as_deref().unwrap_or("Consolas");
    let body_size = defaults.size.unwrap_or(22).to_string();
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
    if let Some(colour) = &defaults.colour {
        xml.empty("w:color", &[("w:val", colour)]);
    }
    xml.empty("w:sz", &[("w:val", &body_size)]);
    xml.empty("w:szCs", &[("w:val", &body_size)]);
    xml.close("w:rPr");
    xml.close("w:rPrDefault");
    xml.open("w:pPrDefault", &[]);
    xml.open("w:pPr", &[]);
    let line = defaults.line.unwrap_or(259).to_string();
    xml.empty(
        "w:spacing",
        &[
            ("w:after", "160"),
            ("w:line", &line),
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
    if let Some(align) = &defaults.align {
        xml.open("w:pPr", &[]);
        xml.empty("w:jc", &[("w:val", align)]);
        xml.close("w:pPr");
    }
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
