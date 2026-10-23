//! The `asciidoc-parser` implementation of [`DocumentParser`], a second opinion on the corpus.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use adoc_core::SourceRange;
use asciidoc_parser::{
    attributes::Attrlist,
    blocks::{
        AdmonitionVariant, Block as ApBlock, BreakType, ColumnStyle, FindBlocks as _,
        HorizontalAlignment, IsBlock, ListItemMarker, ListType, MediaType, QuoteType,
        TableCellContent, TableRow,
    },
    inlines::{CharRef, InlineNode, RefVariant, StemNotation, StyleVariant, UiKind},
    parser::{IncludeContent, IncludeFileHandler, IncludeResolution},
    Document, HasSpan, Parser, SafeMode, Span,
};

use crate::{
    AdmonitionKind, Align, Block, BlockKind, BlockMeta, Callout, CalloutItem, Cell, CellStyle,
    Column, ColumnWidth, DescriptionItem, Dom, DomWarning, IncludeResolver, Inline, ListItem,
    ListKind, MediaKind, ParseInput, Row, SourceSpan,
};

/// The block styles rendered as diagrams, as in the acdc implementation.
use crate::acdc::DIAGRAM_STYLES;

/// Files read for includes: the parser's file name for each, relative to the page directory,
/// mapped to the real file and its text.
type Included = Rc<RefCell<HashMap<String, (PathBuf, Arc<str>)>>>;

struct Includes {
    page_dir: PathBuf,
    primary: String,
    resolver: Arc<dyn IncludeResolver>,
    included: Included,
}

impl fmt::Debug for Includes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Includes").finish_non_exhaustive()
    }
}

impl IncludeFileHandler for Includes {
    fn resolve_target<'src>(
        &self,
        source: Option<&str>,
        target: &str,
        _attrlist: &Attrlist<'src>,
        _parser: &Parser,
    ) -> IncludeResolution {
        // The parser names a nested file by its path from the page's directory, so the file
        // asked for is that directory, the including file's directory and the target: the
        // same path acdc asks for.
        let container = source.filter(|source| *source != self.primary);
        let base = container
            .and_then(|container| Path::new(container).parent())
            .map_or_else(|| self.page_dir.clone(), |dir| self.page_dir.join(dir));
        let requested = base.join(target);
        match self.resolver.read(&requested) {
            Some((path, text)) => {
                let name = container
                    .and_then(|container| Path::new(container).parent())
                    .map_or_else(|| PathBuf::from(target), |dir| dir.join(target));
                self.included.borrow_mut().insert(
                    name.to_string_lossy().replace('\\', "/"),
                    (path, Arc::from(text.as_str())),
                );
                IncludeResolution::Found(IncludeContent::new(text))
            }
            None => IncludeResolution::NotFound,
        }
    }
}

/// Parses with `asciidoc-parser`.
#[derive(Clone, Copy, Debug, Default)]
pub struct AsciidocParser;

impl crate::DocumentParser for AsciidocParser {
    fn name(&self) -> &'static str {
        "asciidoc-parser"
    }

    fn parse(&self, input: &ParseInput<'_>) -> Dom {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse(input))).unwrap_or_else(
            |_| Dom {
                blocks: vec![crate::acdc::unsupported_page(input.path, input.text)],
                warnings: vec![DomWarning {
                    message: "the parser failed on this document".to_owned(),
                    span: None,
                }],
                ..Dom::default()
            },
        )
    }
}

fn parse(input: &ParseInput<'_>) -> Dom {
    let page = input.path.to_path_buf();
    let primary = page.to_string_lossy().into_owned();
    let included = Included::default();
    let mut parser = Parser::default()
        .with_safe_mode(SafeMode::Unsafe)
        .with_primary_file_name(&primary)
        .with_include_file_handler(Includes {
            page_dir: page.parent().map_or_else(PathBuf::new, Path::to_path_buf),
            primary: primary.clone(),
            resolver: input.includes.clone(),
            included: included.clone(),
        });
    for (name, value) in &input.attributes {
        parser = parser.with_intrinsic_attribute(
            name,
            value,
            asciidoc_parser::parser::ModificationContext::Anywhere,
        );
    }
    let document = parser.parse(input.text);
    let included = included.borrow().clone();
    let converter = Converter {
        document: &document,
        page,
        page_text: input.text,
        included,
    };
    converter.document()
}

struct Converter<'d> {
    document: &'d Document<'static>,
    page: PathBuf,
    page_text: &'d str,
    included: HashMap<String, (PathBuf, Arc<str>)>,
}

impl Converter<'_> {
    fn document(&self) -> Dom {
        let title = self
            .document
            .header()
            .title()
            .map(|title| vec![Inline::Text(strip_html(title))]);
        let warnings = self
            .document
            .warnings()
            .map(|warning| DomWarning {
                message: format!("{:?}", warning.warning),
                span: Some(self.span(warning.source)),
            })
            .collect();
        let mut attributes = BTreeMap::new();
        for name in [
            "toc",
            "sectnums",
            "imagesdir",
            "toclevels",
            "experimental",
            "toc-title",
        ] {
            if let Some(value) = attribute_text(self.document, name) {
                attributes.insert(name.to_owned(), value);
            }
        }
        Dom {
            title,
            attributes,
            blocks: self.blocks(self.document.child_blocks()),
            warnings,
        }
    }

    // --- spans -----------------------------------------------------------------------------

    fn span(&self, span: Span<'_>) -> SourceSpan {
        let origin = self.document.origin_of(span);
        let (path, text, chain) = match origin.file {
            Some(file) if file != self.page.to_string_lossy() => match self.included.get(file) {
                Some((path, text)) => (path.clone(), &**text, vec![file.to_owned()]),
                None => (self.page.clone(), self.page_text, Vec::new()),
            },
            _ => (self.page.clone(), self.page_text, Vec::new()),
        };
        let line_start = text
            .split_inclusive('\n')
            .take(origin.line.saturating_sub(1))
            .map(str::len)
            .sum::<usize>()
            .min(text.len());
        let column = origin.col.unwrap_or(1).saturating_sub(1);
        let start = text[line_start..]
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(offset, _)| line_start + offset);
        let mut end = (start + span.data().len()).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        SourceSpan {
            path,
            range: SourceRange::new(start, end.max(start)),
            line: u32::try_from(origin.line).unwrap_or(u32::MAX),
            include_chain: chain,
        }
    }

    fn meta<'b>(&self, block: &'b ApBlock<'b>) -> BlockMeta {
        BlockMeta {
            span: self.span(block.span()),
            id: block.id().map(str::to_owned),
            roles: block.roles().into_iter().map(str::to_owned).collect(),
            title: block
                .title()
                .map(|title| vec![Inline::Text(strip_html(title))])
                .unwrap_or_default(),
            style: block.declared_style().map(str::to_owned),
            attributes: block
                .attrlist()
                .map(|attrlist| {
                    attrlist
                        .attributes()
                        .filter_map(|attribute| {
                            Some((attribute.name()?.to_owned(), attribute.value().to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    // --- blocks ----------------------------------------------------------------------------

    fn blocks<'b>(&self, blocks: impl Iterator<Item = &'b ApBlock<'b>>) -> Vec<Block> {
        blocks.flat_map(|block| self.block(block)).collect()
    }

    fn block<'b>(&self, block: &'b ApBlock<'b>) -> Vec<Block> {
        let meta = self.meta(block);
        let context = block.resolved_context().to_string();
        let style = block.resolved_style().unwrap_or("");
        let kind = match block {
            ApBlock::Preamble(preamble) => return self.blocks(preamble.child_blocks()),
            ApBlock::DocumentAttribute(_) => return Vec::new(),
            ApBlock::Section(section) if context == "floating_title" => BlockKind::Heading {
                level: u8::try_from(section.level()).unwrap_or(u8::MAX),
                title: vec![Inline::Text(strip_html(section.section_title()))],
            },
            ApBlock::Section(section) => BlockKind::Section {
                level: u8::try_from(section.level()).unwrap_or(u8::MAX),
                title: vec![Inline::Text(strip_html(section.section_title()))],
                blocks: self.blocks(section.child_blocks()),
            },
            ApBlock::Simple(simple) => {
                let inlines = self.inlines(simple.content().inlines());
                match context.as_ref() {
                    "listing" | "source" => {
                        let (text, callouts) = verbatim(simple.content().inlines());
                        BlockKind::Listing {
                            language: language(block),
                            text,
                            callouts,
                        }
                    }
                    "literal" => BlockKind::Literal {
                        text: verbatim(simple.content().inlines()).0,
                    },
                    "pass" => BlockKind::Passthrough {
                        content: verbatim(simple.content().inlines()).0,
                    },
                    "stem" | "latexmath" | "asciimath" => BlockKind::Stem {
                        latex: context == "latexmath" || style == "latexmath",
                        source: verbatim(simple.content().inlines()).0,
                    },
                    _ if DIAGRAM_STYLES.contains(&style) => BlockKind::Diagram {
                        kind: style.to_owned(),
                        source: verbatim(simple.content().inlines()).0,
                    },
                    // A line comment parses as a paragraph with nothing in it.
                    _ if inlines.is_empty() => return Vec::new(),
                    _ => BlockKind::Paragraph { inlines },
                }
            }
            ApBlock::RawDelimited(raw) => {
                let nodes = raw.content().inlines();
                if DIAGRAM_STYLES.contains(&style) {
                    BlockKind::Diagram {
                        kind: style.to_owned(),
                        source: verbatim(nodes).0,
                    }
                } else {
                    match context.as_ref() {
                        "comment" => return Vec::new(),
                        "literal" => BlockKind::Literal {
                            text: verbatim(nodes).0,
                        },
                        "pass" => BlockKind::Passthrough {
                            content: verbatim(nodes).0,
                        },
                        "stem" | "latexmath" | "asciimath" => BlockKind::Stem {
                            latex: context == "latexmath",
                            source: verbatim(nodes).0,
                        },
                        _ => {
                            let (text, callouts) = verbatim(nodes);
                            BlockKind::Listing {
                                language: language(block),
                                text,
                                callouts,
                            }
                        }
                    }
                }
            }
            ApBlock::CompoundDelimited(compound) => {
                let blocks = self.blocks(compound.child_blocks());
                if let Some(kind) = AdmonitionKind::from_style(style) {
                    BlockKind::Admonition { kind, blocks }
                } else {
                    match context.as_ref() {
                        "sidebar" => BlockKind::Sidebar { blocks },
                        "example" => BlockKind::Example { blocks },
                        _ => BlockKind::Open { blocks },
                    }
                }
            }
            ApBlock::Admonition(admonition) => {
                let kind = match admonition.variant() {
                    AdmonitionVariant::Note => AdmonitionKind::Note,
                    AdmonitionVariant::Tip => AdmonitionKind::Tip,
                    AdmonitionVariant::Important => AdmonitionKind::Important,
                    AdmonitionVariant::Caution => AdmonitionKind::Caution,
                    _ => AdmonitionKind::Warning,
                };
                let blocks = match admonition.content() {
                    Some(content) => vec![Block {
                        meta: BlockMeta {
                            span: meta.span.clone(),
                            ..BlockMeta::default()
                        },
                        kind: BlockKind::Paragraph {
                            inlines: self.inlines(content.inlines()),
                        },
                    }],
                    None => self.blocks(admonition.child_blocks()),
                };
                BlockKind::Admonition { kind, blocks }
            }
            ApBlock::Quote(quote) => {
                let attribution = quote.attribution().map(strip_html);
                let citation = quote.citetitle().map(strip_html);
                match quote.type_() {
                    QuoteType::Verse => BlockKind::Verse {
                        attribution,
                        citation,
                        text: quote
                            .content()
                            .map(|content| plain(&self.inlines(content.inlines())))
                            .unwrap_or_default(),
                    },
                    _ => BlockKind::Quote {
                        attribution,
                        citation,
                        blocks: match quote.content() {
                            Some(content) => vec![Block {
                                meta: BlockMeta {
                                    span: meta.span.clone(),
                                    ..BlockMeta::default()
                                },
                                kind: BlockKind::Paragraph {
                                    inlines: self.inlines(content.inlines()),
                                },
                            }],
                            None => self.blocks(quote.blocks().iter()),
                        },
                    },
                }
            }
            ApBlock::List(list) => {
                let items: Vec<_> = list
                    .child_blocks()
                    .filter_map(|item| match item {
                        ApBlock::ListItem(item) => Some(item),
                        _ => None,
                    })
                    .collect();
                match list.type_() {
                    ListType::Description => BlockKind::DescriptionList {
                        items: items
                            .iter()
                            .map(|item| {
                                let term = match item.list_item_marker() {
                                    ListItemMarker::DefinedTerm { term, .. } => {
                                        self.inlines(term.inlines())
                                    }
                                    _ => Vec::new(),
                                };
                                let (content, blocks) = self.principal(item);
                                DescriptionItem {
                                    span: self.span(item.span()),
                                    term,
                                    content,
                                    blocks,
                                }
                            })
                            .collect(),
                    },
                    ListType::Callout => BlockKind::CalloutList {
                        items: items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| {
                                let (content, blocks) = self.principal(item);
                                CalloutItem {
                                    span: self.span(item.span()),
                                    number: index + 1,
                                    content,
                                    blocks,
                                }
                            })
                            .collect(),
                    },
                    kind => BlockKind::List {
                        kind: match kind {
                            ListType::Ordered => ListKind::Ordered,
                            _ if list.is_checklist() => ListKind::Checklist,
                            _ => ListKind::Unordered,
                        },
                        items: items
                            .iter()
                            .map(|item| {
                                let (content, blocks) = self.principal(item);
                                ListItem {
                                    span: self.span(item.span()),
                                    checked: item.checkbox(),
                                    content,
                                    blocks,
                                }
                            })
                            .collect(),
                    },
                }
            }
            ApBlock::ListItem(item) => {
                let (inlines, blocks) = self.principal(item);
                let mut out = vec![Block {
                    meta: meta.clone(),
                    kind: BlockKind::Paragraph { inlines },
                }];
                out.extend(blocks);
                return out;
            }
            ApBlock::Media(media) => {
                let target = media
                    .target()
                    .map(|target| target.data().to_owned())
                    .unwrap_or_default();
                let attributes = media.macro_attrlist();
                let named = |name: &str, position: usize| {
                    attributes
                        .named_or_positional_attribute(name, position)
                        .map(|attribute| attribute.value().to_owned())
                };
                match media.type_() {
                    MediaType::Image => BlockKind::Image {
                        target,
                        alt: named("alt", 1).unwrap_or_default(),
                        width: named("width", 2),
                        height: named("height", 3),
                    },
                    MediaType::Video => BlockKind::Media {
                        kind: MediaKind::Video,
                        targets: vec![target],
                    },
                    MediaType::Audio => BlockKind::Media {
                        kind: MediaKind::Audio,
                        targets: vec![target],
                    },
                }
            }
            ApBlock::Break(brk) => match brk.type_() {
                BreakType::Page => BlockKind::PageBreak,
                _ => BlockKind::ThematicBreak,
            },
            ApBlock::Toc(_) => BlockKind::Toc,
            ApBlock::Table(table) => {
                let row = |row: &TableRow<'_>| Row {
                    cells: row
                        .cells()
                        .iter()
                        .map(|cell| self.cell(cell, &meta.span))
                        .collect(),
                };
                BlockKind::Table {
                    columns: table
                        .columns()
                        .iter()
                        .map(|column| Column {
                            width: ColumnWidth::Proportional(
                                u32::try_from(column.width()).unwrap_or(1),
                            ),
                            align: align(column.h_align()),
                            style: cell_style(column.style()),
                        })
                        .collect(),
                    header: table.header_row().map(row),
                    rows: table.body_rows().iter().map(row).collect(),
                    footer: table.footer_row().map(row),
                }
            }
            // The block enum is non-exhaustive: a new kind keeps its source.
            other => BlockKind::Unsupported {
                name: other.resolved_context().to_string(),
                source: other.span().data().to_owned(),
            },
        };
        vec![Block { meta, kind }]
    }

    /// A list item's principal text and the blocks attached to it.
    fn principal(&self, item: &asciidoc_parser::blocks::ListItem<'_>) -> (Vec<Inline>, Vec<Block>) {
        let mut children = item.child_blocks();
        let first = if item.has_empty_principal_text() {
            None
        } else {
            children.next()
        };
        let content = match first {
            Some(ApBlock::Simple(simple)) => self.inlines(simple.content().inlines()),
            Some(other) => {
                let mut blocks = self.block(other);
                blocks.extend(self.blocks(children));
                return (Vec::new(), blocks);
            }
            None => Vec::new(),
        };
        (content, self.blocks(children))
    }

    fn cell(&self, cell: &asciidoc_parser::blocks::TableCell<'_>, table: &SourceSpan) -> Cell {
        let blocks = match cell.content() {
            TableCellContent::Simple(content) => {
                let inlines = self.inlines(content.inlines());
                if inlines.is_empty() {
                    Vec::new()
                } else {
                    vec![Block {
                        meta: BlockMeta {
                            span: self.span(content.original()),
                            ..BlockMeta::default()
                        },
                        kind: BlockKind::Paragraph { inlines },
                    }]
                }
            }
            TableCellContent::AsciiDoc(document) => self.blocks(document.blocks().iter()),
            #[allow(unreachable_patterns)]
            _ => vec![Block {
                meta: BlockMeta {
                    span: table.clone(),
                    ..BlockMeta::default()
                },
                kind: BlockKind::Paragraph {
                    inlines: Vec::new(),
                },
            }],
        };
        Cell {
            blocks,
            colspan: cell.colspan(),
            rowspan: cell.rowspan(),
            align: Some(align(cell.h_align())),
            style: Some(cell_style(cell.style())),
        }
    }

    // --- inlines ---------------------------------------------------------------------------

    fn inlines(&self, nodes: &[InlineNode<'_>]) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::new();
        for node in nodes {
            for mut inline in self.inline(node) {
                // The text after a hard break starts with the line's newline; the break is it.
                if let (Inline::Text(next), Some(Inline::LineBreak)) = (&mut inline, out.last()) {
                    if let Some(rest) = next.strip_prefix('\n') {
                        *next = rest.to_owned();
                    }
                }
                if let (Inline::Text(next), Some(Inline::Text(previous))) =
                    (&inline, out.last_mut())
                {
                    previous.push_str(next);
                } else {
                    out.push(inline);
                }
            }
        }
        out
    }

    fn inline(&self, node: &InlineNode<'_>) -> Vec<Inline> {
        vec![match node {
            InlineNode::Text { value, .. } => Inline::Text(value.to_string()),
            InlineNode::CharRef { value, .. } => Inline::Text(char_ref(value)),
            InlineNode::Raw { value, .. } => Inline::Passthrough(value.to_string()),
            InlineNode::Styled(styled) => {
                let content = self.inlines(&styled.children);
                let inner = match styled.variant {
                    StyleVariant::Strong => Inline::Bold(content),
                    StyleVariant::Emphasis => Inline::Italic(content),
                    StyleVariant::Code => Inline::Monospace(content),
                    StyleVariant::Superscript => Inline::Superscript(content),
                    StyleVariant::Subscript => Inline::Subscript(content),
                    StyleVariant::Mark if !styled.roles.is_empty() => {
                        return vec![Inline::Span {
                            roles: styled.roles.iter().map(ToString::to_string).collect(),
                            content,
                        }];
                    }
                    StyleVariant::Mark => Inline::Highlight(content),
                    StyleVariant::Unquoted if styled.roles.is_empty() => return content,
                    StyleVariant::Unquoted => {
                        return vec![Inline::Span {
                            roles: styled.roles.iter().map(ToString::to_string).collect(),
                            content,
                        }];
                    }
                    StyleVariant::DoubleQuote => {
                        let mut quoted = vec![Inline::Text("\u{201c}".to_owned())];
                        quoted.extend(content);
                        quoted.push(Inline::Text("\u{201d}".to_owned()));
                        return quoted;
                    }
                    _ => {
                        let mut quoted = vec![Inline::Text("\u{2018}".to_owned())];
                        quoted.extend(content);
                        quoted.push(Inline::Text("\u{2019}".to_owned()));
                        return quoted;
                    }
                };
                if styled.roles.is_empty() {
                    inner
                } else {
                    Inline::Span {
                        roles: styled.roles.iter().map(ToString::to_string).collect(),
                        content: vec![inner],
                    }
                }
            }
            InlineNode::Ref(reference) => {
                let text = self.inlines(&reference.children);
                match reference.variant {
                    RefVariant::Link => Inline::Link {
                        target: reference.target.to_string(),
                        text,
                    },
                    _ => Inline::Xref {
                        target: reference.target.to_string(),
                        text,
                        span: self.span(reference.span()),
                    },
                }
            }
            InlineNode::Image(image) if image.is_icon => Inline::Icon {
                name: image.target.to_string(),
            },
            InlineNode::Image(image) => Inline::Image {
                target: image.target.to_string(),
                alt: image.alt.as_deref().unwrap_or("").to_owned(),
            },
            InlineNode::Footnote(footnote) => Inline::Footnote {
                id: footnote.id.as_deref().map(str::to_owned),
                number: footnote
                    .number
                    .as_deref()
                    .and_then(|number| number.parse().ok())
                    .unwrap_or(0),
                content: self.inlines(&footnote.children),
            },
            InlineNode::Anchor(anchor) => Inline::Anchor {
                id: anchor.id.to_string(),
            },
            InlineNode::Ui(ui) => match &ui.kind {
                UiKind::Keyboard(keys) => {
                    Inline::Keyboard(keys.iter().map(ToString::to_string).collect())
                }
                UiKind::Button(label) => Inline::Button(label.to_string()),
                UiKind::Menu {
                    menu,
                    submenus,
                    item,
                } => Inline::Menu(
                    std::iter::once(menu.to_string())
                        .chain(submenus.iter().map(ToString::to_string))
                        .chain(item.iter().map(ToString::to_string))
                        .collect(),
                ),
            },
            InlineNode::IndexTerm(_) | InlineNode::Callout(_) => return Vec::new(),
            InlineNode::Stem(stem) => Inline::Stem {
                latex: matches!(stem.notation, StemNotation::LatexMath),
                source: stem.value.to_string(),
            },
            InlineNode::LineBreak { .. } => Inline::LineBreak,
        }]
    }
}

/// Verbatim text with callout markers removed and recorded by line.
fn verbatim(nodes: &[InlineNode<'_>]) -> (String, Vec<Callout>) {
    let mut text = String::new();
    let mut callouts = Vec::new();
    for node in nodes {
        match node {
            InlineNode::Callout(callout) => {
                while text.ends_with(' ') {
                    text.pop();
                }
                for comment in ["//", "#", "--", ";;"] {
                    if let Some(rest) = text.strip_suffix(comment) {
                        if rest.is_empty() || rest.ends_with([' ', '\t', '\n']) {
                            text.truncate(rest.trim_end_matches([' ', '\t']).len());
                            break;
                        }
                    }
                }
                callouts.push(Callout {
                    line: text.matches('\n').count(),
                    number: callout.number.parse().unwrap_or(0),
                });
            }
            InlineNode::Text { value, .. } | InlineNode::Raw { value, .. } => text.push_str(value),
            InlineNode::CharRef { value, .. } => text.push_str(&char_ref(value)),
            InlineNode::LineBreak { .. } => text.push('\n'),
            InlineNode::Styled(styled) => text.push_str(&verbatim(&styled.children).0),
            _ => {}
        }
    }
    (text, callouts)
}

fn char_ref(value: &CharRef<'_>) -> String {
    match value {
        CharRef::Special(character) => character.to_string(),
        CharRef::Replacement(replacement) => (*replacement).to_owned(),
        CharRef::Entity(entity) => decode_entity(entity),
        #[allow(unreachable_patterns)]
        _ => String::new(),
    }
}

/// `&#169;`, `&#x2014;` or a few named entities as the character; anything else as written.
fn decode_entity(entity: &str) -> String {
    let name = entity.trim_start_matches('&').trim_end_matches(';');
    let character = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => name
            .strip_prefix("#x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| {
                name.strip_prefix('#')
                    .and_then(|decimal| decimal.parse().ok())
            })
            .and_then(char::from_u32),
    };
    character.map_or_else(|| entity.to_owned(), |character| character.to_string())
}

/// Rendered HTML (as the crate gives titles) as plain text.
fn strip_html(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut entity = String::new();
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '&' => entity.push('&'),
            ';' if !entity.is_empty() => {
                entity.push(';');
                text.push_str(&decode_entity(&entity));
                entity.clear();
            }
            _ if !entity.is_empty() => entity.push(character),
            _ => text.push(character),
        }
    }
    text.push_str(&entity);
    text
}

fn plain(inlines: &[Inline]) -> String {
    Inline::plain_text(inlines)
}

fn language(block: &ApBlock<'_>) -> Option<String> {
    let attrlist = block.attrlist()?;
    attrlist
        .named_attribute("language")
        .or_else(|| attrlist.nth_attribute(2))
        .map(|attribute| attribute.value().to_owned())
        .filter(|language| !language.is_empty())
}

fn attribute_text(document: &Document<'_>, name: &str) -> Option<String> {
    document
        .is_attribute_set(name)
        .then(|| match document.attribute_value(name) {
            asciidoc_parser::document::InterpretedValue::Value(value) => value.to_string(),
            _ => String::new(),
        })
}

fn align(alignment: HorizontalAlignment) -> Align {
    match alignment {
        HorizontalAlignment::Center => Align::Centre,
        HorizontalAlignment::Right => Align::End,
        _ => Align::Start,
    }
}

fn cell_style(style: ColumnStyle) -> CellStyle {
    match style {
        ColumnStyle::AsciiDoc => CellStyle::AsciiDoc,
        ColumnStyle::Emphasis => CellStyle::Emphasis,
        ColumnStyle::Header => CellStyle::Header,
        ColumnStyle::Literal => CellStyle::Literal,
        ColumnStyle::Monospace => CellStyle::Monospace,
        ColumnStyle::Strong => CellStyle::Strong,
        _ => CellStyle::Default,
    }
}
