//! Inline content as Word runs: formatting as run properties and character styles, links as
//! hyperlinks (internal ones to bookmarks, external ones through relationships), anchors as
//! bookmarks and footnotes as Word footnotes.

use adoc_render::{ResolvedLink, SourceSpan, ViewInline};

use crate::{
    body::{Format, Hyperlink, Part, Writer},
    xml::Xml,
};

/// Where a cross reference leads in a Word document.
enum Destination {
    /// A bookmark in the document.
    Bookmark(String),
    /// A page of the site, outside the document.
    Url(String),
    /// Nowhere: the reference is written as text, for this reason.
    Nowhere(String),
}

/// Whether `url` may be a hyperlink: a web, mail, file transfer or telephone address, or a
/// relative one. Anything else (`javascript:`, `data:`, `file:` and the rest) stays text.
pub(crate) fn safe_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.len() > 2000 || url.chars().any(char::is_control) {
        return false;
    }
    let scheme_end = url.find(':');
    let path_start = url.find(['/', '?', '#']);
    match (scheme_end, path_start) {
        (Some(colon), path) if path.is_none_or(|path| colon < path) => matches!(
            url[..colon].to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto" | "ftp" | "ftps" | "tel"
        ),
        _ => true,
    }
}

impl Writer<'_> {
    /// `text` as runs in `format`: its tabs as Word tabs and its line endings as breaks.
    pub(crate) fn text(&mut self, text: &str, format: Format) {
        if text.is_empty() {
            return;
        }
        self.xml.open("w:r", &[]);
        self.run_properties(format);
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        for (line_number, line) in text.split('\n').enumerate() {
            if line_number > 0 {
                self.xml.empty("w:br", &[]);
            }
            for (piece_number, piece) in line.split('\t').enumerate() {
                if piece_number > 0 {
                    self.xml.empty("w:tab", &[]);
                }
                if !piece.is_empty() {
                    self.xml.element("w:t", &[("xml:space", "preserve")], piece);
                }
            }
        }
        self.xml.close("w:r");
    }

    /// A run's properties, in the order the schema gives them.
    fn run_properties(&mut self, format: Format) {
        let any = format.style.is_some()
            || format.bold
            || format.italic
            || format.strike
            || format.mark
            || format.underline
            || format.superscript
            || format.subscript;
        if !any {
            return;
        }
        self.xml.open("w:rPr", &[]);
        if let Some(style) = format.style {
            let style = self.style_id(style);
            self.xml.empty("w:rStyle", &[("w:val", &style)]);
        }
        if format.bold {
            self.xml.empty("w:b", &[]);
            self.xml.empty("w:bCs", &[]);
        }
        if format.italic {
            self.xml.empty("w:i", &[]);
            self.xml.empty("w:iCs", &[]);
        }
        if format.strike {
            self.xml.empty("w:strike", &[]);
        }
        if format.mark {
            self.xml.empty("w:highlight", &[("w:val", "yellow")]);
        }
        if format.underline {
            self.xml.empty("w:u", &[("w:val", "single")]);
        }
        if format.superscript || format.subscript {
            let align = if format.superscript {
                "superscript"
            } else {
                "subscript"
            };
            self.xml.empty("w:vertAlign", &[("w:val", align)]);
        }
        self.xml.close("w:rPr");
    }

    pub(crate) fn inlines(&mut self, inlines: &[ViewInline], format: Format) {
        for inline in inlines {
            self.inline(inline, format);
        }
    }

    fn inline(&mut self, inline: &ViewInline, format: Format) {
        match inline {
            ViewInline::Text(text) | ViewInline::Raw(text) => self.text(text, format),
            ViewInline::Strong(content) => self.inlines(
                content,
                Format {
                    bold: true,
                    ..format
                },
            ),
            ViewInline::Emphasis(content) => self.inlines(
                content,
                Format {
                    italic: true,
                    ..format
                },
            ),
            ViewInline::Code(content) => self.inlines(
                content,
                Format {
                    style: Some("CodeChar"),
                    ..format
                },
            ),
            ViewInline::Mark(content) => self.inlines(
                content,
                Format {
                    mark: true,
                    ..format
                },
            ),
            ViewInline::Superscript(content) => self.inlines(
                content,
                Format {
                    superscript: true,
                    ..format
                },
            ),
            ViewInline::Subscript(content) => self.inlines(
                content,
                Format {
                    subscript: true,
                    ..format
                },
            ),
            ViewInline::Span { roles, content } => {
                let has = |role: &str| roles.iter().any(|found| found == role);
                self.inlines(
                    content,
                    Format {
                        underline: format.underline || has("underline"),
                        strike: format.strike || has("line-through"),
                        ..format
                    },
                );
            }
            ViewInline::IndexTerm { content, .. } => self.inlines(content, format),
            ViewInline::Link { url, content } => {
                let shown = if content.is_empty() {
                    vec![ViewInline::Text(url.clone())]
                } else {
                    content.clone()
                };
                if safe_url(url) {
                    self.external_link(url, &shown, format);
                } else {
                    self.inlines(&shown, format);
                    self.warn(
                        None,
                        format!(
                            "the link {url} is not a web or mail address and is exported as text"
                        ),
                    );
                }
            }
            ViewInline::Xref {
                target,
                link,
                content,
                span,
            } => self.xref(target, link, content, span, format),
            ViewInline::Anchor(id) => {
                if let Some(name) = self.labels.get(&self.key, id).map(str::to_owned) {
                    self.bookmark(&name);
                }
            }
            ViewInline::Image(image) => self.image(image, format, None),
            ViewInline::Icon(name) => self.text(&format!("[{name}]"), format),
            ViewInline::FootnoteRef { number, repeat, .. } => self.footnote(*number, *repeat),
            ViewInline::LineBreak => {
                self.xml.open("w:r", &[]);
                self.xml.empty("w:br", &[]);
                self.xml.close("w:r");
            }
            ViewInline::Keyboard(keys) => {
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        self.text("+", format);
                    }
                    self.text(
                        key,
                        Format {
                            style: Some("Keyboard"),
                            ..format
                        },
                    );
                }
            }
            ViewInline::Button(label) => self.text(
                &format!("[{label}]"),
                Format {
                    bold: true,
                    ..format
                },
            ),
            ViewInline::Menu(items) => self.text(
                &items.join(" › "),
                Format {
                    bold: true,
                    ..format
                },
            ),
            ViewInline::Stem {
                source, rendered, ..
            } => self.inline_formula(source, rendered.as_ref(), format),
        }
    }

    /// The relationship id of a hyperlink to `url` from the part being written.
    pub(crate) fn hyperlink(&mut self, url: &str) -> String {
        let links = &mut self.hyperlinks[self.part.0];
        let url = url.trim().replace(' ', "%20");
        if let Some(link) = links.iter().find(|link| link.url == url) {
            return link.id.clone();
        }
        let id = format!("rLink{}", links.len() + 1);
        links.push(Hyperlink {
            id: id.clone(),
            url,
        });
        id
    }

    pub(crate) fn external_link(&mut self, url: &str, content: &[ViewInline], format: Format) {
        let id = self.hyperlink(url);
        self.xml
            .open("w:hyperlink", &[("r:id", &id), ("w:history", "1")]);
        self.inlines(
            content,
            Format {
                style: Some("Hyperlink"),
                ..format
            },
        );
        self.xml.close("w:hyperlink");
    }

    /// Where a reference leads: a label of the document, a page of the site, or nowhere. As
    /// the PDF exporter decides it, with the reason a reference stays text.
    fn destination(&self, target: &str, link: &ResolvedLink) -> Destination {
        let fragment = link.fragment.clone().or_else(|| {
            target
                .split_once('#')
                .map(|(_, fragment)| fragment.to_owned())
        });
        if link.unresolved {
            return Destination::Nowhere(format!(
                "the cross reference {target} leads to nothing and is exported as text"
            ));
        }
        let page = match &link.target {
            Some(location) if location.path != self.source => self.pages.get(&location.path),
            _ => Some(&self.key),
        };
        if let Some(page) = page {
            let found = fragment
                .as_deref()
                .and_then(|fragment| self.labels.get(page, fragment))
                .or_else(|| self.labels.get(page, ""));
            return match found {
                Some(name) => Destination::Bookmark(name.to_owned()),
                None => Destination::Nowhere(format!(
                    "the cross reference {target} names nothing in the document and is exported as text"
                )),
            };
        }
        let url = link
            .target
            .as_ref()
            .and_then(|location| self.options.page_urls.get(&location.path));
        match url {
            Some(url) => {
                let mut url = url.clone();
                if let Some(fragment) = &fragment {
                    url.push('#');
                    url.push_str(fragment);
                }
                Destination::Url(url)
            }
            None => Destination::Nowhere(format!(
                "the cross reference {target} leads outside the document and is exported as text"
            )),
        }
    }

    fn xref(
        &mut self,
        target: &str,
        link: &ResolvedLink,
        content: &[ViewInline],
        span: &SourceSpan,
        format: Format,
    ) {
        let shown = if content.is_empty() {
            vec![ViewInline::Text(
                link.title.clone().unwrap_or_else(|| target.to_owned()),
            )]
        } else {
            content.to_vec()
        };
        match self.destination(target, link) {
            Destination::Bookmark(name) => {
                self.xml
                    .open("w:hyperlink", &[("w:anchor", &name), ("w:history", "1")]);
                self.inlines(
                    &shown,
                    Format {
                        style: Some("Hyperlink"),
                        ..format
                    },
                );
                self.xml.close("w:hyperlink");
            }
            Destination::Url(url) if safe_url(&url) => self.external_link(&url, &shown, format),
            Destination::Url(url) => {
                self.inlines(&shown, format);
                self.warn(
                    Some(span),
                    format!("the address {url} is not a web address and is exported as text"),
                );
            }
            Destination::Nowhere(reason) => {
                self.inlines(&shown, format);
                self.warn(Some(span), reason);
            }
        }
    }

    /// A footnote reference: the first a Word footnote, with a bookmark around its mark; a
    /// later one a `NOTEREF` field to that bookmark, which Word shows as the same number.
    fn footnote(&mut self, number: u32, repeat: bool) {
        let mark = Format {
            style: Some("FootnoteReference"),
            ..Format::default()
        };
        if let Some((id, name)) = self.written_footnotes.get(&number).cloned() {
            if repeat || self.part == Part::FOOTNOTES {
                self.field(&format!(" NOTEREF {name} \\f \\h "), &id.to_string(), mark);
                return;
            }
        }
        let content = match self.page_footnotes.get(&number) {
            Some(content) if !repeat && self.part == Part::DOCUMENT => content.clone(),
            _ => {
                self.text(&format!("[{number}]"), mark);
                self.warn(
                    None,
                    format!("footnote {number} is not in the page and is exported as its number"),
                );
                return;
            }
        };
        self.footnote_count += 1;
        let id = self.footnote_count;
        let name = format!("_f{id}");
        self.written_footnotes.insert(number, (id, name.clone()));
        self.placed.insert(name.clone());
        self.bookmark_count += 1;
        let bookmark = self.bookmark_count.to_string();
        self.xml
            .empty("w:bookmarkStart", &[("w:id", &bookmark), ("w:name", &name)]);
        self.xml.open("w:r", &[]);
        self.run_properties(mark);
        self.xml
            .empty("w:footnoteReference", &[("w:id", &id.to_string())]);
        self.xml.close("w:r");
        self.xml.empty("w:bookmarkEnd", &[("w:id", &bookmark)]);

        // The footnote itself, written into the footnotes part.
        let body = std::mem::replace(&mut self.xml, std::mem::take(&mut self.footnotes));
        let part = std::mem::replace(&mut self.part, Part::FOOTNOTES);
        let after_table = std::mem::replace(&mut self.after_table, false);
        self.xml.open("w:footnote", &[("w:id", &id.to_string())]);
        self.xml.open("w:p", &[]);
        self.xml.open("w:pPr", &[]);
        let style = self.style_id("FootnoteText");
        self.xml.empty("w:pStyle", &[("w:val", &style)]);
        self.xml.close("w:pPr");
        self.xml.open("w:r", &[]);
        self.run_properties(mark);
        self.xml.empty("w:footnoteRef", &[]);
        self.xml.close("w:r");
        self.text(" ", Format::default());
        self.inlines(&content, Format::default());
        self.xml.close("w:p");
        self.xml.close("w:footnote");
        self.footnotes = std::mem::replace(&mut self.xml, body);
        self.part = part;
        self.after_table = after_table;
    }

    /// A field: its instruction, and the result shown until Word updates it.
    pub(crate) fn field(&mut self, instruction: &str, result: &str, format: Format) {
        self.xml.open("w:r", &[]);
        self.xml.empty("w:fldChar", &[("w:fldCharType", "begin")]);
        self.xml.close("w:r");
        self.xml.open("w:r", &[]);
        self.xml
            .element("w:instrText", &[("xml:space", "preserve")], instruction);
        self.xml.close("w:r");
        self.xml.open("w:r", &[]);
        self.xml
            .empty("w:fldChar", &[("w:fldCharType", "separate")]);
        self.xml.close("w:r");
        self.text(result, format);
        self.xml.open("w:r", &[]);
        self.xml.empty("w:fldChar", &[("w:fldCharType", "end")]);
        self.xml.close("w:r");
    }
}

/// The footnotes part around the footnotes written: the separators Word expects first.
pub(crate) fn footnotes_part(footnotes: Xml) -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open("w:footnotes", &crate::parts::NAMESPACES);
    for (kind, id, mark) in [
        ("separator", "-1", "w:separator"),
        ("continuationSeparator", "0", "w:continuationSeparator"),
    ] {
        xml.open("w:footnote", &[("w:type", kind), ("w:id", id)]);
        xml.open("w:p", &[]);
        xml.open("w:pPr", &[]);
        xml.empty(
            "w:spacing",
            &[("w:after", "0"), ("w:line", "240"), ("w:lineRule", "auto")],
        );
        xml.close("w:pPr");
        xml.open("w:r", &[]);
        xml.empty(mark, &[]);
        xml.close("w:r");
        xml.close("w:p");
        xml.close("w:footnote");
    }
    xml.append(footnotes);
    xml.close("w:footnotes");
    xml.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::safe_url;

    #[test]
    fn only_web_mail_and_relative_addresses_are_links() {
        for url in [
            "https://example.com",
            "HTTP://example.com/a?b#c",
            "mailto:someone@example.com",
            "ftp://example.com/file",
            "tel:+441234",
            "other.html",
            "../guide/page.html#part",
            "page.html?x=a:b",
        ] {
            assert!(safe_url(url), "{url}");
        }
        for url in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "vbscript:x",
            "",
            "https://example.com/\u{1}",
        ] {
            assert!(!safe_url(url), "{url}");
        }
    }
}
