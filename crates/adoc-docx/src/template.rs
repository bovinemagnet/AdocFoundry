//! A Word template (`.dotx`, or a `.docx` used as one), read as untrusted input (the Word looks
//! PRD §5.3): limited in size, refused when it can hold macros, and kept only part by part from
//! what the export uses: its styles, numbering, theme, font table with the fonts it embeds, the
//! last section's page, and the headers and footers that section shows, with their pictures.
//! Every kept XML part is checked well formed. A header or footer is left out, with a warning,
//! when it relates to anything but a picture inside the package or a link, or holds a field
//! other than those that only show the document's own facts, so nothing a template holds can
//! fetch, include or run anything when Word opens the export.

use std::{collections::BTreeMap, fmt};

use roxmltree::{Document, Node};

use crate::{
    parts::{R, W},
    unzip::{self, UnzipError},
    Margins,
};

/// The largest template read.
pub const MAX_TEMPLATE_BYTES: usize = 32 * 1024 * 1024;

const RELATIONSHIPS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CONTENT_TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
pub(crate) const THEME: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
pub(crate) const THEME_TYPE: &str = "application/vnd.openxmlformats-officedocument.theme+xml";

/// The main parts a template may have: a document or a template, never macro-enabled.
const MAIN_TYPES: [&str; 2] = [
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
];

/// The pictures a header, footer or picture bullet may show: content type and the extension
/// the copy is given.
const PICTURE_TYPES: [(&str, &str); 8] = [
    ("image/png", "png"),
    ("image/jpeg", "jpeg"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
    ("image/tiff", "tiff"),
    ("image/x-emf", "emf"),
    ("image/x-wmf", "wmf"),
    ("image/svg+xml", "svg"),
];

const DRAWINGML: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

/// Whether an attribute names an address outside the package: a URL, a `file:` path or a
/// network share. DrawingML's extension `uri`s name schemas, not places, and are allowed.
fn names_an_address(name: &str, value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    // Alternative text and names are only shown.
    if matches!(name, "descr" | "title" | "name")
        || (name == "uri" && value.starts_with("http://schemas."))
    {
        return false;
    }
    value.contains("://") || value.starts_with("file:") || value.starts_with("\\\\")
}

/// Whether an SVG picture loads nothing from outside itself: well-formed, no reference but to
/// itself or a `data:` URL, and no style that could import one.
fn closed_svg(data: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(data) else {
        return false;
    };
    let Ok(document) = Document::parse(text) else {
        return false;
    };
    document.descendants().all(|node| {
        let text_ok = node
            .text()
            .is_none_or(|text| !text.contains("url(") && !text.contains("@import"));
        text_ok
            && node.attributes().all(|attribute| {
                let value = attribute.value().trim();
                if attribute.name() == "href" {
                    value.starts_with('#') || value.starts_with("data:")
                } else {
                    !value.contains("://") && !(value.contains("url(") && !value.contains("url(#"))
                }
            })
    })
}

/// The fields a kept header or footer may hold: each shows a fact of the document, its page or
/// the date, and none reads a file, an address or another program.
const FIELDS: [&str; 18] = [
    "PAGE",
    "NUMPAGES",
    "SECTIONPAGES",
    "SECTION",
    "STYLEREF",
    "DATE",
    "TIME",
    "CREATEDATE",
    "SAVEDATE",
    "PRINTDATE",
    "TITLE",
    "SUBJECT",
    "AUTHOR",
    "KEYWORDS",
    "COMMENTS",
    "LASTSAVEDBY",
    "DOCPROPERTY",
    "FILENAME",
];

/// Why a template could not be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateError {
    TooLarge(usize),
    Unreadable(UnzipError),
    /// Macro-enabled (`.dotm`, `.docm`) or holding a VBA project.
    Macros,
    /// Not a WordprocessingML package, or missing what a template needs.
    NotWord(String),
    /// A part the export would use is not well-formed XML.
    Malformed {
        part: String,
        why: String,
    },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(size) => write!(
                formatter,
                "it is too large: {} MB, past the {} MB a template may be",
                size.div_ceil(1024 * 1024),
                MAX_TEMPLATE_BYTES / (1024 * 1024)
            ),
            Self::Unreadable(error) => error.fmt(formatter),
            Self::Macros => formatter.write_str(
                "it can hold macros (a macro-enabled template or a VBA project), which are refused",
            ),
            Self::NotWord(why) => write!(formatter, "it is not a Word template: {why}"),
            Self::Malformed { part, why } => write!(formatter, "{part} is not well formed: {why}"),
        }
    }
}

impl std::error::Error for TemplateError {}

/// The template's page: the last section's size and margins, in twentieths of a point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplatePage {
    /// In portrait.
    pub width: u32,
    pub height: u32,
    pub landscape: bool,
    pub margins: Margins,
}

/// A picture a kept header or footer shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TemplateMedia {
    /// Its name in the export, `word/media/template<n>.<extension>`.
    pub(crate) name: String,
    pub(crate) content_type: &'static str,
    pub(crate) data: Vec<u8>,
}

/// A kept header's or footer's relationship: a picture (an index into the media) or a link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TemplateTarget {
    Picture(usize),
    Link(String),
}

/// A kept header or footer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TemplateRunning {
    pub(crate) header: bool,
    /// `default`, `even` or `first`.
    pub(crate) kind: &'static str,
    pub(crate) data: Vec<u8>,
    /// Its relationships by id, kept with the ids the part names them by.
    pub(crate) relationships: Vec<(String, TemplateTarget)>,
}

/// A template's font table and the fonts it embeds, by the relationship ids it names them by.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TemplateFonts {
    pub(crate) table: Vec<u8>,
    pub(crate) fonts: Vec<(String, Vec<u8>)>,
    pub(crate) embeds: bool,
}

/// What an export uses of a template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Template {
    pub(crate) styles: String,
    pub(crate) numbering: Option<String>,
    /// The numbering's picture bullets.
    pub(crate) numbering_relationships: Vec<(String, TemplateTarget)>,
    pub(crate) theme: Option<Vec<u8>>,
    pub(crate) fonts: Option<TemplateFonts>,
    page: Option<TemplatePage>,
    pub(crate) running: Vec<TemplateRunning>,
    pub(crate) media: Vec<TemplateMedia>,
    /// The last section's first page has its own header and footer.
    pub(crate) title_page: bool,
    /// Odd and even pages have different headers and footers (its settings say so).
    pub(crate) odd_even: bool,
    /// What was left out, and why.
    pub warnings: Vec<String>,
}

fn xml<'a>(part: &str, text: &'a str) -> Result<Document<'a>, TemplateError> {
    Document::parse(text).map_err(|error| TemplateError::Malformed {
        part: part.to_owned(),
        why: error.to_string(),
    })
}

fn is(node: Node<'_, '_>, namespace: &str, name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace() == Some(namespace)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| is(*child, W, name))
}

fn w_attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attribute((W, name))
}

/// A twip value: a number of twentieths of a point, never negative.
fn twips(node: Option<Node<'_, '_>>, name: &str) -> Option<u32> {
    let value = w_attribute(node?, name)?;
    value
        .parse::<i64>()
        .ok()
        .map(|value| u32::try_from(value.clamp(0, 31_680 * 4)).unwrap_or(0))
}

/// `target` of a relationship from `source`, as a part name: relative to the source's folder,
/// or from the package's root when it starts with `/`. `None` when it leaves the package.
fn resolve(source: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        let folder = source.rsplit_once('/').map_or("", |(folder, _)| folder);
        folder.split('/').filter(|part| !part.is_empty()).collect()
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            segment => parts.push(segment),
        }
    }
    Some(parts.join("/"))
}

/// The relationships part of `part`.
fn relationships_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// A relationship: its type, target and whether it leaves the package.
struct Relationship {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

/// The package as read: its entries (looked up ignoring case, as OPC names compare) and
/// content types.
struct Package {
    entries: BTreeMap<String, (String, Vec<u8>)>,
    overrides: BTreeMap<String, String>,
    defaults: BTreeMap<String, String>,
}

impl Package {
    fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .get(&name.to_ascii_lowercase())
            .map(|(_, data)| data.as_slice())
    }

    fn text(&self, name: &str) -> Result<Option<&str>, TemplateError> {
        self.get(name)
            .map(|data| {
                std::str::from_utf8(data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data)).map_err(
                    |_| TemplateError::Malformed {
                        part: name.to_owned(),
                        why: "it is not UTF-8".to_owned(),
                    },
                )
            })
            .transpose()
    }

    fn content_type(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.overrides
            .get(&lower)
            .or_else(|| {
                let extension = lower.rsplit_once('.')?.1;
                self.defaults.get(extension)
            })
            .map(String::as_str)
    }

    /// The relationships of `part` (`""` for the package's).
    fn relationships(&self, part: &str) -> Result<Vec<Relationship>, TemplateError> {
        let name = if part.is_empty() {
            "_rels/.rels".to_owned()
        } else {
            relationships_name(part)
        };
        let Some(text) = self.text(&name)? else {
            return Ok(Vec::new());
        };
        let document = xml(&name, text)?;
        Ok(document
            .root_element()
            .children()
            .filter(|node| is(*node, RELATIONSHIPS_NS, "Relationship"))
            .filter_map(|node| {
                Some(Relationship {
                    id: node.attribute("Id")?.to_owned(),
                    kind: node.attribute("Type")?.to_owned(),
                    target: node.attribute("Target")?.to_owned(),
                    external: node
                        .attribute("TargetMode")
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("External")),
                })
            })
            .collect())
    }
}

/// `kind` is the relationship type `name` (`styles`, `header`, …) of the transitional schema.
fn of_kind(relationship: &Relationship, name: &str) -> bool {
    relationship.kind.strip_prefix(REL) == Some(name)
}

/// The fields a part holds that are not in [`FIELDS`]: simple fields' instructions, and
/// complex fields' put together from their runs.
fn foreign_fields(document: &Document<'_>) -> Vec<String> {
    let mut found = Vec::new();
    let mut check = |instruction: &str| {
        let name = instruction
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if !FIELDS.contains(&name.as_str()) {
            found.push(if name.is_empty() {
                "(empty)".to_owned()
            } else {
                name
            });
        }
    };
    // Each open complex field's instruction so far, and whether it was checked.
    let mut open: Vec<(String, bool)> = Vec::new();
    for node in document.descendants() {
        if is(node, W, "fldSimple") {
            check(w_attribute(node, "instr").unwrap_or_default());
        } else if is(node, W, "instrText") {
            if let Some((instruction, _)) = open.last_mut() {
                instruction.push_str(node.text().unwrap_or_default());
            } else {
                check(node.text().unwrap_or_default());
            }
        } else if is(node, W, "fldChar") {
            match w_attribute(node, "fldCharType") {
                Some("begin") => open.push((String::new(), false)),
                Some("separate") => {
                    if let Some((instruction, checked)) = open.last_mut() {
                        if !*checked {
                            check(instruction);
                            *checked = true;
                        }
                    }
                }
                Some("end") => {
                    if let Some((instruction, checked)) = open.pop() {
                        if !checked {
                            check(&instruction);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for (instruction, checked) in open {
        if !checked {
            check(&instruction);
        }
    }
    found
}

impl Template {
    /// Reads a template from its bytes.
    pub fn read(bytes: &[u8]) -> Result<Self, TemplateError> {
        if bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(TemplateError::TooLarge(bytes.len()));
        }
        let entries = unzip::entries(bytes).map_err(TemplateError::Unreadable)?;
        let lower = |name: &str| name.to_ascii_lowercase();
        if entries.keys().any(|name| {
            let name = lower(name);
            name.contains("vbaproject") || name.contains("vbadata")
        }) {
            return Err(TemplateError::Macros);
        }
        let mut package = Package {
            entries: entries
                .into_iter()
                .map(|(name, data)| (lower(&name), (name, data)))
                .collect(),
            overrides: BTreeMap::new(),
            defaults: BTreeMap::new(),
        };
        let types = package
            .text("[Content_Types].xml")?
            .ok_or_else(|| TemplateError::NotWord("it has no [Content_Types].xml".to_owned()))?;
        let types = xml("[Content_Types].xml", types)?;
        let mut overrides = BTreeMap::new();
        let mut defaults = BTreeMap::new();
        for node in types.root_element().children() {
            let Some(content_type) = node.attribute("ContentType") else {
                continue;
            };
            if content_type.to_ascii_lowercase().contains("macroenabled")
                || content_type.to_ascii_lowercase().contains("vbaproject")
            {
                return Err(TemplateError::Macros);
            }
            if is(node, CONTENT_TYPES_NS, "Override") {
                if let Some(name) = node.attribute("PartName") {
                    overrides.insert(lower(name.trim_start_matches('/')), content_type.to_owned());
                }
            } else if is(node, CONTENT_TYPES_NS, "Default") {
                if let Some(extension) = node.attribute("Extension") {
                    defaults.insert(lower(extension), content_type.to_owned());
                }
            }
        }
        package.overrides = overrides;
        package.defaults = defaults;

        let main = package
            .relationships("")?
            .into_iter()
            .find(|relationship| of_kind(relationship, "officeDocument") && !relationship.external)
            .and_then(|relationship| resolve("", &relationship.target))
            .ok_or_else(|| TemplateError::NotWord("it names no main document".to_owned()))?;
        if !package
            .content_type(&main)
            .is_some_and(|kind| MAIN_TYPES.contains(&kind))
        {
            return Err(TemplateError::NotWord(format!(
                "{main} is not a WordprocessingML document"
            )));
        }
        let relationships = package.relationships(&main)?;
        let part_of = |name: &str| {
            relationships
                .iter()
                .find(|relationship| of_kind(relationship, name) && !relationship.external)
                .and_then(|relationship| resolve(&main, &relationship.target))
        };
        let mut warnings = Vec::new();

        let styles_name = part_of("styles")
            .ok_or_else(|| TemplateError::NotWord("it has no styles".to_owned()))?;
        let styles = package
            .text(&styles_name)?
            .ok_or_else(|| TemplateError::NotWord(format!("{styles_name} is missing")))?
            .to_owned();
        check_root(&styles_name, &styles, "styles")?;

        let numbering_name = part_of("numbering");

        let theme = match part_of("theme") {
            Some(name) => match package.text(&name)? {
                Some(text) => {
                    xml(&name, text)?;
                    if package.relationships(&name)?.is_empty() {
                        Some(text.as_bytes().to_vec())
                    } else {
                        warnings.push(format!(
                            "the template's theme ({name}) refers to other parts; it is left out"
                        ));
                        None
                    }
                }
                None => None,
            },
            None => None,
        };

        let fonts = match part_of("fontTable") {
            Some(name) => Self::fonts(&package, &name, &mut warnings)?,
            None => None,
        };

        let odd_even = match part_of("settings") {
            Some(name) => match package.text(&name)? {
                Some(text) => {
                    let settings = xml(&name, text)?;
                    settings
                        .root_element()
                        .children()
                        .find(|node| is(*node, W, "evenAndOddHeaders"))
                        .is_some_and(|node| {
                            !matches!(w_attribute(node, "val"), Some("0" | "false" | "off"))
                        })
                }
                None => false,
            },
            None => false,
        };

        let document = package
            .text(&main)?
            .ok_or_else(|| TemplateError::NotWord(format!("{main} is missing")))?;
        let document = xml(&main, document)?;
        let body = child(document.root_element(), "body")
            .ok_or_else(|| TemplateError::NotWord(format!("{main} has no body")))?;
        let section = body
            .children()
            .rev()
            .find(|node| node.is_element())
            .filter(|node| is(*node, W, "sectPr"));
        let mut template = Self {
            styles,
            numbering: None,
            numbering_relationships: Vec::new(),
            theme,
            fonts,
            page: None,
            running: Vec::new(),
            media: Vec::new(),
            title_page: false,
            odd_even,
            warnings: Vec::new(),
        };
        if let Some(name) = numbering_name {
            if let Some(text) = package.text(&name)? {
                check_root(&name, text, "numbering")?;
                let document = xml(&name, text)?;
                match template.kept_relationships(&package, &name, &document, false)? {
                    Ok(relationships) => {
                        template.numbering = Some(text.to_owned());
                        template.numbering_relationships = relationships;
                    }
                    Err(why) => warnings.push(format!(
                        "the template's list numbering ({name}) is left out: {why}"
                    )),
                }
            }
        }
        if let Some(section) = section {
            template.page = page(section);
            template.title_page = child(section, "titlePg").is_some_and(|node| {
                !matches!(w_attribute(node, "val"), Some("0" | "false" | "off"))
            });
            // A section without a header or footer of a type shows the one before it, so each is
            // looked for from the last section back.
            let sections: Vec<Node<'_, '_>> = body
                .descendants()
                .filter(|node| is(*node, W, "sectPr"))
                .collect();
            let mut seen = std::collections::BTreeSet::new();
            for reference in sections.iter().rev().flat_map(|section| section.children()) {
                let header = is(reference, W, "headerReference");
                if !header && !is(reference, W, "footerReference") {
                    continue;
                }
                let kind = match w_attribute(reference, "type") {
                    Some("even") => "even",
                    Some("first") => "first",
                    _ => "default",
                };
                if !seen.insert((header, kind)) {
                    continue;
                }
                let Some(id) = reference.attribute((R, "id")) else {
                    continue;
                };
                let Some(name) = relationships
                    .iter()
                    .find(|relationship| relationship.id == id && !relationship.external)
                    .and_then(|relationship| resolve(&main, &relationship.target))
                else {
                    continue;
                };
                template.running_part(&package, &name, header, kind, &mut warnings)?;
            }
        }
        template.warnings = warnings;
        Ok(template)
    }

    /// The font table at `name` and the fonts it embeds, or `None`, with a warning, when it
    /// relates to anything but fonts in the package.
    fn fonts(
        package: &Package,
        name: &str,
        warnings: &mut Vec<String>,
    ) -> Result<Option<TemplateFonts>, TemplateError> {
        let Some(text) = package.text(name)? else {
            return Ok(None);
        };
        let table = xml(name, text)?;
        let embeds = table.descendants().any(|node| {
            node.is_element()
                && node.tag_name().namespace() == Some(W)
                && node.tag_name().name().starts_with("embed")
        });
        let mut fonts = Vec::new();
        for relationship in package.relationships(name)? {
            let data = (!relationship.external && of_kind(&relationship, "font"))
                .then(|| resolve(name, &relationship.target))
                .flatten()
                .and_then(|target| package.get(&target));
            match data {
                Some(data) => fonts.push((relationship.id, data.to_vec())),
                None => {
                    warnings.push(format!(
                        "the template's font table ({name}) refers to something other than a \
                         font in it; its fonts are left out"
                    ));
                    return Ok(None);
                }
            }
        }
        Ok(Some(TemplateFonts {
            table: text.as_bytes().to_vec(),
            fonts,
            embeds,
        }))
    }

    /// Keeps the header or footer at `name`, unless it relates to anything but a picture in
    /// the package or a link, or holds a field outside [`FIELDS`].
    fn running_part(
        &mut self,
        package: &Package,
        name: &str,
        header: bool,
        kind: &'static str,
        warnings: &mut Vec<String>,
    ) -> Result<(), TemplateError> {
        let which = if header { "header" } else { "footer" };
        let Some(text) = package.text(name)? else {
            return Ok(());
        };
        let document = xml(name, text)?;
        let fields = foreign_fields(&document);
        if !fields.is_empty() {
            warnings.push(format!(
                "the template's {kind} {which} ({name}) is left out: it holds the field{} {}, \
                 which could read something outside the document",
                if fields.len() == 1 { "" } else { "s" },
                fields.join(", ")
            ));
            return Ok(());
        }
        let relationships = match self.kept_relationships(package, name, &document, true)? {
            Ok(relationships) => relationships,
            Err(why) => {
                warnings.push(format!(
                    "the template's {kind} {which} ({name}) is left out: {why}"
                ));
                return Ok(());
            }
        };
        self.running.push(TemplateRunning {
            header,
            kind,
            data: text.as_bytes().to_vec(),
            relationships,
        });
        Ok(())
    }

    /// The relationships of the part `name` (parsed as `document`) an export can keep:
    /// pictures in the package of an allowed type that load nothing themselves, copied once
    /// each, and, where `links` allows, external links. `Err` says why the part cannot be kept:
    /// it relates to anything else, uses a link's id other than as a link, or names an address
    /// outside the package in an attribute.
    fn kept_relationships(
        &mut self,
        package: &Package,
        name: &str,
        document: &Document<'_>,
        links: bool,
    ) -> Result<Result<Vec<(String, TemplateTarget)>, String>, TemplateError> {
        let mut kept = Vec::new();
        for relationship in package.relationships(name)? {
            if links && of_kind(&relationship, "hyperlink") && relationship.external {
                kept.push((relationship.id, TemplateTarget::Link(relationship.target)));
                continue;
            }
            let picture = (of_kind(&relationship, "image") && !relationship.external)
                .then(|| resolve(name, &relationship.target))
                .flatten()
                .and_then(|target| {
                    let (content_type, extension) = PICTURE_TYPES
                        .into_iter()
                        .find(|(known, _)| package.content_type(&target) == Some(*known))?;
                    let data = package.get(&target)?;
                    Some((content_type, extension, data))
                });
            let Some((content_type, extension, data)) = picture else {
                return Ok(Err(format!(
                    "it refers to {}, which an export does not copy",
                    if relationship.external {
                        format!("{} outside the template", relationship.target)
                    } else {
                        relationship.target
                    }
                )));
            };
            if content_type == "image/svg+xml" && !closed_svg(data) {
                return Ok(Err(format!(
                    "its picture {} could load something from outside it",
                    relationship.target
                )));
            }
            let index = match self.media.iter().position(|known| known.data == data) {
                Some(index) => index,
                None => {
                    self.media.push(TemplateMedia {
                        name: format!("word/media/template{}.{extension}", self.media.len() + 1),
                        content_type,
                        data: data.to_vec(),
                    });
                    self.media.len() - 1
                }
            };
            kept.push((relationship.id, TemplateTarget::Picture(index)));
        }
        // A link's id is used only as a link: anything else naming it (a picture's `r:link`, a
        // VML `r:href`) would fetch it.
        for node in document.descendants().filter(Node::is_element) {
            let linking = is(node, W, "hyperlink")
                || (node.tag_name().namespace() == Some(DRAWINGML)
                    && matches!(node.tag_name().name(), "hlinkClick" | "hlinkHover"));
            for attribute in node.attributes() {
                let value = attribute.value();
                if attribute.namespace() == Some(R)
                    && !linking
                    && kept.iter().any(|(id, target)| {
                        id == value && matches!(target, TemplateTarget::Link(_))
                    })
                {
                    return Ok(Err(format!(
                        "it uses the link {value} as something to fetch"
                    )));
                }
                if names_an_address(attribute.name(), value) {
                    return Ok(Err(format!("it names {value} directly")));
                }
            }
        }
        Ok(Ok(kept))
    }

    /// The last section's page, when it gives one.
    #[must_use]
    pub fn page(&self) -> Option<TemplatePage> {
        self.page
    }

    /// Whether it gives headers (`header`) or footers.
    #[must_use]
    pub fn has_running(&self, header: bool) -> bool {
        self.running.iter().any(|part| part.header == header)
    }

    /// Leaves out its headers (`header`) or footers, for settings that write their own.
    pub fn without_running(&mut self, header: bool) {
        // Pictures only a removed part showed are not copied: the writer copies those used.
        self.running.retain(|part| part.header != header);
    }
}

/// Checks that `text` is well-formed XML whose root is `w:<root>`, with the WordprocessingML
/// namespace bound to `w`, as the export's own parts merged into it name it.
fn check_root(part: &str, text: &str, root: &str) -> Result<(), TemplateError> {
    let document = xml(part, text)?;
    let element = document.root_element();
    if !is(element, W, root) {
        return Err(TemplateError::NotWord(format!("{part} is not w:{root}")));
    }
    if element.lookup_prefix(W) != Some("w") {
        return Err(TemplateError::NotWord(format!(
            "{part} names the WordprocessingML namespace other than `w`"
        )));
    }
    Ok(())
}

/// The page a section sets: its size, orientation and margins.
fn page(section: Node<'_, '_>) -> Option<TemplatePage> {
    let size = child(section, "pgSz")?;
    let (width, height) = (twips(Some(size), "w")?, twips(Some(size), "h")?);
    let landscape = w_attribute(size, "orient") == Some("landscape");
    let (width, height) = if landscape {
        (height, width)
    } else {
        (width, height)
    };
    let margin = child(section, "pgMar");
    let defaults = Margins::default();
    Some(TemplatePage {
        width,
        height,
        landscape,
        margins: Margins {
            top: twips(margin, "top").unwrap_or(defaults.top),
            right: twips(margin, "right").unwrap_or(defaults.right),
            bottom: twips(margin, "bottom").unwrap_or(defaults.bottom),
            left: twips(margin, "left").unwrap_or(defaults.left),
            header: twips(margin, "header").unwrap_or(defaults.header),
            footer: twips(margin, "footer").unwrap_or(defaults.footer),
        },
    })
}
