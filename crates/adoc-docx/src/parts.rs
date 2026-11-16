//! The parts every document has: the main document around its body, the styles and the core
//! properties.

use crate::{xml::Xml, Paper};

pub(crate) const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub(crate) const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(crate) const DOCUMENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
pub(crate) const STYLES_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
pub(crate) const CORE_TYPE: &str = "application/vnd.openxmlformats-package.core-properties+xml";

pub(crate) const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
pub(crate) const CORE_PROPERTIES: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
pub(crate) const STYLES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

/// The deepest heading style: Word has nine.
pub(crate) const DEEPEST_HEADING: u8 = 9;

/// `word/document.xml`: the body, then the section's page size and margins.
pub(crate) fn document(body: Xml, paper: Paper) -> Vec<u8> {
    // Twentieths of a point.
    let (width, height) = match paper {
        Paper::A4 => ("11906", "16838"),
        Paper::Letter => ("12240", "15840"),
    };
    let mut xml = Xml::part();
    xml.open("w:document", &[("xmlns:w", W), ("xmlns:r", R)]);
    xml.open("w:body", &[]);
    xml.append(body);
    xml.open("w:sectPr", &[]);
    xml.empty("w:pgSz", &[("w:w", width), ("w:h", height)]);
    xml.empty(
        "w:pgMar",
        &[
            ("w:top", "1440"),
            ("w:right", "1440"),
            ("w:bottom", "1440"),
            ("w:left", "1440"),
            ("w:header", "708"),
            ("w:footer", "708"),
            ("w:gutter", "0"),
        ],
    );
    xml.close("w:sectPr");
    xml.close("w:body");
    xml.close("w:document");
    xml.into_bytes()
}

/// A paragraph style based on Normal, with its paragraph and run properties as written.
fn paragraph_style(
    xml: &mut Xml,
    id: &str,
    name: &str,
    outline: Option<u8>,
    spacing: (&str, &str),
    size: &str,
    bold: bool,
) {
    xml.open("w:style", &[("w:type", "paragraph"), ("w:styleId", id)]);
    xml.empty("w:name", &[("w:val", name)]);
    xml.empty("w:basedOn", &[("w:val", "Normal")]);
    xml.empty("w:next", &[("w:val", "Normal")]);
    xml.empty("w:qFormat", &[]);
    xml.open("w:pPr", &[]);
    xml.empty("w:keepNext", &[]);
    xml.empty(
        "w:spacing",
        &[("w:before", spacing.0), ("w:after", spacing.1)],
    );
    if let Some(level) = outline {
        xml.empty("w:outlineLvl", &[("w:val", &level.to_string())]);
    }
    xml.close("w:pPr");
    xml.open("w:rPr", &[]);
    if bold {
        xml.empty("w:b", &[]);
        xml.empty("w:bCs", &[]);
    }
    xml.empty("w:sz", &[("w:val", size)]);
    xml.empty("w:szCs", &[("w:val", size)]);
    xml.close("w:rPr");
    xml.close("w:style");
}

/// `word/styles.xml`: Normal, Title, Subtitle and the nine headings. Headings carry Word's
/// built-in names (`heading 1`), so Word treats them as its own, outline levels and table of
/// contents included.
pub(crate) fn styles() -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open("w:styles", &[("xmlns:w", W)]);
    xml.open("w:docDefaults", &[]);
    xml.open("w:rPrDefault", &[]);
    xml.open("w:rPr", &[]);
    xml.empty(
        "w:rFonts",
        &[
            ("w:ascii", "Calibri"),
            ("w:hAnsi", "Calibri"),
            ("w:eastAsia", "Calibri"),
            ("w:cs", "Calibri"),
        ],
    );
    xml.empty("w:sz", &[("w:val", "22")]);
    xml.empty("w:szCs", &[("w:val", "22")]);
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
    xml.open(
        "w:style",
        &[
            ("w:type", "character"),
            ("w:default", "1"),
            ("w:styleId", "DefaultParagraphFont"),
        ],
    );
    xml.empty("w:name", &[("w:val", "Default Paragraph Font")]);
    xml.empty("w:uiPriority", &[("w:val", "1")]);
    xml.empty("w:semiHidden", &[]);
    xml.empty("w:unhideWhenUsed", &[]);
    xml.close("w:style");

    paragraph_style(&mut xml, "Title", "Title", None, ("0", "120"), "56", false);
    paragraph_style(
        &mut xml,
        "Subtitle",
        "Subtitle",
        None,
        ("0", "240"),
        "30",
        false,
    );
    // Sizes in half-points, from 16 pt down to the body's 11 pt.
    let sizes = ["32", "28", "26", "24", "22", "22", "22", "22", "22"];
    for level in 1..=DEEPEST_HEADING {
        paragraph_style(
            &mut xml,
            &format!("Heading{level}"),
            &format!("heading {level}"),
            Some(level - 1),
            ("240", "80"),
            sizes[usize::from(level - 1)],
            true,
        );
    }
    xml.close("w:styles");
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
