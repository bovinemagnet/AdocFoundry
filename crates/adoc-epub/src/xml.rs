//! The one writer every XML file of a book goes through. Element and attribute names are the
//! crate's own; every document string reaches a file only through [`Xml::text`] or an attribute
//! value, escaped, with the characters XML 1.0 forbids replaced, so a document can never break
//! a file's markup.

/// An XML file, or a fragment of one, built in memory.
#[derive(Default)]
pub(crate) struct Xml {
    out: String,
}

/// The replacement for a character XML 1.0 has no place for: every control character but tab,
/// line feed and carriage return, and U+FFFE and U+FFFF. (A Rust string holds no surrogate.)
pub(crate) fn allowed(c: char) -> char {
    match c {
        '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}' => {
            '\u{FFFD}'
        }
        c => c,
    }
}

impl Xml {
    /// A file, starting with its XML declaration.
    pub(crate) fn file() -> Self {
        Self {
            out: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"),
        }
    }

    /// An XHTML content document, up to its open `body`: the declaration, the doctype, the
    /// root with its namespaces and language, and a `head` with `title` and a link to each of
    /// `stylesheets`.
    pub(crate) fn xhtml(language: &str, title: &str, stylesheets: &[String]) -> Self {
        let mut xml = Self::file();
        xml.raw("<!DOCTYPE html>\n");
        xml.open(
            "html",
            &[
                ("xmlns", "http://www.w3.org/1999/xhtml"),
                ("xmlns:epub", "http://www.idpf.org/2007/ops"),
                ("xml:lang", language),
                ("lang", language),
            ],
        );
        xml.open("head", &[]);
        xml.empty("meta", &[("charset", "UTF-8")]);
        xml.element("title", &[], title);
        for href in stylesheets {
            xml.empty(
                "link",
                &[("rel", "stylesheet"), ("type", "text/css"), ("href", href)],
            );
        }
        xml.close("head");
        xml
    }

    /// Markup the crate itself wrote: never document text.
    fn raw(&mut self, markup: &str) {
        self.out.push_str(markup);
    }

    fn start(&mut self, name: &str, attributes: &[(&str, &str)]) {
        self.out.push('<');
        self.out.push_str(name);
        for (attribute, value) in attributes {
            self.out.push(' ');
            self.out.push_str(attribute);
            self.out.push_str("=\"");
            for c in value.chars().map(allowed) {
                match c {
                    '&' => self.out.push_str("&amp;"),
                    '<' => self.out.push_str("&lt;"),
                    '>' => self.out.push_str("&gt;"),
                    '"' => self.out.push_str("&quot;"),
                    '\'' => self.out.push_str("&apos;"),
                    // Kept as written: a reader would read them as spaces.
                    '\t' => self.out.push_str("&#9;"),
                    '\n' => self.out.push_str("&#10;"),
                    '\r' => self.out.push_str("&#13;"),
                    c => self.out.push(c),
                }
            }
            self.out.push('"');
        }
    }

    pub(crate) fn open(&mut self, name: &str, attributes: &[(&str, &str)]) {
        self.start(name, attributes);
        self.out.push('>');
    }

    pub(crate) fn empty(&mut self, name: &str, attributes: &[(&str, &str)]) {
        self.start(name, attributes);
        self.out.push_str("/>");
    }

    pub(crate) fn close(&mut self, name: &str) {
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
    }

    /// A line break between elements, for a reader of the file; never inside text.
    pub(crate) fn newline(&mut self) {
        self.out.push('\n');
    }

    /// Document text, escaped. A carriage return is written as a reference, since a reader
    /// would otherwise read it as a line feed.
    pub(crate) fn text(&mut self, text: &str) {
        for c in text.chars().map(allowed) {
            match c {
                '&' => self.out.push_str("&amp;"),
                '<' => self.out.push_str("&lt;"),
                '>' => self.out.push_str("&gt;"),
                '\r' => self.out.push_str("&#13;"),
                c => self.out.push(c),
            }
        }
    }

    /// An element holding only `text`.
    pub(crate) fn element(&mut self, name: &str, attributes: &[(&str, &str)], text: &str) {
        self.open(name, attributes);
        self.text(text);
        self.close(name);
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.out.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_attributes_are_escaped() {
        let mut xml = Xml::default();
        xml.element("t", &[("v", "a\"b'<c>&\td\n")], "<&>\"'\r\u{1}");
        assert_eq!(
            String::from_utf8(xml.into_bytes()).unwrap(),
            "<t v=\"a&quot;b&apos;&lt;c&gt;&amp;&#9;d&#10;\">&lt;&amp;&gt;\"'&#13;\u{FFFD}</t>"
        );
    }

    #[test]
    fn only_what_xml_forbids_is_replaced() {
        for c in [
            '\t', '\n', '\r', ' ', '\u{7F}', '\u{85}', '\u{D7FF}', '\u{E000}', '\u{FFFD}',
        ] {
            assert_eq!(allowed(c), c, "{c:?}");
        }
        for c in [
            '\u{0}', '\u{8}', '\u{B}', '\u{C}', '\u{1F}', '\u{FFFE}', '\u{FFFF}',
        ] {
            assert_eq!(allowed(c), '\u{FFFD}', "{c:?}");
        }
    }
}
