//! Inline content as DocBook inlines: `emphasis` (with `role="strong"` for strong text and
//! `role="marked"` for highlighted text), `literal` for code, `superscript`, `subscript`,
//! `phrase` for text with roles, `link` with `xlink:href`, `keycombo`/`keycap`, `guibutton`,
//! `menuchoice` and `indexterm`; a footnote is a `footnote` where it is first referenced and a
//! `footnoteref` where it is referenced again. A hard line break is Asciidoctor's
//! `<?asciidoc-br?>` instruction, the one processing instruction written, since DocBook has no
//! element for one. A passthrough is written as text, never as markup, with a warning.

use adoc_dom::SourceSpan;
use adoc_render::{ResolvedLink, ViewInline};

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
            ViewInline::Anchor(anchor) => {
                if let Some(id) = self.anchor_id(anchor) {
                    self.xml.empty("anchor", &[("xml:id", &id)]);
                }
            }
            ViewInline::Xref {
                target,
                link,
                content,
                span,
            } => self.xref(target, link, content, span),
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

    /// A cross reference (PRD §5.3): to a place in the publication, a `link` to its id with the
    /// text the view gives it (the author's, else the target's title), or an `xref`, which the
    /// processor fills in, when there is neither; to a page outside it, a `link` to its published address where one is
    /// known; otherwise its text, with a warning. A `linkend` is only ever written to an id
    /// the publication has, so none dangles.
    fn xref(
        &mut self,
        target: &str,
        link: &ResolvedLink,
        content: &[ViewInline],
        span: &SourceSpan,
    ) {
        let fragment = link
            .fragment
            .clone()
            .or_else(|| {
                target
                    .split_once('#')
                    .map(|(_, fragment)| fragment.to_owned())
            })
            .filter(|fragment| !fragment.is_empty());
        if self.in_link {
            self.xref_text(target, link, content);
            return;
        }
        if link.unresolved {
            self.warn(
                format!(
                    "the cross reference {target} leads to nothing, so it is written as its text"
                ),
                Some(span),
            );
            self.xref_text(target, link, content);
            return;
        }
        let page = match &link.target {
            Some(location) => self.ids.page_of(&location.path, fragment.as_deref()),
            None => Some(self.page.index),
        };
        if let Some(page) = page {
            let id = match &fragment {
                Some(fragment) => self.ids.anchor(page, fragment),
                None => self.ids.page(page),
            }
            .map(str::to_owned);
            let Some(id) = id else {
                self.warn(
                    format!(
                        "the cross reference {target} names nothing in the publication, so it is \
                         written as its text"
                    ),
                    Some(span),
                );
                self.xref_text(target, link, content);
                return;
            };
            // With no text of the author's and no title to show, the view shows `[target]`;
            // an `xref` lets the processor write the target's own label instead.
            let placeholder = format!("[{target}]");
            let untitled = content.is_empty()
                || matches!(content, [ViewInline::Text(text)] if *text == placeholder);
            if untitled && link.title.is_none() {
                self.xml.empty("xref", &[("linkend", &id)]);
                return;
            }
            self.in_link = true;
            self.xml.open("link", &[("linkend", &id)]);
            self.xref_text(target, link, content);
            self.xml.close("link");
            self.in_link = false;
            return;
        }
        let url = link
            .target
            .as_ref()
            .and_then(|location| {
                self.options.page_urls.get(&location.path).or_else(|| {
                    self.options
                        .page_urls
                        .get(&adoc_core::normalize_path(&location.path))
                })
            })
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .cloned();
        match url {
            Some(mut url) => {
                if let Some(fragment) = &fragment {
                    url.push('#');
                    url.push_str(fragment);
                }
                self.in_link = true;
                self.xml.open("link", &[("xlink:href", &url)]);
                self.xref_text(target, link, content);
                self.xml.close("link");
                self.in_link = false;
            }
            None => {
                self.warn(
                    format!(
                        "the cross reference {target} leads outside the publication, so it is \
                         written as its text"
                    ),
                    Some(span),
                );
                self.xref_text(target, link, content);
            }
        }
    }

    /// A reference's text: the author's, else its target's title, else the target as written.
    fn xref_text(&mut self, target: &str, link: &ResolvedLink, content: &[ViewInline]) {
        if content.is_empty() {
            let shown = link.title.clone().unwrap_or_else(|| target.to_owned());
            self.xml.text(&shown);
        } else {
            self.inlines(content);
        }
    }

    /// The footnote `number` of the page: the note itself where it is first referenced, a
    /// reference to it after.
    fn footnote(&mut self, number: u32, repeat: bool) {
        if let Some(id) = self.page.notes.get(&number).filter(|_| repeat).cloned() {
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
        if self.in_footnote || self.page.notes.contains_key(&number) {
            // A note inside a note, or one already written: its text stays where it is.
            self.inlines(&note);
            return;
        }
        let id = self.ids.allocate(&format!("{}.fn{number}", self.page.key));
        self.page.notes.insert(number, id.clone());
        self.in_footnote = true;
        self.xml.open("footnote", &[("xml:id", &id)]);
        self.xml.open("para", &[]);
        self.inlines(&note);
        self.xml.close("para");
        self.xml.close("footnote");
        self.in_footnote = false;
    }
}
