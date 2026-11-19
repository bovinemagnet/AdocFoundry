//! Inline content as DocBook inlines: `emphasis` (with `role="strong"` for strong text and
//! `role="marked"` for highlighted text), `literal` for code, `superscript`, `subscript`,
//! `phrase` for text with roles, `link` with `xlink:href`, `keycombo`/`keycap`, `guibutton`,
//! `menuchoice` and `indexterm`; a footnote is a `footnote` where it is first referenced and a
//! `footnoteref` where it is referenced again. A hard line break is Asciidoctor's
//! `<?asciidoc-br?>` instruction, the one processing instruction written, since DocBook has no
//! element for one. A passthrough is written as text, never as markup, with a warning.

use adoc_render::ViewInline;

use crate::document::Writer;

/// The processing instruction for a hard line break, as Asciidoctor's DocBook converter writes
/// it and its stylesheets read it.
pub(crate) const LINE_BREAK: &str = "asciidoc-br";

impl Writer<'_> {
    pub(crate) fn inlines(&mut self, content: &[ViewInline]) {
        for inline in content {
            self.inline(inline);
        }
    }

    fn inline(&mut self, inline: &ViewInline) {
        match inline {
            ViewInline::Text(text) => self.xml.text(text),
            ViewInline::Strong(content) => self.wrapped("emphasis", &[("role", "strong")], content),
            ViewInline::Emphasis(content) => self.wrapped("emphasis", &[], content),
            ViewInline::Code(content) => self.wrapped("literal", &[], content),
            ViewInline::Mark(content) => self.wrapped("emphasis", &[("role", "marked")], content),
            ViewInline::Superscript(content) => self.wrapped("superscript", &[], content),
            ViewInline::Subscript(content) => self.wrapped("subscript", &[], content),
            ViewInline::Span { roles, content } => {
                let role = roles.join(" ");
                if role.is_empty() {
                    self.inlines(content);
                } else {
                    self.wrapped("phrase", &[("role", &role)], content);
                }
            }
            ViewInline::Link { url, content } => self.link(url, content),
            ViewInline::LineBreak => {
                self.xml.instruction(LINE_BREAK);
                self.xml.text("\n");
            }
            ViewInline::Keyboard(keys) => {
                let keys: Vec<&String> = keys.iter().filter(|key| !key.is_empty()).collect();
                if let [key] = keys.as_slice() {
                    self.xml.element("keycap", &[], key);
                } else if !keys.is_empty() {
                    self.xml.open("keycombo", &[]);
                    for key in keys {
                        self.xml.element("keycap", &[], key);
                    }
                    self.xml.close("keycombo");
                }
            }
            ViewInline::Button(label) => self.xml.element("guibutton", &[], label),
            ViewInline::Menu(items) => {
                let items: Vec<&String> = items.iter().filter(|item| !item.is_empty()).collect();
                if let [item] = items.as_slice() {
                    self.xml.element("guimenu", &[], item);
                } else if let Some((first, rest)) = items.split_first() {
                    self.xml.open("menuchoice", &[]);
                    self.xml.element("guimenu", &[], first);
                    if let Some((last, middle)) = rest.split_last() {
                        for item in middle {
                            self.xml.element("guisubmenu", &[], item);
                        }
                        self.xml.element("guimenuitem", &[], last);
                    }
                    self.xml.close("menuchoice");
                }
            }
            ViewInline::Icon(name) => self.xml.element("phrase", &[("role", "icon")], name),
            ViewInline::Image(image) => self.inline_picture(image),
            ViewInline::Stem {
                source, rendered, ..
            } => self.inline_equation(source, rendered.as_ref()),
            ViewInline::FootnoteRef { number, repeat, .. } => self.footnote(*number, *repeat),
            ViewInline::IndexTerm { terms, content } => {
                let terms: Vec<&String> = terms.iter().filter(|term| !term.is_empty()).collect();
                if !terms.is_empty() {
                    self.xml.open("indexterm", &[]);
                    for (name, term) in ["primary", "secondary", "tertiary"].iter().zip(&terms) {
                        self.xml.element(name, &[], term);
                    }
                    self.xml.close("indexterm");
                }
                self.inlines(content);
            }
            ViewInline::Raw(text) => {
                let source = self.page.span();
                self.warn(
                    "inline passthrough content is written as text, never as markup",
                    source.as_ref(),
                );
                self.xml.text(text);
            }
            other => self
                .xml
                .text(&ViewInline::plain_text(std::slice::from_ref(other))),
        }
    }

    fn wrapped(&mut self, name: &str, attributes: &[(&str, &str)], content: &[ViewInline]) {
        self.xml.open(name, attributes);
        self.inlines(content);
        self.xml.close(name);
    }

    /// A link to `url`, with its text, or the address when it has none. A link inside a link,
    /// which DocBook does not allow, keeps only its text.
    fn link(&mut self, url: &str, content: &[ViewInline]) {
        if self.in_link {
            self.inlines(content);
            return;
        }
        self.in_link = true;
        self.xml.open("link", &[("xlink:href", url)]);
        if content.is_empty() {
            self.xml.text(url);
        } else {
            self.inlines(content);
        }
        self.xml.close("link");
        self.in_link = false;
    }

    /// The footnote `number` of the page: the note itself where it is first referenced, a
    /// reference to it after.
    fn footnote(&mut self, number: u32, repeat: bool) {
        let id = format!("{}-fn{number}", self.page.key);
        if repeat && self.page.noted.contains(&number) {
            self.xml.empty("footnoteref", &[("linkend", &id)]);
            return;
        }
        let Some(note) = self
            .page
            .footnotes
            .iter()
            .find(|note| note.number == number)
            .map(|note| note.content.clone())
        else {
            let source = self.page.span();
            self.warn(format!("footnote {number} has no text"), source.as_ref());
            return;
        };
        if self.in_footnote || !self.page.noted.insert(number) {
            // A note inside a note, or one already written: its text stays where it is.
            self.inlines(&note);
            return;
        }
        self.in_footnote = true;
        self.xml.open("footnote", &[("xml:id", &id)]);
        self.xml.open("para", &[]);
        self.inlines(&note);
        self.xml.close("para");
        self.xml.close("footnote");
        self.in_footnote = false;
    }
}
