//! KFX (KF10): the book as Amazon Ion fragments in a `CONT` container, written in-house (Kindle
//! PRD §6.5). KFX has no public specification. The fragment types, symbol numbers and container
//! layout here follow `ebook-rs` 0.18.6's MIT-licensed writer (`src/kfx/`; its notice is
//! `LICENSE-ebook-rs` beside this file), and the Ion encoding the Amazon Ion specification;
//! `docs/qa/spike-kindle.md` records the choice. One departure: the `YJ_symbols` import names
//! 842 symbols, so that local symbols starting at `$852` follow it as the Ion specification
//! counts, where `ebook-rs` names 851.
//!
//! Each chapter is a section (`$260`) whose one page template names a storyline (`$259`) of
//! blocks: text (`$269`), whose characters are in content fragments (`$145`) and whose bold,
//! italic, fixed-width and link runs are style events, pictures (`$271`) and tables (`$278`).
//! Links are anchors (`$266`) to a block's id and a character offset, or to an address. The
//! table of contents is the book navigation (`$389`); the position maps (`$264`, `$265`,
//! `$550`) give every block its place in the reading order.

use std::collections::{BTreeMap, HashMap};

use sha2::{Digest, Sha256};

use crate::book::{Block, Cell, Inline, KindleBook, Link, Note, ParagraphClass, Target, TocEntry};
use crate::{KindleError, KindleFormat};
use ion::Value;

pub(crate) mod ion;

/// The highest symbol of the shared `YJ_symbols` table version 10 this writer names.
const YJ_LAST: u32 = 851;
/// How many symbols the `YJ_symbols` import contributes after Ion's nine system symbols.
const YJ_MAX_ID: u32 = YJ_LAST - 9;
/// The id fragments of the book as a whole are written under.
const NO_ID: u32 = 348;
/// The most bytes of text one content fragment holds.
const CONTENT_SIZE: usize = 8192;
/// The first id given to a block.
const FIRST_EID: i64 = 1000;
/// The bytes before each entity's Ion: `ENTY`, version 1, a 21-byte header, and the entity's
/// own header `{$410: 0, $411: 0}` (no compression, no DRM).
const ENTITY_HEADER: [u8; 21] =
    *b"ENTY\x01\x00\x15\x00\x00\x00\xe0\x01\x00\xea\xd6\x03\x9a\x20\x03\x9b\x20";
const CONTAINER_HEADER: usize = 18;

// The `YJ_symbols` this writer names, by meaning.
const FONT_FAMILY: u32 = 11;
const FONT_STYLE: u32 = 12;
const FONT_WEIGHT: u32 = 13;
const FONT_SIZE: u32 = 16;
const LINE_HEIGHT: u32 = 42;
const STYLE_EVENTS: u32 = 142;
const OFFSET: u32 = 143;
const LENGTH: u32 = 144;
const CONTENT: u32 = 145;
const CONTENT_LIST: u32 = 146;
const COLUMN_SPAN: u32 = 148;
const ROW_SPAN: u32 = 149;
const ID: u32 = 155;
const STYLE: u32 = 157;
const TYPE: u32 = 159;
const FORMAT: u32 = 161;
const MIME: u32 = 162;
const EXTERNAL_RESOURCE: u32 = 164;
const LOCATION: u32 = 165;
const READING_ORDERS: u32 = 169;
const SECTIONS: u32 = 170;
const STYLE_NAME: u32 = 173;
const SECTION_NAME: u32 = 174;
const RESOURCE_NAME: u32 = 175;
const STORY_NAME: u32 = 176;
const READING_ORDER_NAME: u32 = 178;
const LINK_TO: u32 = 179;
const ANCHOR_NAME: u32 = 180;
const CONTAINS: u32 = 181;
const LOCATIONS: u32 = 182;
const POSITION: u32 = 183;
const PID: u32 = 184;
const EID: u32 = 185;
const URI: u32 = 186;
const NAV_TYPE: u32 = 235;
const NAV_CONTAINER_NAME: u32 = 239;
const REPRESENTATION: u32 = 241;
const LABEL: u32 = 244;
const TARGET_POSITION: u32 = 246;
const ENTRIES: u32 = 247;
const CONTAINER_LIST: u32 = 252;
const METADATA: u32 = 258;
const STORYLINE: u32 = 259;
const SECTION: u32 = 260;
const ANCHOR: u32 = 266;
const PAGE_TEMPLATES: u32 = 141;
const SECTION_POSITIONS: u32 = 264;
const POSITION_IDS: u32 = 265;
const TEXT: u32 = 269;
const IMAGE: u32 = 271;
const TABLE: u32 = 278;
const TABLE_ROW: u32 = 279;
const UNIT: u32 = 306;
const VALUE: u32 = 307;
const EM: u32 = 308;
const LINE_HEIGHT_UNIT: u32 = 310;
const DEFAULT: u32 = 351;
const BOLD: u32 = 361;
const ITALIC: u32 = 382;
const BOOK_NAVIGATION: u32 = 389;
const NAV_CONTAINER: u32 = 391;
const NAV_CONTAINERS: u32 = 392;
const NAV_UNIT: u32 = 393;
const INDEX: u32 = 403;
const CONTAINER_ID: u32 = 409;
const COMPRESSION: u32 = 410;
const DRM: u32 = 411;
const CHUNK_SIZE: u32 = 412;
const INDEX_OFFSET: u32 = 413;
const INDEX_LENGTH: u32 = 414;
const SYMBOLS_OFFSET: u32 = 415;
const SYMBOLS_LENGTH: u32 = 416;
const RAW_MEDIA: u32 = 417;
const CONTAINER_ENTITY_MAP: u32 = 419;
const WIDTH: u32 = 422;
const HEIGHT: u32 = 423;
const BOOK_METADATA: u32 = 490;
const CATEGORISED_METADATA: u32 = 491;
const KEY: u32 = 492;
const CATEGORY: u32 = 495;
const DOCUMENT_DATA: u32 = 538;
const LOCATION_MAP: u32 = 550;
const ALT_TEXT: u32 = 584;
const CONTENT_FEATURES: u32 = 585;
const NAMESPACE: u32 = 586;
const MAJOR: u32 = 587;
const MINOR: u32 = 588;
const VERSION_INFO: u32 = 589;
const FEATURES: u32 = 590;
const FORMAT_CAPABILITIES: u32 = 593;
const CAPABILITIES_OFFSET: u32 = 594;
const CAPABILITIES_LENGTH: u32 = 595;
const TOC: u32 = 212;
const LTR: u32 = 376;
// Ion's own symbols.
const ION_SYMBOL_TABLE: u32 = 3;
const NAME: u32 = 4;
const VERSION: u32 = 5;
const IMPORTS: u32 = 6;
const SYMBOLS: u32 = 7;
const MAX_ID: u32 = 8;

fn invalid(reason: impl Into<String>) -> KindleError {
    KindleError::Invalid {
        format: KindleFormat::Kfx,
        reason: reason.into(),
    }
}

/// The book's local symbols, numbered after `YJ_symbols`.
#[derive(Default)]
struct Symbols {
    names: Vec<String>,
    ids: HashMap<String, u32>,
}

impl Symbols {
    fn id(&mut self, name: &str) -> u32 {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = YJ_LAST + 1 + u32::try_from(self.names.len()).unwrap_or(u32::MAX - YJ_LAST - 1);
        self.names.push(name.to_owned());
        self.ids.insert(name.to_owned(), id);
        id
    }

    fn max_id(&self) -> u32 {
        YJ_LAST + u32::try_from(self.names.len()).unwrap_or(0)
    }
}

/// How a run of text looks.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Look {
    Body,
    Heading(u8),
    Italic,
    Bold,
    Fixed,
}

impl Look {
    fn name(self) -> String {
        match self {
            Self::Body => "s_body".to_owned(),
            Self::Heading(level) => format!("s_h{level}"),
            Self::Italic => "s_italic".to_owned(),
            Self::Bold => "s_bold".to_owned(),
            Self::Fixed => "s_fixed".to_owned(),
        }
    }

    /// Its properties: sizes in `em`, no font named but the generic fixed-width family, so the
    /// reader's choices apply.
    fn properties(self) -> Vec<(u32, Value)> {
        let decimal = |coefficient, exponent| Value::Decimal {
            coefficient,
            exponent,
        };
        match self {
            Self::Body => vec![(
                LINE_HEIGHT,
                Value::Struct(vec![
                    (VALUE, decimal(1, 0)),
                    (UNIT, Value::Symbol(LINE_HEIGHT_UNIT)),
                ]),
            )],
            Self::Heading(level) => {
                let size = match level {
                    1 => 16,
                    2 => 14,
                    3 => 12,
                    _ => 11,
                };
                vec![
                    (FONT_WEIGHT, Value::Symbol(BOLD)),
                    (
                        FONT_SIZE,
                        Value::Struct(vec![(VALUE, decimal(size, -1)), (UNIT, Value::Symbol(EM))]),
                    ),
                ]
            }
            Self::Italic => vec![(FONT_STYLE, Value::Symbol(ITALIC))],
            Self::Bold => vec![(FONT_WEIGHT, Value::Symbol(BOLD))],
            Self::Fixed => vec![(FONT_FAMILY, Value::String("monospace".to_owned()))],
        }
    }
}

/// What a run of a text block does: looks some way, or leads somewhere.
#[derive(Clone, Debug)]
enum Run {
    Look(Look),
    Link(Link),
}

/// A run, in characters of its block's text.
#[derive(Clone, Debug)]
struct Event {
    start: usize,
    length: usize,
    run: Run,
}

/// A text block or table cell: its id, its characters and its runs.
#[derive(Clone, Debug, Default)]
struct Text {
    eid: i64,
    text: String,
    characters: usize,
    events: Vec<Event>,
}

/// A table row: its id, and each cell with its column and row spans.
type TableRow = (i64, Vec<(Text, u32, u32)>);

/// Where a target leads, from a chapter: a block's id and a character offset.
type Placer<'a> = &'a dyn Fn(&Target, usize) -> Option<(i64, usize)>;

enum Unit {
    Text { look: Look, text: Text },
    Image { eid: i64, asset: usize, alt: String },
    Table { eid: i64, rows: Vec<TableRow> },
}

/// A chapter flattened into units, with where its anchors are.
struct Flattener<'a> {
    book: &'a KindleBook,
    next_eid: i64,
    units: Vec<Unit>,
    anchors: HashMap<String, (i64, usize)>,
}

impl Flattener<'_> {
    fn eid(&mut self) -> i64 {
        let eid = self.next_eid;
        self.next_eid += 1;
        eid
    }

    fn blocks(&mut self, blocks: &[Block], prefix: &str) {
        for block in blocks {
            self.block(block, prefix);
        }
    }

    /// A text block from `content`, `prefix` before it, its anchor at its start.
    fn text_unit(&mut self, look: Look, anchor: Option<&str>, prefix: &str, content: &[Inline]) {
        let eid = self.eid();
        if let Some(anchor) = anchor {
            self.anchors.insert(anchor.to_owned(), (eid, 0));
        }
        let mut text = Text {
            eid,
            ..Text::default()
        };
        push_text(&mut text, prefix);
        self.inlines(&mut text, content);
        self.units.push(Unit::Text { look, text });
    }

    fn block(&mut self, block: &Block, prefix: &str) {
        match block {
            Block::Heading {
                level,
                anchor,
                content,
            } => self.text_unit(Look::Heading(*level), anchor.as_deref(), prefix, content),
            Block::Paragraph {
                anchor,
                class,
                content,
            } => {
                let look = match class {
                    ParagraphClass::Normal => Look::Body,
                    ParagraphClass::Caption | ParagraphClass::Attribution => Look::Italic,
                    ParagraphClass::Label => Look::Bold,
                };
                self.text_unit(look, anchor.as_deref(), prefix, content);
            }
            Block::Preformatted { anchor, text, code } => {
                let content = [Inline::Text(text.clone())];
                let look = if *code { Look::Fixed } else { Look::Body };
                self.text_unit(look, anchor.as_deref(), prefix, &content);
            }
            Block::List {
                anchor,
                ordered,
                items,
            } => {
                if let Some(anchor) = anchor {
                    self.anchors.insert(anchor.clone(), (self.next_eid, 0));
                }
                for (number, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{prefix}{}. ", number + 1)
                    } else {
                        format!("{prefix}• ")
                    };
                    let nested = format!("{prefix}    ");
                    match item.split_first() {
                        Some((
                            Block::Paragraph {
                                anchor, content, ..
                            },
                            rest,
                        )) => {
                            self.text_unit(Look::Body, anchor.as_deref(), &marker, content);
                            self.blocks(rest, &nested);
                        }
                        _ => {
                            self.text_unit(Look::Body, None, &marker, &[]);
                            self.blocks(item, &nested);
                        }
                    }
                }
            }
            Block::Definitions { anchor, items } => {
                if let Some(anchor) = anchor {
                    self.anchors.insert(anchor.clone(), (self.next_eid, 0));
                }
                for (term, description) in items {
                    self.text_unit(Look::Bold, None, prefix, term);
                    self.blocks(description, &format!("{prefix}    "));
                }
            }
            Block::Quote { anchor, blocks } | Block::Aside { anchor, blocks } => {
                if let Some(anchor) = anchor {
                    self.anchors.insert(anchor.clone(), (self.next_eid, 0));
                }
                self.blocks(blocks, prefix);
            }
            Block::Table { anchor, rows } => {
                let eid = self.eid();
                if let Some(anchor) = anchor {
                    self.anchors.insert(anchor.clone(), (eid, 0));
                }
                let rows = rows
                    .iter()
                    .map(|row| {
                        let row_eid = self.eid();
                        let cells = row
                            .cells
                            .iter()
                            .map(|cell| (self.cell(cell, row.header), cell.colspan, cell.rowspan))
                            .collect();
                        (row_eid, cells)
                    })
                    .collect();
                self.units.push(Unit::Table { eid, rows });
            }
            Block::Figure { anchor, asset, alt } => {
                let eid = self.eid();
                if let Some(anchor) = anchor {
                    self.anchors.insert(anchor.clone(), (eid, 0));
                }
                if self.book.assets.get(*asset).is_some() {
                    self.units.push(Unit::Image {
                        eid,
                        asset: *asset,
                        alt: alt.clone(),
                    });
                } else {
                    let mut text = Text {
                        eid,
                        ..Text::default()
                    };
                    push_text(&mut text, alt);
                    self.units.push(Unit::Text {
                        look: Look::Italic,
                        text,
                    });
                }
            }
            Block::Rule => self.text_unit(Look::Body, None, "", &[Inline::Text("* * *".into())]),
            // The reader lays out pages; a reflowable book has no forced break.
            Block::PageBreak => {}
            Block::Notes(notes) => {
                for note in notes {
                    self.note(note);
                }
            }
        }
    }

    /// A cell's blocks as one run of text, a line each.
    fn cell(&mut self, cell: &Cell, header: bool) -> Text {
        let eid = self.eid();
        let mut text = Text {
            eid,
            ..Text::default()
        };
        let mut inner = Flattener {
            book: self.book,
            next_eid: self.next_eid,
            units: Vec::new(),
            anchors: HashMap::new(),
        };
        inner.blocks(&cell.blocks, "");
        self.next_eid = inner.next_eid;
        for (anchor, _) in inner.anchors {
            self.anchors.insert(anchor, (eid, 0));
        }
        for unit in inner.units {
            // A picture in a cell is its description; a table in a cell was made lines of
            // text when the book was built.
            let (look, part) = match unit {
                Unit::Text { look, text } => (look, text),
                Unit::Image { alt, .. } => {
                    let mut part = Text::default();
                    push_text(&mut part, &alt);
                    (Look::Italic, part)
                }
                Unit::Table { .. } => continue,
            };
            if !text.text.is_empty() {
                push_text(&mut text, "\n");
            }
            let shift = text.characters;
            if look != Look::Body {
                text.events.push(Event {
                    start: shift,
                    length: part.characters,
                    run: Run::Look(look),
                });
            }
            for event in part.events {
                text.events.push(Event {
                    start: event.start + shift,
                    ..event
                });
            }
            text.text.push_str(&part.text);
            text.characters += part.characters;
        }
        if header && text.characters > 0 {
            text.events.insert(
                0,
                Event {
                    start: 0,
                    length: text.characters,
                    run: Run::Look(Look::Bold),
                },
            );
        }
        text
    }

    fn note(&mut self, note: &Note) {
        let eid = self.eid();
        self.anchors.insert(note.anchor.clone(), (eid, 0));
        let mut text = Text {
            eid,
            ..Text::default()
        };
        let number = note.number.to_string();
        match &note.back {
            Some(back) => {
                let start = text.characters;
                push_text(&mut text, &number);
                text.events.push(Event {
                    start,
                    length: number.chars().count(),
                    run: Run::Link(Link::Internal(Target {
                        chapter: usize::MAX,
                        anchor: Some(back.clone()),
                    })),
                });
            }
            None => push_text(&mut text, &number),
        }
        push_text(&mut text, ". ");
        self.inlines(&mut text, &note.content);
        self.units.push(Unit::Text {
            look: Look::Body,
            text,
        });
    }

    fn inlines(&mut self, text: &mut Text, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(text, inline);
        }
    }

    fn spanned(&mut self, text: &mut Text, run: Run, content: &[Inline]) {
        let start = text.characters;
        let at = text.events.len();
        self.inlines(text, content);
        let length = text.characters - start;
        if length > 0 {
            // Before the runs inside it, so they are applied over it.
            text.events.insert(at, Event { start, length, run });
        }
    }

    fn inline(&mut self, text: &mut Text, inline: &Inline) {
        match inline {
            Inline::Text(value) => push_text(text, value),
            Inline::Strong(content) | Inline::Mark(content) => {
                self.spanned(text, Run::Look(Look::Bold), content);
            }
            Inline::Emphasis(content) => self.spanned(text, Run::Look(Look::Italic), content),
            Inline::Code(content) => self.spanned(text, Run::Look(Look::Fixed), content),
            Inline::Superscript(content) | Inline::Subscript(content) => {
                self.inlines(text, content);
            }
            Inline::Link { target, content } => {
                self.spanned(text, Run::Link(target.clone()), content);
            }
            Inline::Anchor(anchor) => {
                self.anchors
                    .insert(anchor.clone(), (text.eid, text.characters));
            }
            Inline::NoteRef {
                number,
                note,
                anchor,
            } => {
                if let Some(anchor) = anchor {
                    self.anchors
                        .insert(anchor.clone(), (text.eid, text.characters));
                }
                let mark = format!("[{number}]");
                let start = text.characters;
                push_text(text, &mark);
                text.events.push(Event {
                    start,
                    length: mark.chars().count(),
                    run: Run::Link(Link::Internal(Target {
                        chapter: usize::MAX,
                        anchor: Some(note.clone()),
                    })),
                });
            }
            Inline::Image { alt, .. } => push_text(text, alt),
            Inline::LineBreak => push_text(text, "\n"),
        }
    }
}

fn push_text(text: &mut Text, value: &str) {
    // Characters XML and Ion text readers refuse become the replacement character.
    for character in value.chars() {
        let character = match character {
            '\n' | '\t' => character,
            c if (c as u32) < 0x20 => '\u{FFFD}',
            c => c,
        };
        text.text.push(character);
        text.characters += 1;
    }
}

/// Book-wide identifiers derived from the book's identifier: a container id, an ASIN-like
/// content id and a book id, as `ebook-rs` forms them.
fn identifiers(identifier: &str) -> (String, String, String) {
    let digest = |domain: &[u8]| {
        let mut hash = Sha256::new();
        hash.update(domain);
        hash.update(identifier.as_bytes());
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
    };
    let first = digest(b"adocfoundry-kfx-a\0");
    let second = digest(b"adocfoundry-kfx-b\0");
    (
        format!("CR!{}", &first[..28]),
        format!("B0{}", &second[..30]),
        format!("adocfoundry_{}", second[30..54].to_ascii_lowercase()),
    )
}

/// An entity: its id, type and body.
struct Entity {
    id: u32,
    kind: u32,
    body: Vec<u8>,
}

fn ion_entity(id: u32, kind: u32, value: Value) -> Entity {
    Entity {
        id,
        kind,
        body: ion::stream(&[value]),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn write(book: &KindleBook) -> Result<Vec<u8>, KindleError> {
    let mut symbols = Symbols::default();
    let mut next_eid = FIRST_EID;

    // Every chapter flattened, its page template's id first.
    struct Flat {
        page: i64,
        units: Vec<Unit>,
        first: Option<i64>,
    }
    let mut chapters = Vec::with_capacity(book.chapters.len());
    let mut anchors: Vec<HashMap<String, (i64, usize)>> = Vec::new();
    for chapter in &book.chapters {
        let page = next_eid;
        next_eid += 1;
        let mut flattener = Flattener {
            book,
            next_eid,
            units: Vec::new(),
            anchors: HashMap::new(),
        };
        flattener.blocks(&chapter.blocks, "");
        next_eid = flattener.next_eid;
        let first = flattener.units.first().map(|unit| match unit {
            Unit::Text { text, .. } => text.eid,
            Unit::Image { eid, .. } | Unit::Table { eid, .. } => *eid,
        });
        anchors.push(flattener.anchors);
        chapters.push(Flat {
            page,
            units: flattener.units,
            first,
        });
    }

    // Where a target leads: a block's id and a character offset.
    let place = |target: &Target, from: usize| -> Option<(i64, usize)> {
        // A note and its mark are in the chapter they were written in.
        let chapter = if target.chapter == usize::MAX {
            from
        } else {
            target.chapter
        };
        match &target.anchor {
            None => {
                let flat = chapters.get(chapter)?;
                Some((flat.first.unwrap_or(flat.page), 0))
            }
            Some(anchor) => anchors.get(chapter)?.get(anchor).copied(),
        }
    };

    // Section and storyline names, then the pictures'.
    let names: Vec<(u32, u32)> = (0..chapters.len())
        .map(|index| {
            (
                symbols.id(&format!("c{index}")),
                symbols.id(&format!("l{index}")),
            )
        })
        .collect();
    let pictures: Vec<(u32, u32, String)> = (0..book.assets.len())
        .map(|index| {
            let location = format!("resource/rsrc{index}");
            (
                symbols.id(&format!("e{index}")),
                symbols.id(&location),
                location,
            )
        })
        .collect();

    // Anchors, one per place or address.
    let mut anchor_entities: Vec<(u32, Value)> = Vec::new();
    let mut anchor_by_place: HashMap<(i64, usize), u32> = HashMap::new();
    let mut anchor_by_uri: HashMap<String, u32> = HashMap::new();
    let mut anchor_for =
        |symbols: &mut Symbols, link: &Link, from: usize| -> Result<u32, KindleError> {
            match link {
                Link::External(uri) => {
                    if let Some(found) = anchor_by_uri.get(uri) {
                        return Ok(*found);
                    }
                    let name = symbols.id(&format!("a{}", anchor_entities.len()));
                    anchor_entities.push((
                        name,
                        Value::Struct(vec![
                            (ANCHOR_NAME, Value::Symbol(name)),
                            (URI, Value::String(uri.clone())),
                        ]),
                    ));
                    anchor_by_uri.insert(uri.clone(), name);
                    Ok(name)
                }
                Link::Internal(target) => {
                    let (eid, offset) = place(target, from).ok_or_else(|| {
                        invalid(format!(
                            "a link in chapter {} leads to an anchor the book does not have",
                            from + 1
                        ))
                    })?;
                    if let Some(found) = anchor_by_place.get(&(eid, offset)) {
                        return Ok(*found);
                    }
                    let name = symbols.id(&format!("a{}", anchor_entities.len()));
                    anchor_entities.push((
                        name,
                        Value::Struct(vec![
                            (ANCHOR_NAME, Value::Symbol(name)),
                            (
                                POSITION,
                                Value::Struct(vec![(ID, Value::Int(eid)), (OFFSET, int(offset))]),
                            ),
                        ]),
                    ));
                    anchor_by_place.insert((eid, offset), name);
                    Ok(name)
                }
            }
        };

    // Storylines, sections, content and the position maps.
    let mut looks: BTreeMap<Look, u32> = BTreeMap::new();
    let mut look_of = |symbols: &mut Symbols, look: Look| -> u32 {
        *looks
            .entry(look)
            .or_insert_with(|| symbols.id(&look.name()))
    };
    let mut content: Vec<(u32, Vec<String>)> = Vec::new();
    let mut content_bytes = 0usize;
    let mut add_content = |symbols: &mut Symbols, text: &str| -> (u32, i64) {
        let full = content
            .last()
            .is_none_or(|(_, strings)| !strings.is_empty() && content_bytes >= CONTENT_SIZE);
        if full {
            let name = symbols.id(&format!("content_{}", content.len()));
            content.push((name, Vec::new()));
            content_bytes = 0;
        }
        let (name, strings) = content
            .last_mut()
            .expect("a content fragment was just made");
        strings.push(text.to_owned());
        content_bytes += text.len();
        (*name, i64::try_from(strings.len() - 1).unwrap_or(i64::MAX))
    };
    let mut events_of = |symbols: &mut Symbols,
                         text: &Text,
                         from: usize,
                         looks: &mut dyn FnMut(&mut Symbols, Look) -> u32|
     -> Result<Vec<Value>, KindleError> {
        let mut out = Vec::new();
        for event in &text.events {
            let mut fields = vec![(OFFSET, int(event.start)), (LENGTH, int(event.length))];
            match &event.run {
                Run::Look(look) => fields.push((STYLE, Value::Symbol(looks(symbols, *look)))),
                Run::Link(link) => {
                    fields.push((LINK_TO, Value::Symbol(anchor_for(symbols, link, from)?)));
                }
            }
            out.push(Value::Struct(fields));
        }
        Ok(out)
    };
    let mut sections = Vec::new();
    let mut storylines = Vec::new();
    let mut section_positions = Vec::new();
    let mut position_ids = Vec::new();
    let mut locations = Vec::new();
    let mut pid: i64 = 0;
    for (index, flat) in chapters.iter().enumerate() {
        let (section, story) = names[index];
        let mut contains = vec![Value::Int(flat.page)];
        position_ids.push(Value::Struct(vec![
            (PID, Value::Int(pid)),
            (EID, Value::Int(flat.page)),
        ]));
        pid += 1;
        let mut place_block = |eid: i64, characters: usize, contains: &mut Vec<Value>| {
            contains.push(Value::Int(eid));
            position_ids.push(Value::Struct(vec![
                (PID, Value::Int(pid)),
                (EID, Value::Int(eid)),
            ]));
            locations.push(Value::Struct(vec![
                (ID, Value::Int(eid)),
                (OFFSET, Value::Int(0)),
            ]));
            pid += i64::try_from(characters.max(1)).unwrap_or(i64::MAX);
        };
        let mut blocks = Vec::with_capacity(flat.units.len());
        for unit in &flat.units {
            match unit {
                Unit::Text { look, text } => {
                    place_block(text.eid, text.characters, &mut contains);
                    let style = look_of(&mut symbols, *look);
                    let (name, at) = add_content(&mut symbols, &text.text);
                    let events = events_of(&mut symbols, text, index, &mut look_of)?;
                    let mut fields = vec![
                        (ID, Value::Int(text.eid)),
                        (STYLE, Value::Symbol(style)),
                        (TYPE, Value::Symbol(TEXT)),
                        (
                            CONTENT,
                            Value::Struct(vec![
                                (NAME, Value::Symbol(name)),
                                (INDEX, Value::Int(at)),
                            ]),
                        ),
                    ];
                    if !events.is_empty() {
                        fields.push((STYLE_EVENTS, Value::List(events)));
                    }
                    blocks.push(Value::Struct(fields));
                }
                Unit::Image { eid, asset, alt } => {
                    place_block(*eid, 1, &mut contains);
                    let mut fields = vec![
                        (ID, Value::Int(*eid)),
                        (TYPE, Value::Symbol(IMAGE)),
                        (RESOURCE_NAME, Value::Symbol(pictures[*asset].0)),
                    ];
                    if !alt.is_empty() {
                        fields.push((ALT_TEXT, Value::String(alt.clone())));
                    }
                    blocks.push(Value::Struct(fields));
                }
                Unit::Table { eid, rows } => {
                    place_block(*eid, 1, &mut contains);
                    let mut row_values = Vec::with_capacity(rows.len());
                    for (row_eid, cells) in rows {
                        place_block(*row_eid, 1, &mut contains);
                        let mut cell_values = Vec::with_capacity(cells.len());
                        for (cell, colspan, rowspan) in cells {
                            place_block(cell.eid, cell.characters, &mut contains);
                            let style = look_of(&mut symbols, Look::Body);
                            let mut fields = vec![
                                (ID, Value::Int(cell.eid)),
                                (STYLE, Value::Symbol(style)),
                                (TYPE, Value::Symbol(TEXT)),
                            ];
                            if *colspan > 1 {
                                fields.push((COLUMN_SPAN, Value::Int(i64::from(*colspan))));
                            }
                            if *rowspan > 1 {
                                fields.push((ROW_SPAN, Value::Int(i64::from(*rowspan))));
                            }
                            if !cell.text.is_empty() {
                                let (name, at) = add_content(&mut symbols, &cell.text);
                                fields.push((
                                    CONTENT,
                                    Value::Struct(vec![
                                        (NAME, Value::Symbol(name)),
                                        (INDEX, Value::Int(at)),
                                    ]),
                                ));
                                let events = events_of(&mut symbols, cell, index, &mut look_of)?;
                                if !events.is_empty() {
                                    fields.push((STYLE_EVENTS, Value::List(events)));
                                }
                            }
                            cell_values.push(Value::Struct(fields));
                        }
                        row_values.push(Value::Struct(vec![
                            (ID, Value::Int(*row_eid)),
                            (TYPE, Value::Symbol(TABLE_ROW)),
                            (CONTENT_LIST, Value::List(cell_values)),
                        ]));
                    }
                    blocks.push(Value::Struct(vec![
                        (ID, Value::Int(*eid)),
                        (TYPE, Value::Symbol(TABLE)),
                        (CONTENT_LIST, Value::List(row_values)),
                    ]));
                }
            }
        }
        section_positions.push(Value::Struct(vec![
            (CONTAINS, Value::List(contains)),
            (SECTION_NAME, Value::Symbol(section)),
        ]));
        sections.push((
            section,
            Value::Struct(vec![
                (SECTION_NAME, Value::Symbol(section)),
                (
                    PAGE_TEMPLATES,
                    Value::List(vec![Value::Struct(vec![
                        (ID, Value::Int(flat.page)),
                        (STORY_NAME, Value::Symbol(story)),
                        (TYPE, Value::Symbol(TEXT)),
                    ])]),
                ),
            ]),
        ));
        storylines.push((
            story,
            Value::Struct(vec![
                (STORY_NAME, Value::Symbol(story)),
                (CONTENT_LIST, Value::List(blocks)),
            ]),
        ));
    }
    position_ids.push(Value::Struct(vec![
        (PID, Value::Int(pid)),
        (EID, Value::Int(0)),
    ]));

    // The table of contents.
    fn nav_units(entries: &[TocEntry], place: Placer<'_>) -> Result<Vec<Value>, KindleError> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let (eid, offset) = place(&entry.target, entry.target.chapter)
                .ok_or_else(|| invalid("a table of contents entry leads nowhere"))?;
            let label = if entry.title.trim().is_empty() {
                "—".to_owned()
            } else {
                entry.title.clone()
            };
            let mut fields = vec![
                (
                    REPRESENTATION,
                    Value::Struct(vec![(LABEL, Value::String(label))]),
                ),
                (
                    TARGET_POSITION,
                    Value::Struct(vec![(ID, Value::Int(eid)), (OFFSET, int(offset))]),
                ),
            ];
            if !entry.children.is_empty() {
                fields.push((ENTRIES, Value::List(nav_units(&entry.children, place)?)));
            }
            out.push(Value::Annotated(
                vec![NAV_UNIT],
                Box::new(Value::Struct(fields)),
            ));
        }
        Ok(out)
    }
    let units = nav_units(&book.toc, &place)?;
    let containers = if units.is_empty() {
        Vec::new()
    } else {
        let name = symbols.id("n0");
        vec![Value::Annotated(
            vec![NAV_CONTAINER],
            Box::new(Value::Struct(vec![
                (NAV_TYPE, Value::Symbol(TOC)),
                (NAV_CONTAINER_NAME, Value::Symbol(name)),
                (ENTRIES, Value::List(units)),
            ])),
        )]
    };

    // The entities, in `ebook-rs`'s order.
    let metadata = &book.metadata;
    let (container_id, asin, book_id) = identifiers(&metadata.identifier);
    let mut entities = Vec::new();
    let feature = |namespace: &str, key: &str| {
        Value::Struct(vec![
            (NAMESPACE, Value::String(namespace.to_owned())),
            (KEY, Value::String(key.to_owned())),
            (
                VERSION_INFO,
                Value::Struct(vec![(
                    VERSION,
                    Value::Struct(vec![(MAJOR, Value::Int(1)), (MINOR, Value::Int(0))]),
                )]),
            ),
        ])
    };
    entities.push(ion_entity(
        NO_ID,
        CONTENT_FEATURES,
        Value::Struct(vec![(
            FEATURES,
            Value::List(vec![
                feature("com.amazon.yjconversion", "reflow-style"),
                feature("SDK.Marker", "CanonicalFormat"),
            ]),
        )]),
    ));
    let pair = |key: &str, value: Value| {
        Value::Struct(vec![(KEY, Value::String(key.to_owned())), (VALUE, value)])
    };
    let text_pair = |key: &str, value: &str| pair(key, Value::String(value.to_owned()));
    let mut title_metadata = vec![
        text_pair("ASIN", &asin),
        text_pair("asset_id", &container_id),
        text_pair("author", &metadata.authors.join("; ")),
        text_pair("book_id", &book_id),
        text_pair("cde_content_type", "PDOC"),
        text_pair("content_id", &asin),
        pair("is_sample", Value::Bool(false)),
        text_pair("language", &metadata.language),
        pair("override_kindle_font", Value::Bool(false)),
        text_pair("title", &metadata.title),
    ];
    if let Some(cover) = book.cover.filter(|cover| *cover < pictures.len()) {
        title_metadata.push(text_pair("cover_image", &format!("e{cover}")));
    }
    if let Some(publisher) = &metadata.publisher {
        title_metadata.push(text_pair("publisher", publisher));
    }
    if let Some(description) = &metadata.description {
        title_metadata.push(text_pair("description", description));
    }
    let category = |name: &str, pairs: Vec<Value>| {
        Value::Struct(vec![
            (CATEGORY, Value::String(name.to_owned())),
            (METADATA, Value::List(pairs)),
        ])
    };
    entities.push(ion_entity(
        NO_ID,
        BOOK_METADATA,
        Value::Struct(vec![(
            CATEGORISED_METADATA,
            Value::List(vec![
                category(
                    "kindle_audit_metadata",
                    vec![
                        text_pair("file_creator", "AdocFoundry"),
                        text_pair("creator_version", env!("CARGO_PKG_VERSION")),
                    ],
                ),
                category("kindle_title_metadata", title_metadata),
                category(
                    "kindle_ebook_metadata",
                    vec![
                        text_pair("selection", "enabled"),
                        text_pair("nested_span", "enabled"),
                    ],
                ),
                category("kindle_capability_metadata", Vec::new()),
            ]),
        )]),
    ));
    let reading_orders = Value::List(vec![Value::Struct(vec![
        (READING_ORDER_NAME, Value::Symbol(DEFAULT)),
        (
            SECTIONS,
            Value::List(
                names
                    .iter()
                    .map(|(section, _)| Value::Symbol(*section))
                    .collect(),
            ),
        ),
    ])]);
    entities.push(ion_entity(
        NO_ID,
        METADATA,
        Value::Struct(vec![(READING_ORDERS, reading_orders.clone())]),
    ));
    let max_id = symbols.max_id();
    entities.push(ion_entity(
        NO_ID,
        DOCUMENT_DATA,
        Value::Struct(vec![
            (560, Value::Symbol(557)),
            (192, Value::Symbol(LTR)),
            (112, Value::Symbol(383)),
            (
                FONT_SIZE,
                Value::Struct(vec![
                    (
                        VALUE,
                        Value::Decimal {
                            coefficient: 1,
                            exponent: 0,
                        },
                    ),
                    (UNIT, Value::Symbol(EM)),
                ]),
            ),
            (436, Value::Symbol(441)),
            (MAX_ID, Value::Int(i64::from(max_id))),
            (
                LINE_HEIGHT,
                Value::Struct(vec![
                    (
                        VALUE,
                        Value::Decimal {
                            coefficient: 12,
                            exponent: -1,
                        },
                    ),
                    (UNIT, Value::Symbol(EM)),
                ]),
            ),
            (477, Value::Symbol(56)),
            (READING_ORDERS, reading_orders),
        ]),
    ));
    entities.push(ion_entity(
        NO_ID,
        BOOK_NAVIGATION,
        Value::List(vec![Value::Struct(vec![
            (READING_ORDER_NAME, Value::Symbol(DEFAULT)),
            (NAV_CONTAINERS, Value::List(containers)),
        ])]),
    ));
    for (name, value) in sections {
        entities.push(ion_entity(name, SECTION, value));
    }
    for (name, value) in storylines {
        entities.push(ion_entity(name, STORYLINE, value));
    }
    for (name, strings) in &content {
        entities.push(ion_entity(
            *name,
            CONTENT,
            Value::Struct(vec![
                (NAME, Value::Symbol(*name)),
                (
                    CONTENT_LIST,
                    Value::List(strings.iter().cloned().map(Value::String).collect()),
                ),
            ]),
        ));
    }
    for (asset, (name, raw, location)) in book.assets.iter().zip(&pictures) {
        let format = match asset.kind {
            crate::AssetKind::Png => 284,
            crate::AssetKind::Jpeg => 285,
            crate::AssetKind::Gif => 286,
        };
        let mut fields = vec![
            (FORMAT, Value::Symbol(format)),
            (MIME, Value::String(asset.kind.media_type().to_owned())),
            (LOCATION, Value::String(location.clone())),
            (RESOURCE_NAME, Value::Symbol(*name)),
        ];
        if asset.width > 0 && asset.height > 0 {
            fields.push((WIDTH, Value::Int(i64::from(asset.width))));
            fields.push((HEIGHT, Value::Int(i64::from(asset.height))));
        }
        entities.push(ion_entity(*name, EXTERNAL_RESOURCE, Value::Struct(fields)));
        entities.push(Entity {
            id: *raw,
            kind: RAW_MEDIA,
            body: asset.bytes.clone(),
        });
    }
    for (name, value) in &anchor_entities {
        entities.push(ion_entity(*name, ANCHOR, value.clone()));
    }
    for (look, name) in &looks {
        let mut fields = look.properties();
        fields.push((STYLE_NAME, Value::Symbol(*name)));
        entities.push(ion_entity(*name, STYLE, Value::Struct(fields)));
    }
    entities.push(ion_entity(
        NO_ID,
        SECTION_POSITIONS,
        Value::List(section_positions),
    ));
    entities.push(ion_entity(NO_ID, POSITION_IDS, Value::List(position_ids)));
    entities.push(ion_entity(
        NO_ID,
        LOCATION_MAP,
        Value::List(vec![Value::Struct(vec![(
            LOCATIONS,
            Value::List(locations),
        )])]),
    ));
    entities.push(ion_entity(
        NO_ID,
        395,
        Value::Struct(vec![(ENTRIES, Value::List(Vec::new()))]),
    ));
    // Every named entity, for the container's map.
    let named: Vec<Value> = entities
        .iter()
        .filter(|entity| entity.id != NO_ID)
        .map(|entity| Value::Symbol(entity.id))
        .collect();
    entities.push(ion_entity(
        NO_ID,
        CONTAINER_ENTITY_MAP,
        Value::Struct(vec![(
            CONTAINER_LIST,
            Value::List(vec![Value::Struct(vec![
                (ID, Value::String(container_id.clone())),
                (CONTAINS, Value::List(named)),
            ])]),
        )]),
    ));

    container(&entities, &symbols, &container_id)
}

fn int(value: usize) -> Value {
    Value::Int(i64::try_from(value).unwrap_or(i64::MAX))
}

/// The `CONT` container: its header, the entity index, the document's symbol table, the
/// format capabilities, the container's own description and `kfxgen`'s trailer, then the
/// entities.
fn container(
    entities: &[Entity],
    symbols: &Symbols,
    container_id: &str,
) -> Result<Vec<u8>, KindleError> {
    let too_large = |what: &str| KindleError::TooLarge {
        format: KindleFormat::Kfx,
        reason: what.to_owned(),
    };
    let mut payload = Vec::new();
    let mut index = Vec::with_capacity(entities.len() * 24);
    for entity in entities {
        let offset = payload.len() as u64;
        payload.extend_from_slice(&ENTITY_HEADER);
        payload.extend_from_slice(&entity.body);
        let length = payload.len() as u64 - offset;
        index.extend_from_slice(&entity.id.to_le_bytes());
        index.extend_from_slice(&entity.kind.to_le_bytes());
        index.extend_from_slice(&offset.to_le_bytes());
        index.extend_from_slice(&length.to_le_bytes());
    }
    let symbol_table = ion::stream(&[Value::Annotated(
        vec![ION_SYMBOL_TABLE],
        Box::new(Value::Struct(vec![
            (
                IMPORTS,
                Value::List(vec![Value::Struct(vec![
                    (NAME, Value::String("YJ_symbols".to_owned())),
                    (VERSION, Value::Int(10)),
                    (MAX_ID, Value::Int(i64::from(YJ_MAX_ID))),
                ])]),
            ),
            (
                SYMBOLS,
                Value::List(symbols.names.iter().cloned().map(Value::String).collect()),
            ),
        ])),
    )]);
    let capabilities = ion::stream(&[Value::Annotated(
        vec![FORMAT_CAPABILITIES],
        Box::new(Value::List(vec![Value::Struct(vec![
            (KEY, Value::String("kfxgen.textBlock".to_owned())),
            (VERSION, Value::Int(1)),
        ])])),
    )]);
    let index_offset = CONTAINER_HEADER;
    let symbols_offset = index_offset + index.len();
    let capabilities_offset = symbols_offset + symbol_table.len();
    let info_offset = capabilities_offset + capabilities.len();
    let info = ion::stream(&[Value::Struct(vec![
        (CONTAINER_ID, Value::String(container_id.to_owned())),
        (COMPRESSION, Value::Int(0)),
        (DRM, Value::Int(0)),
        (INDEX_OFFSET, int(index_offset)),
        (INDEX_LENGTH, int(index.len())),
        (SYMBOLS_OFFSET, int(symbols_offset)),
        (SYMBOLS_LENGTH, int(symbol_table.len())),
        (CHUNK_SIZE, Value::Int(4096)),
        (CAPABILITIES_OFFSET, int(capabilities_offset)),
        (CAPABILITIES_LENGTH, int(capabilities.len())),
    ])]);
    let digest = sha1_smol::Sha1::from(&payload).digest().to_string();
    let trailer = format!(
        "[{{key:\"kfxgen_package_version\",value:\"\"}},{{key:\"kfxgen_application_version\",value:\"AdocFoundry {}\"}},{{key:\"kfxgen_payload_sha1\",value:\"{digest}\"}},{{key:\"kfxgen_acr\",value:\"{container_id}\"}}]",
        env!("CARGO_PKG_VERSION")
    );
    let header_length = info_offset + info.len() + trailer.len();
    let field = |value: usize| u32::try_from(value).map_err(|_| too_large("the container"));
    let mut out = Vec::with_capacity(header_length + payload.len());
    out.extend_from_slice(b"CONT");
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&field(header_length)?.to_le_bytes());
    out.extend_from_slice(&field(info_offset)?.to_le_bytes());
    out.extend_from_slice(&field(info.len())?.to_le_bytes());
    out.extend_from_slice(&index);
    out.extend_from_slice(&symbol_table);
    out.extend_from_slice(&capabilities);
    out.extend_from_slice(&info);
    out.extend_from_slice(trailer.as_bytes());
    out.extend_from_slice(&payload);
    check(&out, entities.len())?;
    Ok(out)
}

/// Reads the container back: its header, and every one of the `entities` the index names
/// starting `ENTY` inside the file. A file that fails is never returned.
fn check(bytes: &[u8], entities: usize) -> Result<(), KindleError> {
    let le32 = |at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let le64 = |at: usize| -> Option<usize> {
        usize::try_from(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?)).ok()
    };
    if bytes.get(..4) != Some(b"CONT") || bytes.get(4..6) != Some(&2u16.to_le_bytes()) {
        return Err(invalid("no version 2 CONT signature"));
    }
    let header = le32(6).ok_or_else(|| invalid("no header length"))?;
    let payload = bytes
        .get(header..)
        .ok_or_else(|| invalid("the header runs past the file"))?;
    for entry in 0..entities {
        let at = CONTAINER_HEADER + 24 * entry;
        let offset = le64(at + 8).ok_or_else(|| invalid("an index entry cut short"))?;
        let length = le64(at + 16).ok_or_else(|| invalid("an index entry cut short"))?;
        let entity = offset
            .checked_add(length)
            .and_then(|end| payload.get(offset..end))
            .ok_or_else(|| invalid("an entity outside the file"))?;
        if !entity.starts_with(b"ENTY") {
            return Err(invalid("an entity without its ENTY header"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_symbols_follow_the_shared_table() {
        let mut symbols = Symbols::default();
        assert_eq!(symbols.id("c0"), 852);
        assert_eq!(symbols.id("l0"), 853);
        assert_eq!(symbols.id("c0"), 852);
        assert_eq!(symbols.max_id(), 853);
        assert_eq!(
            9 + YJ_MAX_ID,
            851,
            "the import ends where local symbols begin"
        );
    }

    #[test]
    fn identifiers_are_stable() {
        let (container, asin, book) = identifiers("urn:uuid:1");
        assert_eq!(
            (container.clone(), asin.clone(), book.clone()),
            identifiers("urn:uuid:1")
        );
        assert!(
            container.starts_with("CR!") && container.len() == 31,
            "{container}"
        );
        assert!(asin.starts_with("B0") && asin.len() == 32, "{asin}");
        assert_ne!(identifiers("urn:uuid:2").0, container);
    }
}
