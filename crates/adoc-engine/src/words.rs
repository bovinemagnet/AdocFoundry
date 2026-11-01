//! The active page's word count and reading time: its prose, counted from the parsed page on a
//! worker once typing has paused for [`WORD_COUNT_SETTLE`].

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use adoc_dom::{Block, BlockKind, Dom, Inline};

use crate::{workspace::WorkerMessage, DocumentId, Engine, EngineEvent};

/// How long the active page must go unchanged before it is counted again.
pub const WORD_COUNT_SETTLE: Duration = Duration::from_millis(500);

/// The reading speed the reading time assumes.
pub const WORDS_PER_MINUTE: usize = 200;

/// A page's words and the whole minutes they take to read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordCount {
    pub words: usize,
    pub minutes: usize,
}

impl WordCount {
    #[must_use]
    pub fn new(words: usize) -> Self {
        Self {
            words,
            minutes: words.div_ceil(WORDS_PER_MINUTE),
        }
    }
}

/// The words in `text`: runs between white space that hold a letter or a digit, so a dash or a
/// delimiter line is not one.
#[must_use]
pub fn count_words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
}

#[derive(Default)]
pub(crate) struct WordCounts {
    enabled: bool,
    /// The last count of each document, with the version it counted.
    counts: BTreeMap<DocumentId, (u64, WordCount)>,
    /// When each document last changed; one never changed may be counted at once.
    changed: BTreeMap<DocumentId, Instant>,
    /// The version being counted on a worker.
    counting: Option<(DocumentId, u64)>,
}

impl Engine {
    /// Counts the words of the active page whenever it settles. Only the app shows a count; the
    /// other front ends never parse a page for one.
    pub fn enable_word_count(&mut self) {
        self.words.enabled = true;
    }

    /// The last word count of document `id`, which may lag its text until typing settles.
    #[must_use]
    pub fn word_count(&self, id: DocumentId) -> Option<WordCount> {
        self.words.counts.get(&id).map(|(_, count)| *count)
    }

    /// The active AsciiDoc document's version when it has not been counted or is not being
    /// counted already.
    fn uncounted(&self) -> Option<(DocumentId, u64)> {
        if !self.words.enabled {
            return None;
        }
        let id = self.active?;
        let document = self
            .documents
            .get(&id)
            .filter(|document| document.asciidoc)?;
        let version = document.version;
        let known = |entry: Option<(DocumentId, u64)>| entry == Some((id, version));
        let counted = self
            .words
            .counts
            .get(&id)
            .map(|(counted, _)| (id, *counted));
        (!known(counted) && !known(self.words.counting)).then_some((id, version))
    }

    /// True while the active page waits to be counted.
    pub(crate) fn has_due_word_count(&self) -> bool {
        self.uncounted().is_some()
    }

    /// Notes that document `id` changed, so it is counted only once it settles.
    pub(crate) fn words_changed(&mut self, id: DocumentId) {
        if self.words.enabled {
            self.words.changed.insert(id, self.clock.now());
        }
    }

    /// Starts counting the active page when it has gone unchanged for [`WORD_COUNT_SETTLE`].
    /// Returns the number of counts started.
    pub(crate) fn start_due_word_count(&mut self) -> usize {
        let Some((id, version)) = self.uncounted() else {
            return 0;
        };
        if let Some(changed) = self.words.changed.get(&id) {
            if self.clock.now() < *changed + WORD_COUNT_SETTLE {
                return 0;
            }
        }
        let Some(input) = self.parse_input(id) else {
            return 0;
        };
        self.words.counting = Some((id, version));
        let parser = self.document_parser();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = |error| WorkerMessage::Crashed {
            job: crate::worker::Crashed::WordCount,
            error,
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let (dom, _) = input.parse(parser.as_ref());
            let _ = sender.send(WorkerMessage::WordsCounted {
                id,
                version,
                count: WordCount::new(prose_words(&dom)),
            });
        });
        1
    }

    pub(crate) fn words_counted(&mut self, id: DocumentId, version: u64, count: WordCount) {
        if self.words.counting == Some((id, version)) {
            self.words.counting = None;
        }
        if !self.documents.contains_key(&id) {
            return;
        }
        let changed = self.word_count(id) != Some(count);
        self.words.counts.insert(id, (version, count));
        if changed {
            self.emit(EngineEvent::WordCountChanged(id));
        }
    }

    pub(crate) fn forget_word_count(&mut self, id: DocumentId) {
        self.words.counts.remove(&id);
        self.words.changed.remove(&id);
    }
}

/// The words a reader of the page sees as prose: titles, paragraphs, lists, tables, notes and
/// footnotes, but not code, literal text, diagrams, formulas or passthrough content.
fn prose_words(dom: &Dom) -> usize {
    dom.title.as_deref().map_or(0, inline_words) + dom.blocks.iter().map(block_words).sum::<usize>()
}

fn block_words(block: &Block) -> usize {
    let title = inline_words(&block.meta.title);
    let own = match &block.kind {
        BlockKind::Section { title, blocks, .. } => {
            inline_words(title) + blocks.iter().map(block_words).sum::<usize>()
        }
        BlockKind::Heading { title, .. } => inline_words(title),
        BlockKind::Paragraph { inlines } => inline_words(inlines),
        BlockKind::List { items, .. } => items
            .iter()
            .map(|item| {
                inline_words(&item.content) + item.blocks.iter().map(block_words).sum::<usize>()
            })
            .sum(),
        BlockKind::DescriptionList { items } => items
            .iter()
            .map(|item| {
                inline_words(&item.term)
                    + inline_words(&item.content)
                    + item.blocks.iter().map(block_words).sum::<usize>()
            })
            .sum(),
        BlockKind::CalloutList { items } => items
            .iter()
            .map(|item| {
                inline_words(&item.content) + item.blocks.iter().map(block_words).sum::<usize>()
            })
            .sum(),
        BlockKind::Admonition { blocks, .. }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }
        | BlockKind::Example { blocks }
        | BlockKind::Open { blocks } => blocks.iter().map(block_words).sum(),
        BlockKind::Verse { text, .. } => count_words(text),
        BlockKind::Table {
            header,
            rows,
            footer,
            ..
        } => header
            .iter()
            .chain(rows)
            .chain(footer)
            .flat_map(|row| &row.cells)
            .flat_map(|cell| &cell.blocks)
            .map(block_words)
            .sum(),
        BlockKind::Listing { .. }
        | BlockKind::Literal { .. }
        | BlockKind::Image { .. }
        | BlockKind::Media { .. }
        | BlockKind::Diagram { .. }
        | BlockKind::Stem { .. }
        | BlockKind::Passthrough { .. }
        | BlockKind::ThematicBreak
        | BlockKind::PageBreak
        | BlockKind::Toc
        | BlockKind::Unsupported { .. } => 0,
    };
    title + own
}

fn inline_words(inlines: &[Inline]) -> usize {
    let mut text = String::new();
    push_prose(inlines, &mut text);
    count_words(&text)
}

/// The text of `inlines` as `Inline::plain_text` gives it, with footnotes but without formulas
/// or passthrough content. Each inline is separated by nothing, as the reader sees it.
fn push_prose(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Stem { .. } | Inline::Passthrough(_) => out.push(' '),
            Inline::Footnote { content, .. } => {
                out.push(' ');
                push_prose(content, out);
                out.push(' ');
            }
            Inline::Bold(content)
            | Inline::Italic(content)
            | Inline::Monospace(content)
            | Inline::Highlight(content)
            | Inline::Superscript(content)
            | Inline::Subscript(content)
            | Inline::Span { content, .. }
            | Inline::Link { text: content, .. }
            | Inline::Xref { text: content, .. }
            | Inline::IndexTerm {
                visible: content, ..
            } => push_prose(content, out),
            other => out.push_str(&Inline::plain_text(std::slice::from_ref(other))),
        }
    }
}
