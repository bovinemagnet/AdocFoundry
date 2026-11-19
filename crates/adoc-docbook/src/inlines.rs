//! Inline content as DocBook inlines.

use adoc_render::ViewInline;

use crate::document::Writer;

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
            ViewInline::Superscript(content) => self.wrapped("superscript", &[], content),
            ViewInline::Subscript(content) => self.wrapped("subscript", &[], content),
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
}
