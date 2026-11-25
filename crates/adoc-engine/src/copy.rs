//! Copying rendered text (#794): what a selection in the visual editor or the preview covers, as
//! plain text, AsciiDoc, Markdown or HTML.
//!
//! A selection is two places in the page's text ([`adoc_render::fragment`]). In the visual
//! editor a place is a source offset where the text is the editor's own, and a leaf of the view
//! tree where it is drawn from that, as everything an include brings in is: [`VisualFlow`] puts
//! the two in one order. What is copied comes from the model and the view tree, never from what
//! is laid out, so text out of view is copied too.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_dom::{ParseInput, SourceSpan};
use adoc_model::{Atom, BlockPath, Node, NodeKind, Piece, TextBlock, TextContext, ViewText};
use adoc_render::{
    fragment::{Flow, Fragment, FragmentBlock, LeafKey, Part, Place},
    ResolvedLink, ViewTree,
};

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, VisualDocument,
};

/// The form text is copied in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyFormat {
    /// No formatting but spaces, tabs, new lines and list markers.
    Plain,
    AsciiDoc,
    Markdown,
    /// HTML, with plain text beside it.
    Formatted,
}

/// A place in the visual editor's text.
#[derive(Clone, Debug, PartialEq)]
pub enum TextPlace {
    /// An offset in the page's source, in text the editor draws itself.
    Source(usize),
    /// An offset in the text of a leaf drawn from the view tree.
    Drawn { leaf: LeafKey, offset: usize },
}

/// A selection to copy.
#[derive(Clone, Debug, PartialEq)]
pub enum CopySource {
    /// In the visual editor, its ends in either order.
    Visual {
        id: DocumentId,
        /// The document version the ends are in.
        version: u64,
        from: TextPlace,
        to: TextPlace,
    },
    /// In the preview, each end a leaf and an offset in its text.
    Preview {
        id: DocumentId,
        from: (LeafKey, usize),
        to: (LeafKey, usize),
    },
}

/// Text for the front end to put on the clipboard.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopiedText {
    pub text: String,
    /// The same as HTML, for [`CopyFormat::Formatted`].
    pub html: Option<String>,
}

/// The text of a visual document in the order it is read: the editor's own text blocks and the
/// leaves drawn from the view tree.
#[derive(Clone, Debug, Default)]
pub struct VisualFlow {
    flow: Flow,
    /// The leaf each of the editor's own text blocks shows.
    blocks: HashMap<BlockPath, usize>,
    /// The text block each leaf is shown by; none for a leaf drawn from the view tree.
    paths: Vec<Option<BlockPath>>,
}

impl VisualFlow {
    #[must_use]
    pub fn new(document: &VisualDocument) -> Self {
        Self::with(document, true)
    }

    /// With the editor's own text in place of the view tree's, or not.
    fn with(document: &VisualDocument, own: bool) -> Self {
        let mut flow = Flow::new(&document.view);
        let mut blocks = HashMap::new();
        let mut paths = vec![None; flow.leaves().len()];
        for (path, node, text) in document.model.text_blocks() {
            // Text the model misreads is drawn from the view tree.
            if node.drawn_from_view().is_some() {
                continue;
            }
            let Some(leaf) = leaf_of(node).and_then(|key| flow.position(&key)) else {
                continue;
            };
            if own {
                let key = flow.leaves()[leaf].key.clone();
                flow.set_text(&key, screen_text(text));
            }
            blocks.insert(path.clone(), leaf);
            paths[leaf] = Some(path);
        }
        Self {
            flow,
            blocks,
            paths,
        }
    }

    #[must_use]
    pub fn flow(&self) -> &Flow {
        &self.flow
    }

    /// Whether leaf `leaf` is drawn from the view tree rather than by the editor.
    #[must_use]
    pub fn is_drawn(&self, leaf: usize) -> bool {
        self.paths.get(leaf).is_some_and(Option::is_none)
    }

    /// The text block leaf `leaf` is shown by.
    #[must_use]
    pub fn block(&self, leaf: usize) -> Option<&BlockPath> {
        self.paths.get(leaf)?.as_ref()
    }

    /// Where `place` is. An offset inside what shows as one, such as a link's macro, is at its
    /// end when `end`.
    #[must_use]
    pub fn place(&self, document: &VisualDocument, place: &TextPlace, end: bool) -> Option<Place> {
        match place {
            TextPlace::Drawn { leaf, offset } => self.flow.place(leaf, *offset),
            TextPlace::Source(offset) => {
                let (path, node, text) = document.model.text_block_at(*offset)?;
                let offset = text.display_offset_towards(*offset, end)?;
                if let Some(leaf) = self.blocks.get(&path) {
                    return Some(Place::new(*leaf, offset));
                }
                // Text drawn from the view tree, where the caret is in the model's.
                leaf_of(node)
                    .and_then(|key| self.flow.place(&key, offset))
                    .or_else(|| self.beside(document, &path, end))
            }
        }
    }

    /// The place nearest a text block that has no text, such as an empty cell: the end of the
    /// text before it when `before`, else the start of the text after it.
    fn beside(&self, document: &VisualDocument, path: &BlockPath, before: bool) -> Option<Place> {
        let blocks: Vec<BlockPath> = document
            .model
            .text_blocks()
            .map(|(path, _, _)| path)
            .collect();
        let at = blocks.iter().position(|other| other == path)?;
        let end_of = |leaf: usize| Place::new(leaf, self.flow.leaves()[leaf].text.len());
        let previous = || {
            blocks[..at]
                .iter()
                .rev()
                .find_map(|path| self.blocks.get(path))
                .map(|leaf| end_of(*leaf))
        };
        let next = || {
            blocks[at + 1..]
                .iter()
                .find_map(|path| self.blocks.get(path))
                .map(|leaf| Place::new(*leaf, 0))
        };
        if before {
            previous().or_else(next)
        } else {
            next().or_else(previous)
        }
    }

    /// Whether the editor's text block at `path` has text that can be selected.
    #[must_use]
    pub fn has_text(&self, path: &BlockPath) -> bool {
        self.blocks.contains_key(path)
    }

    /// Two places in order.
    #[must_use]
    pub fn ordered(
        &self,
        document: &VisualDocument,
        one: &TextPlace,
        other: &TextPlace,
    ) -> Option<(Place, Place)> {
        let (first, second) = (
            self.place(document, one, false)?,
            self.place(document, other, false)?,
        );
        // The end of what shows as one is taken at the later end.
        if first <= second {
            Some((first, self.place(document, other, true)?))
        } else {
            Some((second, self.place(document, one, true)?))
        }
    }
}

/// The leaf of the view tree that a text node shows.
fn leaf_of(node: &Node) -> Option<LeafKey> {
    if node.kind == NodeKind::Title {
        return Some(LeafKey::new(&[], Part::DocumentTitle));
    }
    Some(match &node.view_text {
        Some(ViewText::Title) => LeafKey::new(node.origin.as_deref()?, Part::Heading),
        Some(ViewText::Item { list, index }) => LeafKey::new(list, Part::Item(*index)),
        Some(ViewText::Term { list, index }) => LeafKey::new(list, Part::Term(*index)),
        Some(ViewText::Description { list, index }) => {
            LeafKey::new(list, Part::Description(*index))
        }
        Some(ViewText::Block) | None => LeafKey::new(node.origin.as_deref()?, Part::Body),
    })
}

/// A stretch of a text block: where it shows, and its atom if it is one.
struct Stretch<'t> {
    display: std::ops::Range<usize>,
    atom: Option<&'t Atom>,
}

fn runs(block: &TextBlock) -> Vec<Stretch<'_>> {
    fn visit<'t>(pieces: &'t [Piece], at: &mut usize, out: &mut Vec<Stretch<'t>>) {
        for piece in pieces {
            let (length, atom) = match piece {
                Piece::Mark { children, .. } => {
                    visit(children, at, out);
                    continue;
                }
                Piece::Text { range } => (range.len(), None),
                Piece::Atom { atom, shown, .. } => (shown.len(), Some(atom)),
            };
            out.push(Stretch {
                display: *at..*at + length,
                atom,
            });
            *at += length;
        }
    }
    let mut out = Vec::new();
    visit(&block.pieces, &mut 0, &mut out);
    out
}

/// A text block's text as it is on screen, byte for byte with its display text: a line of the
/// source is a space, and a hard break a new line.
fn screen_text(block: &TextBlock) -> String {
    if block.context == TextContext::Verbatim {
        return block.display.clone();
    }
    let mut out = String::with_capacity(block.display.len());
    let mut after_break = false;
    for run in runs(block) {
        let part = &block.display[run.display];
        if matches!(run.atom, Some(Atom::LineBreak)) {
            out.push_str(&" ".repeat(part.len()));
            after_break = true;
            continue;
        }
        match part.strip_prefix('\n').filter(|_| after_break) {
            Some(rest) => {
                out.push('\n');
                out.push_str(&rest.replace('\n', " "));
            }
            None => out.push_str(&part.replace('\n', " ")),
        }
        after_break = false;
    }
    out
}

/// Where display offset `offset` of `block` is in `text`, the view tree's own text of it: they
/// differ where the editor shows an atom as it is written and the page as it renders.
fn aligned(block: &TextBlock, text: &str, offset: usize) -> usize {
    if block.display == text {
        return offset;
    }
    let flat = |text: &str| text.replace('\n', " ");
    let view = flat(text);
    let mut cursor = 0;
    let mut after_break = false;
    for run in runs(block) {
        if run.atom.is_some() {
            after_break = matches!(run.atom, Some(Atom::LineBreak));
            // Within an atom: where the text before it ended.
            if offset < run.display.end {
                return cursor;
            }
            continue;
        }
        let mut display = run.display.clone();
        if after_break && block.display[display.clone()].starts_with('\n') {
            display.start += 1;
        }
        after_break = false;
        let part = flat(&block.display[display.clone()]);
        let Some(found) = view.get(cursor..).and_then(|rest| rest.find(&part)) else {
            continue;
        };
        let start = cursor + found;
        if offset <= display.end {
            return start + offset.saturating_sub(display.start).min(part.len());
        }
        cursor = start + part.len();
    }
    text.len()
}

impl Engine {
    /// What `source` covers.
    fn fragment(
        &mut self,
        source: &CopySource,
        format: CopyFormat,
    ) -> Result<(Fragment, Copied), EngineError> {
        let stale = || EngineError::Refused("the page changed; nothing was copied".to_owned());
        match source {
            CopySource::Visual {
                id,
                version,
                from,
                to,
            } => {
                let current = self
                    .document(*id)
                    .ok_or(EngineError::UnknownDocument(*id))?
                    .version;
                if current != *version {
                    return Err(stale());
                }
                self.settle_visual(*id);
                let document = self.visual(*id).ok_or_else(|| {
                    EngineError::NotFound("the document is not shown in visual mode".to_owned())
                })?;
                let flow = VisualFlow::with(&document, format == CopyFormat::Plain);
                let own = VisualFlow::new(&document);
                let (from, to) = own.ordered(&document, from, to).ok_or_else(stale)?;
                // Offsets in the editor's text, in the view tree's where that is copied.
                let placed = |place: Place| {
                    if format == CopyFormat::Plain {
                        return place;
                    }
                    let Some(text) = own
                        .block(place.leaf)
                        .and_then(|path| document.model.node(path))
                        .and_then(|node| node.text.as_ref())
                    else {
                        return place;
                    };
                    let leaf = &flow.flow.leaves()[place.leaf].text;
                    let offset = aligned(text, leaf, place.offset);
                    Place::new(place.leaf, leaf.floor_char_boundary(offset))
                };
                let (from, to) = (placed(from), placed(to));
                let bullet = self.bullets();
                let fragment = flow.flow.cut(&document.view, from, to, &bullet);
                Ok((fragment, Copied::Visual(document)))
            }
            CopySource::Preview { id, from, to } => {
                let preview = self.rendered_preview(*id).ok_or_else(stale)?;
                let flow = Flow::new(&preview.view);
                let place = |(leaf, offset): &(LeafKey, usize)| flow.place(leaf, *offset);
                let (Some(one), Some(other)) = (place(from), place(to)) else {
                    return Err(stale());
                };
                let bullet = self.bullets();
                let fragment = flow.cut(&preview.view, one.min(other), one.max(other), &bullet);
                Ok((fragment, Copied::Preview(preview)))
            }
        }
    }

    /// The character each list item is written with.
    fn bullets(&self) -> impl Fn(&SourceSpan) -> Option<char> + '_ {
        let texts = RefCell::new(HashMap::new());
        move |span| {
            let mut texts = texts.borrow_mut();
            let text = texts
                .entry(span.path.clone())
                .or_insert_with(|| self.copied_file(&span.path));
            let rest = text.as_ref()?.get(span.range.start..)?;
            match rest.trim_start().chars().next()? {
                '-' => Some('-'),
                _ => Some('*'),
            }
        }
    }

    /// The text of a file of the workspace as it is now: its open buffer's, else what is
    /// indexed or on disk.
    fn copied_file(&self, path: &Path) -> Option<String> {
        if let Some(id) = self.document_for_path(path) {
            return self.document(id).map(crate::OpenDocument::text_string);
        }
        // A guest's shared page has no file, and only open ones are shared with it.
        if crate::engine::shared_page_path(path) {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }

    /// What `source` covers, in `format`.
    pub fn copy_text(
        &mut self,
        source: &CopySource,
        format: CopyFormat,
    ) -> Result<CopiedText, EngineError> {
        let (fragment, copied) = self.fragment(source, format)?;
        let view = copied.view();
        Ok(match format {
            CopyFormat::Plain => CopiedText {
                text: fragment.to_plain(),
                html: None,
            },
            CopyFormat::Markdown => CopiedText {
                text: fragment.to_markdown(),
                html: None,
            },
            CopyFormat::AsciiDoc => {
                let sources = self.block_sources(&fragment, view);
                CopiedText {
                    text: fragment.to_asciidoc(&|span| sources.get(&SpanKey::of(span)).cloned()),
                    html: None,
                }
            }
            CopyFormat::Formatted => {
                let urls = self
                    .site_url()
                    .map(|site| self.page_urls(&site))
                    .unwrap_or_default();
                CopiedText {
                    text: fragment.to_plain(),
                    html: Some(fragment.to_html(&|link| published(link, &urls))),
                }
            }
        })
    }

    /// The code of the block at `trail` of a page shown visually, without its callouts, so that
    /// it can be run.
    pub fn visual_code(&mut self, id: DocumentId, trail: &[usize]) -> Option<String> {
        self.settle_visual(id);
        let document = self.visual(id)?;
        match &adoc_render::block_by_trail(&document.view, trail)?.kind {
            adoc_render::ViewKind::Code { text, callouts, .. } => {
                Some(adoc_render::fragment::runnable_code(text, callouts))
            }
            _ => None,
        }
    }

    /// The source of each block of `fragment` taken whole, where copying it as it is written
    /// gives the same text: it names no include or conditional, and reads on its own as it did
    /// on the page.
    fn block_sources(&mut self, fragment: &Fragment, view: &ViewTree) -> HashMap<SpanKey, String> {
        let mut whole: Vec<&FragmentBlock> = Vec::new();
        fragment.each_block(&mut |block| {
            if block.whole.is_some() {
                whole.push(block);
            }
        });
        let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
        let mut out = HashMap::new();
        for block in whole {
            let Some(span) = &block.whole else {
                continue;
            };
            let text = texts
                .entry(span.path.clone())
                .or_insert_with(|| self.copied_file(&span.path));
            let Some(source) = text
                .as_ref()
                .and_then(|text| text.get(span.range.start..span.range.end))
                .map(|source| source.trim_end().to_owned())
            else {
                continue;
            };
            if self.reads_alike(&source, block, span, view) {
                out.insert(SpanKey::of(span), source);
            }
        }
        out
    }

    /// Whether `source` read on its own, with the page's attributes, says what `block` does.
    fn reads_alike(
        &mut self,
        source: &str,
        block: &FragmentBlock,
        span: &SourceSpan,
        view: &ViewTree,
    ) -> bool {
        let directive = source.lines().any(|line| {
            ["include::", "ifdef::", "ifndef::", "ifeval::", "endif::"]
                .iter()
                .any(|name| line.starts_with(name))
        });
        if directive || source.is_empty() {
            return false;
        }
        let mut input = ParseInput::new(&span.path, source);
        input.attributes = view.attributes.clone().into_iter().collect();
        // Nothing is read for it: a shared page's text may be another participant's.
        input.includes = Arc::new(adoc_dom::NoIncludes);
        let dom = self.document_parser().parse(&input);
        if dom.blocks.len() != 1 {
            return false;
        }
        let (alone, _) = self.view_of(&span.path, &dom, false, false);
        let flow = Flow::new(&alone);
        let Some((from, to)) = flow.ends() else {
            // A diagram drawn as a picture has no text to compare.
            return matches!(
                block.kind,
                adoc_render::fragment::FragmentKind::Diagram(_)
                    | adoc_render::fragment::FragmentKind::Rule
                    | adoc_render::fragment::FragmentKind::PageBreak
                    | adoc_render::fragment::FragmentKind::Media { .. }
            );
        };
        let alone = flow.cut(&alone, from, to, &|_| None);
        let page = Fragment {
            blocks: vec![block.clone()],
            footnotes: Vec::new(),
        };
        uncounted(&alone) == uncounted(&page)
    }
}

/// A fragment's text without what is counted where it is: captions and footnotes.
fn uncounted(fragment: &Fragment) -> String {
    let mut fragment = fragment.clone();
    for block in &mut fragment.blocks {
        block.caption = None;
    }
    let mut out = String::new();
    let text = fragment.to_plain().replace('-', "*");
    let mut rest = text.as_str();
    while let Some(at) = rest.find('[') {
        out.push_str(&rest[..=at]);
        rest = rest[at + 1..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// The published address a cross reference leads to.
fn published(link: &ResolvedLink, urls: &HashMap<PathBuf, String>) -> Option<String> {
    let url = urls.get(&link.target.as_ref()?.path)?;
    Some(match &link.fragment {
        Some(fragment) => format!("{url}#{fragment}"),
        None => url.clone(),
    })
}

type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;

/// Where a block is written, as a key.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SpanKey {
    path: PathBuf,
    start: usize,
    end: usize,
}

impl SpanKey {
    fn of(span: &SourceSpan) -> Self {
        Self {
            path: span.path.clone(),
            start: span.range.start,
            end: span.range.end,
        }
    }
}

/// What a fragment was taken from.
enum Copied {
    Visual(Arc<VisualDocument>),
    Preview(Arc<crate::RenderedPreview>),
}

impl Copied {
    fn view(&self) -> &ViewTree {
        match self {
            Self::Visual(document) => &document.view,
            Self::Preview(preview) => &preview.view,
        }
    }
}

fn copy(
    engine: &mut Engine,
    args: CommandArgs,
    command: crate::CommandId,
    format: CopyFormat,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Copy(source) = args else {
        return Err(EngineError::InvalidArguments {
            command,
            expected: "a selection to copy",
        });
    };
    engine
        .copy_text(&source, format)
        .map(CommandOutcome::Copied)
}

pub(crate) fn register_copy(registry: &mut CommandRegistry) {
    let commands: [(crate::CommandId, &'static str, Run); 4] = [
        (
            ids::EDIT_COPY_PLAIN,
            "Copy as Plain Text",
            |engine, args| copy(engine, args, ids::EDIT_COPY_PLAIN, CopyFormat::Plain),
        ),
        (
            ids::EDIT_COPY_ASCIIDOC,
            "Copy as AsciiDoc",
            |engine, args| copy(engine, args, ids::EDIT_COPY_ASCIIDOC, CopyFormat::AsciiDoc),
        ),
        (
            ids::EDIT_COPY_MARKDOWN,
            "Copy as Markdown",
            |engine, args| copy(engine, args, ids::EDIT_COPY_MARKDOWN, CopyFormat::Markdown),
        ),
        (
            ids::EDIT_COPY_FORMATTED,
            "Copy as Formatted Text",
            |engine, args| {
                copy(
                    engine,
                    args,
                    ids::EDIT_COPY_FORMATTED,
                    CopyFormat::Formatted,
                )
            },
        ),
    ];
    for (id, title, run) in commands {
        register(
            registry,
            spec(
                id,
                title,
                CommandCategory::Edit,
                None,
                CommandContext::ActiveDocument,
            ),
            run,
        );
    }
}
