//! A page's blocks as DocBook blocks, each as the element that means what it is (PRD §5.2):
//! sections nest; paragraphs are `para` (`formalpara` with a title); lists are `itemizedlist`,
//! `orderedlist`, `variablelist`, `qandaset` and `calloutlist`; listings are `programlisting`
//! with their callouts as `co`, literal blocks `screen`; admonitions their own elements;
//! quotations and verses `blockquote`; sidebars and examples their own elements; tables CALS
//! (`table.rs`). Nothing is converted from HTML, and a passthrough is written as text. What
//! has no DocBook element keeps its text, with a warning.

use adoc_dom::{AdmonitionKind, ListKind};
use adoc_render::{ViewBlock, ViewCallout, ViewDescription, ViewInline, ViewKind, ViewListItem};

use crate::document::Writer;

impl Writer<'_> {
    /// Writes `blocks`, whether they show anything or not.
    pub(crate) fn blocks(&mut self, blocks: &[ViewBlock]) -> bool {
        let mut wrote = false;
        for block in blocks {
            wrote |= self.block(block);
        }
        wrote
    }

    /// Writes the blocks of an element DocBook does not let be empty (a list item, an
    /// admonition, a cell…), with an empty paragraph when they show nothing: it adds nothing a
    /// reader sees.
    pub(crate) fn body(&mut self, blocks: &[ViewBlock]) {
        if !self.blocks(blocks) {
            self.xml.empty("para", &[]);
        }
    }

    /// A paragraph of `content`, then `blocks`, as a list item, a description or a callout
    /// holds them, never empty.
    fn item_body(&mut self, content: &[ViewInline], blocks: &[ViewBlock]) {
        let has_text = !ViewInline::plain_text(content).trim().is_empty()
            || content
                .iter()
                .any(|inline| !matches!(inline, ViewInline::Text(_)));
        if has_text {
            self.xml.open("para", &[]);
            self.inlines(content);
            self.xml.close("para");
            self.blocks(blocks);
        } else {
            self.body(blocks);
        }
    }

    /// A block's `title` child, when it has one.
    fn block_title(&mut self, block: &ViewBlock) {
        if !block.title.is_empty() {
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
        }
    }

    /// The id an anchor of the page being written has, the first time it is written.
    pub(crate) fn anchor_id(&mut self, anchor: &str) -> Option<String> {
        let id = self.ids.anchor(self.page.index, anchor)?.to_owned();
        self.ids.place(&id).then_some(id)
    }

    /// Writes `block`, with its id, and whether it wrote anything; a block that writes nothing
    /// but has an id leaves an `anchor` in its place, so a reference to it still lands.
    fn block(&mut self, block: &ViewBlock) -> bool {
        let id = block
            .id
            .as_deref()
            .filter(|id| !id.is_empty())
            .and_then(|id| self.anchor_id(id));
        let wrote = self.block_with(block, id.as_deref());
        if !wrote {
            if let Some(id) = &id {
                self.xml.open("para", &[]);
                self.xml.empty("anchor", &[("xml:id", id)]);
                self.xml.close("para");
                self.xml.newline();
                return true;
            }
        }
        wrote
    }

    fn block_with(&mut self, block: &ViewBlock, id: Option<&str>) -> bool {
        let roles = block.roles.join(" ");
        let mut role: Vec<(&str, &str)> = Vec::new();
        if let Some(id) = id {
            role.push(("xml:id", id));
        }
        if !roles.is_empty() {
            role.push(("role", roles.as_str()));
        }
        match &block.kind {
            ViewKind::Section { title, blocks, .. } => {
                self.xml.open("section", &role);
                self.xml.open("title", &[]);
                self.inlines(title);
                self.xml.close("title");
                self.xml.newline();
                self.body(blocks);
                self.xml.close("section");
            }
            ViewKind::Heading { level, title } => {
                let renderas = format!("sect{}", (*level).clamp(1, 5));
                let mut attributes = role.clone();
                attributes.push(("renderas", &renderas));
                self.xml.open("bridgehead", &attributes);
                self.inlines(title);
                self.xml.close("bridgehead");
            }
            ViewKind::Paragraph(content) => {
                let titled = !block.title.is_empty();
                if titled {
                    self.xml.open("formalpara", &role);
                    self.block_title(block);
                    self.xml.open("para", &[]);
                } else {
                    self.xml.open("para", &role);
                }
                self.inlines(content);
                self.xml.close("para");
                if titled {
                    self.xml.close("formalpara");
                }
            }
            ViewKind::List { kind, items } => self.list(block, *kind, items, &role),
            ViewKind::DescriptionList(items) => self.descriptions(block, items, &role),
            ViewKind::CalloutList(items) => self.callout_list(block, items, &role),
            ViewKind::Admonition { kind, blocks } => {
                let name = match kind {
                    AdmonitionKind::Note => "note",
                    AdmonitionKind::Tip => "tip",
                    AdmonitionKind::Important => "important",
                    AdmonitionKind::Caution => "caution",
                    AdmonitionKind::Warning => "warning",
                };
                if self.forbidden(name) {
                    // Inside another admonition or a note: its label and content, set apart by
                    // its role, since DocBook does not nest them.
                    self.warn(
                        format!(
                            "a {name} inside another admonition is written as its content, which \
                             DocBook allows there"
                        ),
                        Some(&block.span),
                    );
                    let mut attributes = role.clone();
                    attributes.retain(|(attribute, _)| *attribute != "role");
                    attributes.push(("role", name));
                    self.xml.open("para", &attributes);
                    self.xml.open("emphasis", &[("role", "strong")]);
                    if block.title.is_empty() {
                        self.xml.text(admonition_label(name));
                    } else {
                        self.inlines(&block.title);
                    }
                    self.xml.close("emphasis");
                    self.xml.close("para");
                    self.blocks(blocks);
                } else {
                    self.xml.open(name, &role);
                    self.block_title(block);
                    self.within(name, |writer| writer.body(blocks));
                    self.xml.close(name);
                }
            }
            ViewKind::Code {
                language,
                text,
                callouts,
            } => {
                let mut attributes = role.clone();
                if let Some(language) = language.as_deref().filter(|language| !language.is_empty())
                {
                    attributes.push(("language", language));
                }
                if block.options.contains("linenums") || block.attributes.contains_key("linenums") {
                    attributes.push(("linenumbering", "numbered"));
                }
                self.titled_verbatim(block, |writer| {
                    writer.listing(&attributes, text, callouts);
                });
            }
            ViewKind::Literal(text) => {
                self.titled_verbatim(block, |writer| {
                    writer.xml.element("screen", &role, text);
                });
            }
            ViewKind::Quote {
                attribution,
                citation,
                blocks,
            } => {
                self.xml.open("blockquote", &role);
                self.block_title(block);
                self.attribution(attribution.as_deref(), citation.as_deref());
                self.body(blocks);
                self.xml.close("blockquote");
            }
            ViewKind::Verse {
                attribution,
                citation,
                text,
            } => {
                self.xml.open("blockquote", &role);
                self.block_title(block);
                self.attribution(attribution.as_deref(), citation.as_deref());
                self.xml.element("literallayout", &[], text);
                self.xml.close("blockquote");
            }
            ViewKind::Sidebar(blocks) => {
                if self.forbidden("sidebar") {
                    // A sidebar in a sidebar: its title and content, since DocBook does not
                    // nest them.
                    self.title_para(&block.title);
                    let wrote = self.blocks(blocks);
                    if !wrote && block.title.is_empty() {
                        return false;
                    }
                } else {
                    self.xml.open("sidebar", &role);
                    self.block_title(block);
                    self.within("sidebar", |writer| writer.body(blocks));
                    self.xml.close("sidebar");
                }
            }
            ViewKind::Example(blocks) => {
                let formal = !block.title.is_empty() && !self.forbidden("example");
                let name = if formal { "example" } else { "informalexample" };
                self.xml.open(name, &role);
                if formal {
                    self.block_title(block);
                } else {
                    // An untitled example, or a titled one where an `example` may not be (in
                    // another, a figure, a table or an equation), keeps its title as a `para`.
                    self.title_para(&block.title);
                }
                if formal {
                    self.within("example", |writer| writer.body(blocks));
                } else {
                    self.body(blocks);
                }
                self.xml.close(name);
            }
            ViewKind::Open(blocks) => {
                // An open block only groups; its title, if any, leads its content, and its id
                // is an anchor before it.
                if let Some(id) = id {
                    self.xml.open("para", &[]);
                    self.xml.empty("anchor", &[("xml:id", id)]);
                    self.xml.close("para");
                }
                if !block.title.is_empty() {
                    self.xml.open("para", &[("role", "title")]);
                    self.inlines(&block.title);
                    self.xml.close("para");
                }
                let wrote = self.blocks(blocks);
                return wrote || !block.title.is_empty() || id.is_some();
            }
            ViewKind::Table(table) => self.table(block, table, &role),
            ViewKind::Image(image) => self.figure(block, image, &role),
            ViewKind::Diagram(diagram) => self.diagram(block, diagram, &role),
            ViewKind::Stem {
                source, rendered, ..
            } => self.equation(block, source, rendered, &role),
            ViewKind::Media { kind, targets } => {
                self.media(block, *kind == adoc_dom::MediaKind::Video, targets, &role);
            }
            ViewKind::Passthrough(text) => {
                self.warn(
                    "a passthrough block is written as text, never as markup",
                    Some(&block.span),
                );
                let mut attributes = role.clone();
                attributes.retain(|(name, _)| *name != "role");
                attributes.push(("role", "passthrough"));
                self.xml.element("literallayout", &attributes, text);
            }
            // A processor makes its own table of contents, and pagination and rules are its to
            // choose: a page break and a thematic break write nothing.
            ViewKind::Toc | ViewKind::PageBreak | ViewKind::ThematicBreak => return false,
            _ => {
                let text = block_text(block);
                self.warn(
                    format!(
                        "{} is not written as DocBook yet, so only its text is kept",
                        what(&block.kind)
                    ),
                    Some(&block.span),
                );
                if text.contains('\n') {
                    self.xml.element("literallayout", &role, &text);
                } else {
                    self.xml.element("para", &role, &text);
                }
            }
        }
        self.xml.newline();
        true
    }

    /// A listing or literal block, inside a `formalpara` when it has a title, as Asciidoctor
    /// writes one, since neither element has a title of its own.
    fn titled_verbatim(&mut self, block: &ViewBlock, write: impl FnOnce(&mut Self)) {
        if block.title.is_empty() {
            write(self);
        } else {
            self.xml.open("formalpara", &[]);
            self.block_title(block);
            self.xml.open("para", &[]);
            write(self);
            self.xml.close("para");
            self.xml.close("formalpara");
        }
    }

    /// A `programlisting` of `text`, each callout a `co` at the end of its line, named so the
    /// callout list after it can point at it.
    fn listing(&mut self, attributes: &[(&str, &str)], text: &str, callouts: &[adoc_dom::Callout]) {
        let listing = self.page.next_listing();
        let mut marks = Vec::new();
        self.xml.open("programlisting", attributes);
        let lines: Vec<&str> = text.split('\n').collect();
        for (index, line) in lines.iter().enumerate() {
            self.xml.text(line);
            for callout in callouts.iter().filter(|callout| callout.line == index) {
                let id = self.ids.allocate(&format!(
                    "{}.co{listing}-{}-{}",
                    self.page.key,
                    callout.number,
                    marks.len() + 1
                ));
                self.xml.empty("co", &[("xml:id", &id)]);
                marks.push((callout.number, id));
            }
            if index + 1 < lines.len() {
                self.xml.text("\n");
            }
        }
        self.xml.close("programlisting");
        self.page.callouts = marks;
    }

    /// A quotation's `attribution`: who said it, and where.
    fn attribution(&mut self, attribution: Option<&str>, citation: Option<&str>) {
        let attribution = attribution.filter(|text| !text.trim().is_empty());
        let citation = citation.filter(|text| !text.trim().is_empty());
        if attribution.is_none() && citation.is_none() {
            return;
        }
        self.xml.open("attribution", &[]);
        if let Some(attribution) = attribution {
            self.xml.text(attribution);
        }
        if let Some(citation) = citation {
            self.xml.element("citetitle", &[], citation);
        }
        self.xml.close("attribution");
    }

    fn list(
        &mut self,
        block: &ViewBlock,
        kind: ListKind,
        items: &[ViewListItem],
        role: &[(&str, &str)],
    ) {
        let mut attributes = role.to_vec();
        let name = match kind {
            ListKind::Ordered => {
                let numeration = match block.style.as_deref() {
                    Some("loweralpha") => Some("loweralpha"),
                    Some("upperalpha") => Some("upperalpha"),
                    Some("lowerroman") => Some("lowerroman"),
                    Some("upperroman") => Some("upperroman"),
                    Some("arabic") => Some("arabic"),
                    _ => None,
                };
                if let Some(numeration) = numeration {
                    attributes.push(("numeration", numeration));
                }
                if let Some(start) = block
                    .attributes
                    .get("start")
                    .filter(|start| start.trim().parse::<i64>().is_ok())
                {
                    attributes.push(("startingnumber", start.trim()));
                }
                "orderedlist"
            }
            ListKind::Unordered => "itemizedlist",
            ListKind::Checklist => {
                if attributes.is_empty() {
                    attributes.push(("role", "checklist"));
                }
                "itemizedlist"
            }
        };
        self.xml.open(name, &attributes);
        self.block_title(block);
        for item in items {
            let mark = match item.checked {
                // The marks DocBook's stylesheets draw as ticked and empty boxes.
                Some(true) => vec![("override", "checkedbox")],
                Some(false) => vec![("override", "checkbox")],
                None => Vec::new(),
            };
            self.xml.open("listitem", &mark);
            self.item_body(&item.content, &item.blocks);
            self.xml.close("listitem");
        }
        self.xml.close(name);
    }

    /// A description list: a `variablelist`, or a `qandaset` for the `qanda` style.
    fn descriptions(
        &mut self,
        block: &ViewBlock,
        items: &[ViewDescription],
        role: &[(&str, &str)],
    ) {
        if block.style.as_deref() == Some("qanda") {
            self.xml.open("qandaset", role);
            self.block_title(block);
            for item in items {
                self.xml.open("qandaentry", &[]);
                self.xml.open("question", &[]);
                self.xml.open("para", &[]);
                self.inlines(&item.term);
                self.xml.close("para");
                self.xml.close("question");
                self.xml.open("answer", &[]);
                self.item_body(&item.content, &item.blocks);
                self.xml.close("answer");
                self.xml.close("qandaentry");
            }
            self.xml.close("qandaset");
            return;
        }
        let mut attributes = role.to_vec();
        if attributes.is_empty() && block.style.as_deref() == Some("horizontal") {
            attributes.push(("role", "horizontal"));
        }
        self.xml.open("variablelist", &attributes);
        self.block_title(block);
        for item in items {
            self.xml.open("varlistentry", &[]);
            self.xml.open("term", &[]);
            self.inlines(&item.term);
            self.xml.close("term");
            self.xml.open("listitem", &[]);
            self.item_body(&item.content, &item.blocks);
            self.xml.close("listitem");
            self.xml.close("varlistentry");
        }
        self.xml.close("variablelist");
    }

    /// A callout list: each callout points at the marks of its number in the listing before
    /// it. With no listing marked so, the list is an ordered list, with a warning.
    fn callout_list(&mut self, block: &ViewBlock, items: &[ViewCallout], role: &[(&str, &str)]) {
        let marks = std::mem::take(&mut self.page.callouts);
        let refs: Vec<String> = items
            .iter()
            .map(|item| {
                marks
                    .iter()
                    .filter(|(number, _)| *number == item.number)
                    .map(|(_, id)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if refs.iter().any(String::is_empty) {
            self.warn(
                "a callout list has a number its listing does not mark, so it is written as an \
                 ordered list",
                Some(&block.span),
            );
            self.xml.open("orderedlist", role);
            self.block_title(block);
            for item in items {
                self.xml.open("listitem", &[]);
                self.item_body(&item.content, &item.blocks);
                self.xml.close("listitem");
            }
            self.xml.close("orderedlist");
            return;
        }
        self.xml.open("calloutlist", role);
        self.block_title(block);
        for (item, arearefs) in items.iter().zip(&refs) {
            self.xml.open("callout", &[("arearefs", arearefs)]);
            self.item_body(&item.content, &item.blocks);
            self.xml.close("callout");
        }
        self.xml.close("calloutlist");
    }
}

/// A block's kind, as a diagnostic names it.
fn what(kind: &ViewKind) -> String {
    match kind {
        ViewKind::Unsupported { name, .. } => format!("the {name} block"),
        ViewKind::List { .. } => "a list".to_owned(),
        ViewKind::DescriptionList(_) => "a description list".to_owned(),
        ViewKind::CalloutList(_) => "a callout list".to_owned(),
        ViewKind::Admonition { .. } => "an admonition".to_owned(),
        ViewKind::Code { .. } => "a source block".to_owned(),
        ViewKind::Literal(_) => "a literal block".to_owned(),
        ViewKind::Quote { .. } => "a quotation".to_owned(),
        ViewKind::Verse { .. } => "a verse".to_owned(),
        ViewKind::Sidebar(_) => "a sidebar".to_owned(),
        ViewKind::Example(_) => "an example".to_owned(),
        ViewKind::Open(_) => "an open block".to_owned(),
        ViewKind::Table(_) => "a table".to_owned(),
        ViewKind::Image(_) => "an image".to_owned(),
        ViewKind::Media { .. } => "an audio or video block".to_owned(),
        ViewKind::Diagram(_) => "a diagram".to_owned(),
        ViewKind::Stem { .. } => "a formula".to_owned(),
        ViewKind::Passthrough(_) => "a passthrough block".to_owned(),
        ViewKind::Heading { .. } => "a discrete heading".to_owned(),
        ViewKind::ThematicBreak => "a thematic break".to_owned(),
        ViewKind::PageBreak => "a page break".to_owned(),
        ViewKind::Section { .. } | ViewKind::Paragraph(_) | ViewKind::Toc => "a block".to_owned(),
    }
}

/// The text a block shows, its lines kept, for a block written as text alone.
pub(crate) fn block_text(block: &ViewBlock) -> String {
    let mut lines = Vec::new();
    push_text(block, &mut lines);
    lines.join("\n")
}

fn push_text(block: &ViewBlock, lines: &mut Vec<String>) {
    let inline = |content: &[ViewInline], lines: &mut Vec<String>| {
        let text = ViewInline::plain_text(content);
        if !text.trim().is_empty() {
            lines.push(text);
        }
    };
    let children = |blocks: &[ViewBlock], lines: &mut Vec<String>| {
        for block in blocks {
            push_text(block, lines);
        }
    };
    match &block.kind {
        ViewKind::Section { title, blocks, .. } => {
            inline(title, lines);
            children(blocks, lines);
        }
        ViewKind::Heading { title, .. } => inline(title, lines),
        ViewKind::Paragraph(content) => inline(content, lines),
        ViewKind::List { items, .. } => {
            for item in items {
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::DescriptionList(items) => {
            for item in items {
                inline(&item.term, lines);
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::CalloutList(items) => {
            for item in items {
                inline(&item.content, lines);
                children(&item.blocks, lines);
            }
        }
        ViewKind::Admonition { blocks, .. }
        | ViewKind::Quote { blocks, .. }
        | ViewKind::Sidebar(blocks)
        | ViewKind::Example(blocks)
        | ViewKind::Open(blocks) => children(blocks, lines),
        ViewKind::Code { text, .. }
        | ViewKind::Literal(text)
        | ViewKind::Verse { text, .. }
        | ViewKind::Passthrough(text) => lines.push(text.clone()),
        ViewKind::Table(table) => {
            let rows = table
                .header
                .iter()
                .chain(&table.rows)
                .chain(table.footer.iter());
            for row in rows {
                for cell in row {
                    children(&cell.blocks, lines);
                }
            }
        }
        ViewKind::Image(image) => lines.push(image.alt.clone()),
        ViewKind::Media { targets, .. } => lines.extend(targets.iter().cloned()),
        ViewKind::Diagram(diagram) => match diagram {
            adoc_render::DiagramView::Svg(_) => {}
            adoc_render::DiagramView::Pending { source, .. }
            | adoc_render::DiagramView::Failed { source, .. } => lines.push(source.clone()),
        },
        ViewKind::Stem { source, .. } | ViewKind::Unsupported { source, .. } => {
            lines.push(source.clone());
        }
        ViewKind::ThematicBreak | ViewKind::PageBreak | ViewKind::Toc => {}
    }
}

/// An admonition's label, as a reader sees it.
fn admonition_label(name: &str) -> &'static str {
    match name {
        "tip" => "Tip",
        "important" => "Important",
        "caution" => "Caution",
        "warning" => "Warning",
        _ => "Note",
    }
}
