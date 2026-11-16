//! The document's pages: sections with their size, margins and running content, covers, the
//! title page, header and footer parts, and the table of contents.
//!
//! A book is a front cover, a title page, the body (the table of contents first, the running
//! content and page numbers from 1) and a back cover, each a section of its own. Covers and the
//! title page show no running content: the first sections name no header or footer, and the back
//! cover, which would otherwise take the body's, names empty ones.

use adoc_render::ViewImage;

use crate::{
    body::{Format, Para, Part, Writer},
    parts::NAMESPACES,
    xml::Xml,
    Line, Margins, Slots,
};

/// English Metric Units in a twentieth of a point.
const EMU_PER_TWIP: i64 = 635;
/// The height a logo in a header or footer is drawn at, in pixels.
const LOGO_HEIGHT: f64 = 32.0;

pub(crate) const HEADER_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
pub(crate) const FOOTER_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
pub(crate) const HEADER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
pub(crate) const FOOTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";

/// A header or footer part: its name in the package, content type, XML and the part it is to
/// the writer, whose hyperlinks and pictures are its relationships.
pub(crate) struct RunningPart {
    pub(crate) name: String,
    pub(crate) content_type: &'static str,
    pub(crate) data: Vec<u8>,
    pub(crate) part: Part,
}

/// A section's header or footer reference: the element, its type and its relationship id.
pub(crate) type Reference = (&'static str, &'static str, String);

/// A section's properties.
pub(crate) struct Section {
    pub(crate) margins: Margins,
    pub(crate) references: Vec<Reference>,
    /// The first page has headers and footers of its own.
    pub(crate) title_page: bool,
    /// Page numbers start again at 1.
    pub(crate) restart: bool,
}

impl Section {
    /// A section filled to its edges: a cover.
    pub(crate) fn cover(references: Vec<Reference>) -> Self {
        Self {
            margins: Margins {
                top: 0,
                right: 0,
                bottom: 0,
                left: 0,
                header: 0,
                footer: 0,
            },
            references,
            title_page: false,
            restart: false,
        }
    }

    pub(crate) fn plain(margins: Margins) -> Self {
        Self {
            margins,
            references: Vec::new(),
            title_page: false,
            restart: false,
        }
    }
}

/// `Line`'s left and right swapped: an even page's, mirroring an odd page's.
fn mirrored(line: &Line) -> Line {
    Line {
        left: line.right.clone(),
        centre: line.centre.clone(),
        right: line.left.clone(),
    }
}

impl Writer<'_> {
    /// A section's properties, in the schema's order.
    pub(crate) fn section_properties(&mut self, section: &Section) {
        let (width, height) = self.options.page_size();
        self.xml.open("w:sectPr", &[]);
        for (element, kind, id) in &section.references {
            self.xml.empty(element, &[("w:type", kind), ("r:id", id)]);
        }
        let mut size = vec![("w:w", width.to_string()), ("w:h", height.to_string())];
        if self.options.landscape {
            size.push(("w:orient", "landscape".to_owned()));
        }
        let size: Vec<(&str, &str)> = size
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        self.xml.empty("w:pgSz", &size);
        let margins = section.margins;
        let values = [
            ("w:top", margins.top),
            ("w:right", margins.right),
            ("w:bottom", margins.bottom),
            ("w:left", margins.left),
            ("w:header", margins.header),
            ("w:footer", margins.footer),
            ("w:gutter", 0),
        ]
        .map(|(name, value)| (name, value.to_string()));
        let values: Vec<(&str, &str)> = values
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        self.xml.empty("w:pgMar", &values);
        if section.restart {
            self.xml.empty("w:pgNumType", &[("w:start", "1")]);
        }
        if section.title_page {
            self.xml.empty("w:titlePg", &[]);
        }
        self.xml.close("w:sectPr");
    }

    /// Ends a section: its properties in a paragraph of their own.
    pub(crate) fn end_section(&mut self, section: &Section) {
        self.close_tables();
        self.xml.open("w:p", &[]);
        self.xml.open("w:pPr", &[]);
        self.section_properties(section);
        self.xml.close("w:pPr");
        self.xml.close("w:p");
    }

    /// A cover: `image` as large as the page holds it, centred. `false` when it cannot be shown,
    /// which is reported.
    pub(crate) fn cover(&mut self, image: &ViewImage, alt: &str) -> bool {
        let Some(path) = &image.path else {
            self.warn(
                None,
                format!(
                    "the cover image {} cannot be found and is left out",
                    image.target
                ),
            );
            return false;
        };
        let picture = match self.media.load(path, &self.options.access) {
            Ok(picture) => picture,
            Err(reason) => {
                self.warn(
                    None,
                    format!("the cover image {} {reason}; it is left out", image.target),
                );
                return false;
            }
        };
        let (width, height) = self.options.page_size();
        let (page_width, page_height) = (
            f64::from(width) * EMU_PER_TWIP as f64,
            f64::from(height) * EMU_PER_TWIP as f64,
        );
        // EMUs per pixel that make the picture as large as the page holds it, a little less so
        // the paragraph holding it fits on the page.
        let scale =
            (page_width / picture.width.max(1.0)).min(page_height / picture.height.max(1.0)) * 0.98;
        let size = (
            (picture.width * scale).floor() as i64,
            (picture.height * scale).floor() as i64,
        );
        self.paragraph(
            Para {
                style: Some("CoverImage"),
                ..Para::default()
            },
            |writer| writer.drawing(&picture, size, alt, None, None),
        );
        true
    }

    /// The title page: the logo, the title and subtitle, the authors, the version and the date,
    /// centred.
    pub(crate) fn title_page(
        &mut self,
        logo: Option<&ViewImage>,
        title: &[adoc_render::ViewInline],
        subtitle: Option<&str>,
        attributes: &std::collections::BTreeMap<String, String>,
    ) {
        let align = self.align.replace("center");
        if let Some(logo) = logo {
            let logo = ViewImage {
                alt: "Logo".to_owned(),
                ..logo.clone()
            };
            self.paragraph(Para::default(), |writer| {
                writer.image(&logo, Format::default(), None);
            });
        }
        self.paragraph(
            Para {
                style: Some("Title"),
                ..Para::default()
            },
            |writer| writer.inlines(title, Format::default()),
        );
        if let Some(subtitle) = subtitle.filter(|subtitle| !subtitle.trim().is_empty()) {
            self.paragraph(
                Para {
                    style: Some("Subtitle"),
                    ..Para::default()
                },
                |writer| writer.text(subtitle, Format::default()),
            );
        }
        let get = |name: &str| {
            attributes
                .get(name)
                .filter(|value| !value.trim().is_empty())
        };
        let details = [
            get("author").or_else(|| get("authors")).cloned(),
            get("revnumber").map(|version| format!("Version {version}")),
            get("revdate").cloned(),
        ];
        for detail in details.into_iter().flatten() {
            self.paragraph(
                Para {
                    style: Some("TitlePageDetail"),
                    ..Para::default()
                },
                |writer| writer.text(&detail, Format::default()),
            );
        }
        self.align = align;
    }

    /// A new part with relationships of its own.
    fn new_part(&mut self) -> Part {
        self.hyperlinks.push(Vec::new());
        self.pictures.push(Vec::new());
        Part(self.hyperlinks.len() - 1)
    }

    /// A header (`header`) or footer part holding `line`, and its relationship id from the
    /// document. `title` is the document's title, `chapter` the style a chapter's heading has.
    fn running_part(&mut self, header: bool, line: &Line, title: &str, chapter: &str) -> String {
        let part = self.new_part();
        let body = std::mem::take(&mut self.xml);
        let previous = std::mem::replace(&mut self.part, part);
        let style = if header { "Header" } else { "Footer" };
        self.paragraph(
            Para {
                style: Some(style),
                ..Para::default()
            },
            |writer| {
                if line.is_empty() {
                    return;
                }
                for (index, slot) in [&line.left, &line.centre, &line.right]
                    .into_iter()
                    .enumerate()
                {
                    if index > 0 {
                        writer.text("\t", Format::default());
                    }
                    writer.slot(slot, title, chapter);
                }
            },
        );
        let content = std::mem::replace(&mut self.xml, body);
        self.part = previous;
        let (root, kind, content_type) = if header {
            ("w:hdr", "header", HEADER_TYPE)
        } else {
            ("w:ftr", "footer", FOOTER_TYPE)
        };
        let number = self
            .running_parts
            .iter()
            .filter(|known| known.content_type == content_type)
            .count()
            + 1;
        let mut xml = Xml::part();
        xml.open(root, &NAMESPACES);
        xml.append(content);
        xml.close(root);
        self.running_parts.push(RunningPart {
            name: format!("word/{kind}{number}.xml"),
            content_type,
            data: xml.into_bytes(),
            part,
        });
        let id = format!("r{}{number}", if header { "Header" } else { "Footer" });
        self.document_links.push((
            id.clone(),
            if header { HEADER } else { FOOTER },
            format!("{kind}{number}.xml"),
        ));
        id
    }

    /// One slot of a header or footer line: its text, with its placeholders as fields, the
    /// title or the logo.
    fn slot(&mut self, slot: &str, title: &str, chapter: &str) {
        let mut rest = slot;
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}').map(|end| start + end) else {
                break;
            };
            self.text(&rest[..start], Format::default());
            match &rest[start + 1..end] {
                "page-number" => self.field(" PAGE ", "1", Format::default()),
                "page-count" => self.field(" NUMPAGES ", "1", Format::default()),
                "document-title" => self.text(title, Format::default()),
                "chapter-title" => {
                    self.field(&format!(" STYLEREF \"{chapter}\" "), "", Format::default());
                }
                "logo" => self.logo(),
                other => self.text(&format!("{{{other}}}"), Format::default()),
            }
            rest = &rest[end + 1..];
        }
        self.text(rest, Format::default());
    }

    /// The running content's logo, a line high.
    fn logo(&mut self) {
        let Some(path) = self.options.running.logo.clone() else {
            self.warn(
                None,
                "a header or footer shows {logo}, and no logo is set".to_owned(),
            );
            return;
        };
        match self.media.load(&path, &self.options.access) {
            Ok(picture) => {
                let height = LOGO_HEIGHT;
                let width = picture.width * height / picture.height.max(1.0);
                let size = (
                    (width * 9525.0).round() as i64,
                    (height * 9525.0).round() as i64,
                );
                self.drawing(&picture, size, "Logo", None, None);
            }
            Err(reason) => self.warn(
                None,
                format!("the logo {} {reason}; it is left out", path.display()),
            ),
        }
    }

    /// The body's header and footer references: each slot's part.
    pub(crate) fn running_references(
        &mut self,
        title: &str,
        chapter: &str,
    ) -> (Vec<Reference>, bool) {
        let running = self.options.running.clone();
        let mut references = Vec::new();
        let mut first_page = false;
        for (header, slots) in [(true, &running.header), (false, &running.footer)] {
            if slots_empty(slots) {
                continue;
            }
            let element = if header {
                "w:headerReference"
            } else {
                "w:footerReference"
            };
            let id = self.running_part(header, &slots.odd, title, chapter);
            references.push((element, "default", id));
            if running.odd_even {
                let even = slots.even.clone().unwrap_or_else(|| mirrored(&slots.odd));
                let id = self.running_part(header, &even, title, chapter);
                references.push((element, "even", id));
            }
        }
        // A first page of its own: the slots' first lines, or nothing.
        if running.header.first.is_some() || running.footer.first.is_some() {
            first_page = true;
            for (header, slots) in [(true, &running.header), (false, &running.footer)] {
                let element = if header {
                    "w:headerReference"
                } else {
                    "w:footerReference"
                };
                let line = slots.first.clone().unwrap_or_default();
                let id = self.running_part(header, &line, title, chapter);
                references.push((element, "first", id));
            }
        }
        (references, first_page)
    }

    /// Empty headers and footers for every type of page: a back cover's, which would otherwise
    /// take the body's.
    pub(crate) fn blank_references(&mut self) -> Vec<Reference> {
        let header = self.running_part(true, &Line::default(), "", "");
        let footer = self.running_part(false, &Line::default(), "", "");
        ["default", "even", "first"]
            .into_iter()
            .flat_map(|kind| {
                [
                    ("w:headerReference", kind, header.clone()),
                    ("w:footerReference", kind, footer.clone()),
                ]
            })
            .collect()
    }

    /// The table of contents, of the headings written at or above `levels`, for inserting
    /// where it was asked for: a `TOC` field Word updates on opening, with its entries written
    /// already, each a link to its heading, so it reads correctly before any update. The page
    /// numbers are `PAGEREF` fields, blank until Word updates them.
    pub(crate) fn table_of_contents(&mut self, levels: u8, title: &str) -> Xml {
        let headings: Vec<(u8, String, String)> = self
            .headings
            .iter()
            .filter(|(level, _, _)| *level <= levels)
            .cloned()
            .collect();
        let body = std::mem::take(&mut self.xml);
        let (align, indent, quote) = (self.align.take(), self.indent, self.quote);
        self.indent = 0;
        self.quote = false;
        self.paragraph(
            Para {
                style: Some("TOCHeading"),
                ..Para::default()
            },
            |writer| writer.text(title, Format::default()),
        );
        let instruction = format!(" TOC \\o \"1-{levels}\" \\h \\z \\u ");
        let open = |writer: &mut Self| {
            writer.xml.open("w:r", &[]);
            writer.xml.empty(
                "w:fldChar",
                &[("w:fldCharType", "begin"), ("w:dirty", "true")],
            );
            writer.xml.close("w:r");
            writer.xml.open("w:r", &[]);
            writer
                .xml
                .element("w:instrText", &[("xml:space", "preserve")], &instruction);
            writer.xml.close("w:r");
            writer.xml.open("w:r", &[]);
            writer
                .xml
                .empty("w:fldChar", &[("w:fldCharType", "separate")]);
            writer.xml.close("w:r");
        };
        let close = |writer: &mut Self| {
            writer.xml.open("w:r", &[]);
            writer.xml.empty("w:fldChar", &[("w:fldCharType", "end")]);
            writer.xml.close("w:r");
        };
        if headings.is_empty() {
            self.paragraph(Para::default(), |writer| {
                open(writer);
                writer.text("Update this field to list the headings.", Format::default());
                close(writer);
            });
        }
        for (index, (level, text, name)) in headings.iter().enumerate() {
            let style = format!("TOC{level}");
            self.paragraph(
                Para {
                    style: Some(&style),
                    ..Para::default()
                },
                |writer| {
                    if index == 0 {
                        open(writer);
                    }
                    writer
                        .xml
                        .open("w:hyperlink", &[("w:anchor", name), ("w:history", "1")]);
                    writer.text(text, Format::default());
                    writer.text("\t", Format::default());
                    writer.field(&format!(" PAGEREF {name} \\h "), "", Format::default());
                    writer.xml.close("w:hyperlink");
                },
            );
        }
        if !headings.is_empty() {
            self.paragraph(Para::default(), close);
        }
        self.align = align;
        self.indent = indent;
        self.quote = quote;
        self.wrote_toc = true;
        std::mem::replace(&mut self.xml, body)
    }
}

fn slots_empty(slots: &Slots) -> bool {
    slots.odd.is_empty()
        && slots.even.as_ref().is_none_or(Line::is_empty)
        && slots.first.as_ref().is_none_or(Line::is_empty)
}
