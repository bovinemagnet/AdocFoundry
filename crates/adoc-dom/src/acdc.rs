//! The acdc-parser implementation of [`DocumentParser`].

use std::{
    collections::HashMap,
    io::Cursor,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use acdc_parser as acdc;
use adoc_core::SourceRange;

use crate::{model::*, DocumentParser, IncludeResolver, ParseInput};

/// Block styles that name a diagram for `adoc-diagrams`, as Asciidoctor Diagram does.
pub const DIAGRAM_STYLES: &[&str] = &[
    "mermaid",
    "plantuml",
    "graphviz",
    "dot",
    "ditaa",
    "d2",
    "svgbob",
    "nomnoml",
    "pikchr",
    "vega",
    "vegalite",
    "wavedrom",
    "bytefield",
    "erd",
    "blockdiag",
    "seqdiag",
    "actdiag",
    "nwdiag",
    "structurizr",
    "bpmn",
    "tikz",
    "a2s",
];

pub struct AcdcParser;

impl DocumentParser for AcdcParser {
    fn name(&self) -> &'static str {
        "acdc"
    }

    fn parse(&self, input: &ParseInput<'_>) -> Dom {
        let page = input.path.to_path_buf();
        catch_unwind(AssertUnwindSafe(|| parse(input))).unwrap_or_else(|_| Dom {
            blocks: vec![unsupported_page(&page, input.text)],
            warnings: vec![DomWarning {
                message: "the parser failed on this document".to_owned(),
                span: None,
            }],
            ..Dom::default()
        })
    }
}

pub(crate) fn unsupported_page(page: &Path, text: &str) -> Block {
    Block {
        meta: BlockMeta {
            span: SourceSpan {
                path: page.to_path_buf(),
                range: SourceRange::new(0, text.len()),
                line: 1,
                include_chain: Vec::new(),
            },
            ..BlockMeta::default()
        },
        kind: BlockKind::Unsupported {
            name: "document".to_owned(),
            source: text.to_owned(),
        },
    }
}

/// Requested include path → the file the resolver actually read and its text, so spans name
/// real files and end on character boundaries in them.
type ResolvedIncludes = Arc<Mutex<HashMap<PathBuf, (PathBuf, Arc<str>)>>>;

fn parse(input: &ParseInput<'_>) -> Dom {
    let page = input.path.to_path_buf();
    let base = page.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    // acdc joins targets onto an absolute base; resolvers are asked beside the page as it
    // was given, which may be relative.
    let absolute_base = lexical(&std::path::absolute(&base).unwrap_or_else(|_| base.clone()));
    let resolved: ResolvedIncludes = Arc::default();
    let loader = {
        let resolver: Arc<dyn IncludeResolver> = input.includes.clone();
        let resolved = resolved.clone();
        let base = base.clone();
        acdc::IncludeLoader::custom(move |target: &acdc::IncludeSourceTarget| {
            let acdc::IncludeSourceTarget::File(requested) = target else {
                return Err(acdc::IncludeSourceError::new(
                    acdc::IncludeSourceErrorKind::NotFound,
                    "remote includes are not read",
                ));
            };
            let asked = match requested.strip_prefix(&absolute_base) {
                Ok(rest) if absolute_base != base => base.join(rest),
                _ => requested.clone(),
            };
            let asked = unprotect(&asked);
            match resolver.read(&asked) {
                Some((path, text)) => {
                    if let Ok(mut map) = resolved.lock() {
                        map.insert(requested.clone(), (path, Arc::from(text.as_str())));
                    }
                    Ok(acdc::IncludeSource::from_reader(Cursor::new(
                        unspace_include_attributes(&protect_resource_ids(&text)).into_bytes(),
                    )))
                }
                None => Err(acdc::IncludeSourceError::new(
                    acdc::IncludeSourceErrorKind::NotFound,
                    "include target not found",
                )),
            }
        })
    };
    let options = acdc::Options::builder()
        .with_safe_mode(acdc::SafeMode::Unsafe)
        .with_base_dir(&base)
        .with_include_loader(loader)
        .with_attributes(
            input
                .attributes
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .build();
    let options = match options {
        Ok(options) => options,
        Err(error) => return failed(&page, input.text, &error.to_string()),
    };
    let protected = unspace_include_attributes(&protect_resource_ids(input.text));
    let parsed = match acdc::parse(&protected, &options) {
        Ok(parsed) => parsed,
        Err(error) => return failed(&page, input.text, &error.to_string()),
    };
    let resolved = resolved.lock().map(|map| map.clone()).unwrap_or_default();
    let converter = Converter {
        parsed: &parsed,
        page,
        page_text: input.text,
        resolved,
        verbatim: std::cell::Cell::new(false),
    };
    converter.document()
}

/// Stands in for `:` inside Antora resource ids in include targets while acdc parses.
const PROTECTED_COLON: char = '\u{1}';

/// `text` with the colons of each include target that is a qualified Antora resource id
/// (`module:family$path`, `component:module:family$path`) replaced by [`PROTECTED_COLON`].
///
/// A stock processor reads `module:` as a URI scheme and links the include instead of reading
/// it; Antora resolves it as a resource. The placeholder is one byte, like the colon, so every
/// offset in the text is unchanged.
fn protect_resource_ids(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let target = line
            .strip_prefix("include::")
            .and_then(|rest| rest.split_once('['))
            .map(|(target, _)| target)
            .filter(|target| target.contains('$') && target.contains(':'));
        match target {
            Some(target) => {
                let start = "include::".len();
                out.push_str(&line[..start]);
                out.push_str(&target.replace(':', &PROTECTED_COLON.to_string()));
                out.push_str(&line[start + target.len()..]);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Stands in, at the end of an include target, for a space taken out of its attribute list.
const PROTECTED_SPACE: char = '\u{2}';

/// `text` with the spaces after the commas of each include's attribute list taken out, and as
/// many [`PROTECTED_SPACE`]s put at the end of its target: acdc rejects `[tag=a, leveloffset=+1]`,
/// which Asciidoctor reads. The line keeps its length, so only offsets inside it move.
fn unspace_include_attributes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let open = content
            .strip_prefix("include::")
            .and_then(|rest| rest.find('['))
            .map(|open| "include::".len() + open)
            .filter(|_| content.ends_with(']') && content.contains(", "));
        let Some(open) = open else {
            out.push_str(line);
            continue;
        };
        let mut list = String::with_capacity(content.len() - open);
        let (mut moved, mut quoted, mut after_comma) = (0, false, false);
        for character in content[open..].chars() {
            match character {
                ' ' if after_comma => {
                    moved += 1;
                    continue;
                }
                '"' => quoted = !quoted,
                _ => {}
            }
            after_comma = character == ',' && !quoted;
            list.push(character);
        }
        out.push_str(&content[..open]);
        out.extend(std::iter::repeat_n(PROTECTED_SPACE, moved));
        out.push_str(&list);
        out.push_str(&line[content.len()..]);
    }
    out
}

/// `path` with `.` and `..` resolved without touching the file system, as acdc resolves the
/// base it joins include targets onto.
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

/// A requested path with protected colons restored and protected spaces taken out.
fn unprotect(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text.contains([PROTECTED_COLON, PROTECTED_SPACE]) {
        PathBuf::from(
            text.replace(PROTECTED_COLON, ":")
                .replace(PROTECTED_SPACE, ""),
        )
    } else {
        path.to_path_buf()
    }
}

fn failed(page: &Path, text: &str, message: &str) -> Dom {
    Dom {
        blocks: vec![unsupported_page(page, text)],
        warnings: vec![DomWarning {
            message: message.to_owned(),
            span: None,
        }],
        ..Dom::default()
    }
}

struct Converter<'p> {
    parsed: &'p acdc::ParseResult,
    page: PathBuf,
    page_text: &'p str,
    resolved: HashMap<PathBuf, (PathBuf, Arc<str>)>,
    /// Set while converting verbatim content, which takes no replacements.
    verbatim: std::cell::Cell<bool>,
}

impl Converter<'_> {
    fn document(&self) -> Dom {
        let document = self.parsed.document();
        let title = document
            .header
            .as_ref()
            .filter(|header| !header.title.is_empty())
            .map(|header| {
                // acdc splits `Main: Sub` at its last `: `; the doctitle is the whole text, and a
                // title page partitions it again where it needs to.
                let mut title = self.inlines(&header.title);
                if let Some(subtitle) = &header.subtitle {
                    title.push(Inline::Text(": ".to_owned()));
                    title.extend(self.inlines(subtitle));
                }
                title
            });
        let attributes = document
            .attributes
            .iter()
            .filter_map(|(name, value)| {
                // A set attribute without a value (`:toc:`) is present with empty text.
                let value = match (value.text(), value.as_integer()) {
                    (Some(text), _) => text.to_owned(),
                    (None, Some(integer)) => integer.to_string(),
                    (None, None) => value.is_presence().then(String::new)?,
                };
                Some((name.to_owned(), value))
            })
            .collect();
        let warnings = self
            .parsed
            .warnings()
            .iter()
            .map(|warning| DomWarning {
                message: warning.kind.to_string(),
                span: warning
                    .location
                    .as_ref()
                    .map(|location| self.span_of_source_location(location)),
            })
            .collect();
        Dom {
            title,
            attributes,
            blocks: self.blocks(&document.blocks),
            warnings,
        }
    }

    // --- spans -----------------------------------------------------------------------------

    fn real_path(&self, file: Option<PathBuf>) -> PathBuf {
        match file {
            None => self.page.clone(),
            Some(file) => self
                .resolved
                .get(&file)
                .map_or(file, |(path, _)| path.clone()),
        }
    }

    fn span(&self, location: &acdc::Location) -> SourceSpan {
        let source = self.parsed.source_location(location);
        self.span_of_source_location(&source)
    }

    fn span_of_source_location(&self, source: &acdc::SourceLocation) -> SourceSpan {
        let location = &source.location;
        // acdc's end offset is the start of the last character; spans end after it.
        let text = match &source.file {
            None => Some(self.page_text),
            Some(file) => self.resolved.get(file).map(|(_, text)| &**text),
        };
        let last = location.absolute_end.max(location.absolute_start);
        let end = text
            .and_then(|text| text.get(last..))
            .and_then(|rest| rest.chars().next())
            .map_or(last, |character| last + character.len_utf8());
        SourceSpan {
            path: self.real_path(source.file.clone()),
            range: SourceRange::new(location.absolute_start, end),
            line: location.start.line,
            include_chain: location
                .start
                .file
                .as_ref()
                .map(|chain| {
                    chain
                        .iter()
                        .map(|file| {
                            file.replace(PROTECTED_COLON, ":")
                                .replace(PROTECTED_SPACE, "")
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn meta(
        &self,
        location: &acdc::Location,
        metadata: Option<&acdc::BlockMetadata<'_>>,
        title: &[acdc::InlineNode<'_>],
    ) -> BlockMeta {
        let mut meta = BlockMeta {
            span: self.span(location),
            title: self.inlines(title),
            ..BlockMeta::default()
        };
        if let Some(metadata) = metadata {
            meta.id = metadata.id.as_ref().map(|anchor| anchor.id.to_owned());
            meta.roles = metadata
                .roles
                .iter()
                .map(|role| (*role).to_owned())
                .collect();
            meta.style = metadata.style.map(str::to_owned);
            meta.attributes = metadata
                .attributes
                .iter()
                .filter_map(|(name, value)| match value {
                    acdc::AttributeValue::String(value) => {
                        Some((name.to_string(), value.to_string()))
                    }
                    acdc::AttributeValue::Bool(true) => Some((name.to_string(), String::new())),
                    _ => None,
                })
                .collect();
        }
        meta
    }

    /// The source text a location covers, in the page or an included file.
    fn source_text(&self, location: &acdc::Location) -> String {
        let source = self.parsed.source_location(location);
        let range = self.span_of_source_location(&source).range;
        let text = match &source.file {
            None => self.page_text,
            Some(file) => self.resolved.get(file).map_or("", |(_, text)| &**text),
        };
        text.get(range.start..range.end).unwrap_or("").to_owned()
    }

    // --- blocks ----------------------------------------------------------------------------

    fn blocks(&self, blocks: &[acdc::Block<'_>]) -> Vec<Block> {
        blocks
            .iter()
            .filter_map(|block| self.block(block))
            .collect()
    }

    fn block(&self, block: &acdc::Block<'_>) -> Option<Block> {
        Some(match block {
            acdc::Block::Section(section) => {
                let mut meta = self.meta(&section.location, Some(&section.metadata), &[]);
                meta.id = Some(section.id().into_owned());
                Block {
                    meta,
                    kind: BlockKind::Section {
                        level: section.level,
                        title: self.inlines(&section.title),
                        blocks: self.blocks(&section.content),
                    },
                }
            }
            acdc::Block::DiscreteHeader(heading) => Block {
                meta: self.meta(&heading.location, Some(&heading.metadata), &[]),
                kind: BlockKind::Heading {
                    level: heading.level,
                    title: self.inlines(&heading.title),
                },
            },
            acdc::Block::Paragraph(paragraph) => self.paragraph(paragraph),
            acdc::Block::Admonition(admonition) => Block {
                meta: self.meta(
                    &admonition.location,
                    Some(&admonition.metadata),
                    &admonition.title,
                ),
                kind: BlockKind::Admonition {
                    kind: match admonition.variant {
                        acdc::AdmonitionVariant::Note => AdmonitionKind::Note,
                        acdc::AdmonitionVariant::Tip => AdmonitionKind::Tip,
                        acdc::AdmonitionVariant::Important => AdmonitionKind::Important,
                        acdc::AdmonitionVariant::Caution => AdmonitionKind::Caution,
                        acdc::AdmonitionVariant::Warning => AdmonitionKind::Warning,
                    },
                    blocks: self.blocks(&admonition.blocks),
                },
            },
            acdc::Block::UnorderedList(list) => {
                let checklist = list.items.iter().any(|item| item.checked.is_some());
                Block {
                    meta: self.meta(&list.location, Some(&list.metadata), &list.title),
                    kind: BlockKind::List {
                        kind: if checklist {
                            ListKind::Checklist
                        } else {
                            ListKind::Unordered
                        },
                        items: self.list_items(&list.items),
                    },
                }
            }
            acdc::Block::OrderedList(list) => Block {
                meta: self.meta(&list.location, Some(&list.metadata), &list.title),
                kind: BlockKind::List {
                    kind: ListKind::Ordered,
                    items: self.list_items(&list.items),
                },
            },
            acdc::Block::DescriptionList(list) => Block {
                meta: self.meta(&list.location, Some(&list.metadata), &list.title),
                kind: BlockKind::DescriptionList {
                    items: list
                        .items
                        .iter()
                        .map(|item| DescriptionItem {
                            span: self.span(&item.location),
                            term: self.inlines(&item.term),
                            content: self.prose(&item.principal_text),
                            blocks: self.blocks(&item.description),
                        })
                        .collect(),
                },
            },
            acdc::Block::CalloutList(list) => Block {
                meta: self.meta(&list.location, Some(&list.metadata), &list.title),
                kind: BlockKind::CalloutList {
                    items: list
                        .items
                        .iter()
                        .map(|item| CalloutItem {
                            span: self.span(&item.location),
                            number: item.callout.number,
                            content: self.inlines(&item.principal),
                            blocks: self.blocks(&item.blocks),
                        })
                        .collect(),
                },
            },
            acdc::Block::DelimitedBlock(delimited)
                if matches!(
                    delimited.inner,
                    acdc::DelimitedBlockType::DelimitedComment(_)
                ) =>
            {
                return None;
            }
            acdc::Block::DelimitedBlock(delimited) => self.delimited(delimited),
            acdc::Block::Image(image) => Block {
                meta: self.meta(&image.location, Some(&image.metadata), &image.title),
                kind: image_kind(&image.source, &image.metadata),
            },
            acdc::Block::Audio(audio) => Block {
                meta: self.meta(&audio.location, Some(&audio.metadata), &audio.title),
                kind: BlockKind::Media {
                    kind: MediaKind::Audio,
                    targets: vec![audio.source.to_string()],
                },
            },
            acdc::Block::Video(video) => Block {
                meta: self.meta(&video.location, Some(&video.metadata), &video.title),
                kind: BlockKind::Media {
                    kind: MediaKind::Video,
                    targets: video.sources.iter().map(ToString::to_string).collect(),
                },
            },
            acdc::Block::ThematicBreak(rule) => Block {
                meta: self.meta(&rule.location, None, &rule.title),
                kind: BlockKind::ThematicBreak,
            },
            acdc::Block::PageBreak(page_break) => Block {
                meta: self.meta(
                    &page_break.location,
                    Some(&page_break.metadata),
                    &page_break.title,
                ),
                kind: BlockKind::PageBreak,
            },
            acdc::Block::TableOfContents(toc) => Block {
                meta: self.meta(&toc.location, Some(&toc.metadata), &[]),
                kind: BlockKind::Toc,
            },
            // Attribute entries and comments produce no output.
            acdc::Block::DocumentAttribute(_) | acdc::Block::Comment(_) => return None,
            other => Block {
                meta: self.meta(other.location(), None, &[]),
                kind: BlockKind::Unsupported {
                    name: "block".to_owned(),
                    source: self.source_text(other.location()),
                },
            },
        })
    }

    fn list_items(&self, items: &[acdc::ListItem<'_>]) -> Vec<ListItem> {
        items
            .iter()
            .map(|item| ListItem {
                span: self.span(&item.location),
                checked: item
                    .checked
                    .as_ref()
                    .map(|checked| matches!(checked, acdc::ListItemCheckedStatus::Checked)),
                content: self.prose(&item.principal),
                blocks: self.blocks(&item.blocks),
            })
            .collect()
    }

    /// A paragraph, or the block its style makes it: `[NOTE]`, `[source]`, `[quote]`, ….
    fn paragraph(&self, paragraph: &acdc::Paragraph<'_>) -> Block {
        let meta = self.meta(
            &paragraph.location,
            Some(&paragraph.metadata),
            &paragraph.title,
        );
        let style = paragraph.metadata.style.unwrap_or("");
        let kind = if let Some(kind) = AdmonitionKind::from_style(style) {
            BlockKind::Admonition {
                kind,
                blocks: vec![Block {
                    meta: BlockMeta {
                        span: meta.span.clone(),
                        ..BlockMeta::default()
                    },
                    kind: BlockKind::Paragraph {
                        inlines: self.prose(&paragraph.content),
                    },
                }],
            }
        } else if style == "source" || style == "listing" {
            let (text, callouts) = self.verbatim_text(&paragraph.content);
            BlockKind::Listing {
                language: source_language(&paragraph.metadata),
                text,
                callouts,
            }
        } else if style == "literal" {
            BlockKind::Literal {
                text: self.verbatim_text(&paragraph.content).0,
            }
        } else if style == "verse" {
            BlockKind::Verse {
                attribution: attribution(&paragraph.metadata),
                citation: citation(&paragraph.metadata),
                text: self.verbatim_text(&paragraph.content).0,
            }
        } else if style == "quote" {
            BlockKind::Quote {
                attribution: attribution(&paragraph.metadata),
                citation: citation(&paragraph.metadata),
                blocks: vec![Block {
                    meta: BlockMeta {
                        span: meta.span.clone(),
                        ..BlockMeta::default()
                    },
                    kind: BlockKind::Paragraph {
                        inlines: self.prose(&paragraph.content),
                    },
                }],
            }
        } else if DIAGRAM_STYLES.contains(&style) {
            BlockKind::Diagram {
                kind: style.to_owned(),
                source: self.verbatim_text(&paragraph.content).0,
            }
        } else if style == "stem" || style == "latexmath" || style == "asciimath" {
            BlockKind::Stem {
                latex: style == "latexmath",
                source: self.verbatim_text(&paragraph.content).0,
            }
        } else if style == "pass" {
            BlockKind::Passthrough {
                content: self.verbatim_text(&paragraph.content).0,
            }
        } else if let Some(image) = style
            .is_empty()
            .then(|| block_image_macro(&self.source_text(&paragraph.location)))
            .flatten()
        {
            image
        } else {
            BlockKind::Paragraph {
                inlines: self.prose(&paragraph.content),
            }
        };
        Block { meta, kind }
    }

    fn delimited(&self, delimited: &acdc::DelimitedBlock<'_>) -> Block {
        let meta = self.meta(
            &delimited.location,
            Some(&delimited.metadata),
            &delimited.title,
        );
        let style = delimited.metadata.style.unwrap_or("");
        let kind = match &delimited.inner {
            acdc::DelimitedBlockType::DelimitedListing(content)
            | acdc::DelimitedBlockType::DelimitedLiteral(content)
                if DIAGRAM_STYLES.contains(&style) =>
            {
                BlockKind::Diagram {
                    kind: style.to_owned(),
                    source: self.verbatim_text(content).0,
                }
            }
            acdc::DelimitedBlockType::DelimitedListing(content) => {
                let (text, callouts) = self.verbatim_text(content);
                BlockKind::Listing {
                    language: source_language(&delimited.metadata),
                    text,
                    callouts,
                }
            }
            acdc::DelimitedBlockType::DelimitedLiteral(content) => BlockKind::Literal {
                text: self.verbatim_text(content).0,
            },
            acdc::DelimitedBlockType::DelimitedExample(blocks) => {
                match AdmonitionKind::from_style(style) {
                    Some(kind) => BlockKind::Admonition {
                        kind,
                        blocks: self.blocks(blocks),
                    },
                    None => BlockKind::Example {
                        blocks: self.blocks(blocks),
                    },
                }
            }
            acdc::DelimitedBlockType::DelimitedOpen(blocks) => {
                match AdmonitionKind::from_style(style) {
                    Some(kind) => BlockKind::Admonition {
                        kind,
                        blocks: self.blocks(blocks),
                    },
                    None => BlockKind::Open {
                        blocks: self.blocks(blocks),
                    },
                }
            }
            acdc::DelimitedBlockType::DelimitedSidebar(blocks) => BlockKind::Sidebar {
                blocks: self.blocks(blocks),
            },
            acdc::DelimitedBlockType::DelimitedQuote(blocks) => BlockKind::Quote {
                attribution: attribution(&delimited.metadata),
                citation: citation(&delimited.metadata),
                blocks: self.blocks(blocks),
            },
            acdc::DelimitedBlockType::DelimitedVerse(content) => BlockKind::Verse {
                attribution: attribution(&delimited.metadata),
                citation: citation(&delimited.metadata),
                text: self.verbatim_text(content).0,
            },
            acdc::DelimitedBlockType::DelimitedPass(content) => BlockKind::Passthrough {
                content: self.verbatim_text(content).0,
            },
            acdc::DelimitedBlockType::DelimitedStem(stem) => BlockKind::Stem {
                latex: matches!(stem.notation, acdc::StemNotation::Latexmath),
                source: stem.content.to_owned(),
            },
            acdc::DelimitedBlockType::DelimitedTable(table) => self.table(table),
            _ => BlockKind::Unsupported {
                name: delimited.delimiter.to_owned(),
                source: self.source_text(&delimited.location),
            },
        };
        Block { meta, kind }
    }

    fn table(&self, table: &acdc::Table<'_>) -> BlockKind {
        let row = |row: &acdc::TableRow<'_>| Row {
            cells: row
                .columns
                .iter()
                .map(|cell| Cell {
                    blocks: self.blocks(&cell.content),
                    colspan: cell.colspan.max(1),
                    rowspan: cell.rowspan.max(1),
                    align: cell.halign.as_ref().map(align),
                    style: cell.style.as_ref().map(cell_style),
                })
                .collect(),
        };
        BlockKind::Table {
            columns: table
                .columns
                .iter()
                .map(|column| Column {
                    width: match column.width {
                        acdc::ColumnWidth::Proportional(width) => ColumnWidth::Proportional(width),
                        acdc::ColumnWidth::Percentage(width) => ColumnWidth::Percentage(width),
                        _ => ColumnWidth::Auto,
                    },
                    align: align(&column.halign),
                    style: cell_style(&column.style),
                })
                .collect(),
            header: table.header.as_ref().map(row),
            rows: table.rows.iter().map(row).collect(),
            footer: table.footer.as_ref().map(row),
        }
    }

    /// The text of a verbatim block, and its callout markers by line.
    fn verbatim_text(&self, nodes: &[acdc::InlineNode<'_>]) -> (String, Vec<Callout>) {
        let outer = self.verbatim.replace(true);
        let converted = self.verbatim_text_inner(nodes);
        self.verbatim.set(outer);
        converted
    }

    fn verbatim_text_inner(&self, nodes: &[acdc::InlineNode<'_>]) -> (String, Vec<Callout>) {
        let mut text = String::new();
        let mut callouts = Vec::new();
        for node in nodes {
            if let acdc::InlineNode::CalloutRef(callout) = node {
                // The marker ends its line; drop the space written before it and, as
                // Asciidoctor does, a line comment that only introduces it.
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
                    number: callout.number,
                });
            } else {
                text.push_str(&Inline::plain_text(&self.inline(node)));
            }
        }
        (text, callouts)
    }

    // --- inlines ---------------------------------------------------------------------------

    fn inlines(&self, nodes: &[acdc::InlineNode<'_>]) -> Vec<Inline> {
        let mut out: Vec<Inline> = Vec::new();
        for node in nodes {
            for inline in self.inline(node) {
                // Adjacent text runs merge, so renderers see words, not fragments.
                if let (Inline::Text(next), Some(Inline::Text(previous))) =
                    (&inline, out.last_mut())
                {
                    previous.push_str(next);
                } else {
                    out.push(inline);
                }
            }
        }
        out.into_iter().flat_map(split_inline_images).collect()
    }

    /// The text of a paragraph or list item: as [`Self::inlines`], less its line comments.
    fn prose(&self, nodes: &[acdc::InlineNode<'_>]) -> Vec<Inline> {
        drop_line_comments(self.inlines(nodes))
    }

    fn inline(&self, node: &acdc::InlineNode<'_>) -> Vec<Inline> {
        let wrap = |roles: Option<&str>, content: Vec<Inline>, make: fn(Vec<Inline>) -> Inline| {
            let inner = make(content);
            match roles {
                Some(role) if !role.is_empty() => vec![Inline::Span {
                    roles: role
                        .split('.')
                        .filter(|r| !r.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    content: vec![inner],
                }],
                _ => vec![inner],
            }
        };
        match node {
            acdc::InlineNode::PlainText(text) if text.escaped || self.verbatim.get() => {
                vec![Inline::Text(text.content.to_owned())]
            }
            acdc::InlineNode::PlainText(text) => {
                vec![Inline::Text(crate::replacements::replace(text.content))]
            }
            acdc::InlineNode::RawText(text) => vec![Inline::Text(text.content.to_owned())],
            acdc::InlineNode::VerbatimText(text) => vec![Inline::Text(text.content.to_owned())],
            acdc::InlineNode::BoldText(bold) => {
                wrap(bold.role, self.inlines(&bold.content), Inline::Bold)
            }
            acdc::InlineNode::ItalicText(italic) => {
                wrap(italic.role, self.inlines(&italic.content), Inline::Italic)
            }
            acdc::InlineNode::MonospaceText(mono) => {
                wrap(mono.role, self.inlines(&mono.content), Inline::Monospace)
            }
            acdc::InlineNode::HighlightText(highlight) => match highlight.role {
                // `[.role]#text#` is a span carrying the role, not a mark.
                Some(role) if !role.is_empty() => vec![Inline::Span {
                    roles: role
                        .split('.')
                        .filter(|r| !r.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    content: self.inlines(&highlight.content),
                }],
                _ => vec![Inline::Highlight(self.inlines(&highlight.content))],
            },
            acdc::InlineNode::SubscriptText(sub) => {
                wrap(sub.role, self.inlines(&sub.content), Inline::Subscript)
            }
            acdc::InlineNode::SuperscriptText(sup) => {
                wrap(sup.role, self.inlines(&sup.content), Inline::Superscript)
            }
            acdc::InlineNode::CurvedQuotationText(quoted) => {
                let mut out = vec![Inline::Text("\u{201c}".to_owned())];
                out.extend(self.inlines(&quoted.content));
                out.push(Inline::Text("\u{201d}".to_owned()));
                out
            }
            acdc::InlineNode::CurvedApostropheText(quoted) => {
                let mut out = vec![Inline::Text("\u{2018}".to_owned())];
                out.extend(self.inlines(&quoted.content));
                out.push(Inline::Text("\u{2019}".to_owned()));
                out
            }
            acdc::InlineNode::StandaloneCurvedApostrophe(_) => {
                vec![Inline::Text("\u{2019}".to_owned())]
            }
            acdc::InlineNode::LineBreak(_) => vec![Inline::LineBreak],
            acdc::InlineNode::InlineAnchor(anchor) => vec![Inline::Anchor {
                id: anchor.id.to_owned(),
            }],
            acdc::InlineNode::CalloutRef(callout) => {
                vec![Inline::Text(format!("({})", callout.number))]
            }
            acdc::InlineNode::Macro(inline_macro) => self.inline_macro(inline_macro),
            _ => Vec::new(),
        }
    }

    fn inline_macro(&self, inline_macro: &acdc::InlineMacro<'_>) -> Vec<Inline> {
        vec![match inline_macro {
            acdc::InlineMacro::Footnote(footnote) => Inline::Footnote {
                id: footnote.id.map(str::to_owned),
                number: footnote.number,
                content: self.inlines(&footnote.content),
            },
            acdc::InlineMacro::Icon(icon) => Inline::Icon {
                name: icon.target.to_string(),
            },
            acdc::InlineMacro::Image(image) => Inline::Image {
                target: image.source.to_string(),
                alt: image
                    .metadata
                    .attributes
                    .get_string("alt")
                    .map(|alt| alt.into_owned())
                    // acdc keeps an inline image's alt text as its title.
                    .or_else(|| Some(inline_text(&image.title)).filter(|alt| !alt.is_empty()))
                    .unwrap_or_default(),
            },
            acdc::InlineMacro::Keyboard(keyboard) => {
                Inline::Keyboard(keyboard.keys.iter().map(|key| (*key).to_owned()).collect())
            }
            acdc::InlineMacro::Button(button) => Inline::Button(button.label.to_owned()),
            acdc::InlineMacro::Menu(menu) => Inline::Menu(
                std::iter::once(menu.target)
                    .chain(menu.items.iter().copied())
                    .map(str::to_owned)
                    .collect(),
            ),
            acdc::InlineMacro::Url(url) => Inline::Link {
                target: url.target.to_string(),
                text: self.inlines(&url.text),
            },
            acdc::InlineMacro::Link(link) => Inline::Link {
                target: link.target.to_string(),
                text: self.inlines(&link.text),
            },
            acdc::InlineMacro::Mailto(mailto) => Inline::Link {
                target: format!("mailto:{}", mailto.target),
                text: self.inlines(&mailto.text),
            },
            acdc::InlineMacro::Autolink(autolink) => Inline::Link {
                target: autolink.url.to_string(),
                text: Vec::new(),
            },
            acdc::InlineMacro::CrossReference(xref) => Inline::Xref {
                target: xref.target.to_owned(),
                text: self.inlines(&xref.text),
                span: self.span(&xref.location),
            },
            acdc::InlineMacro::Pass(pass) => {
                Inline::Passthrough(pass.text.unwrap_or("").to_owned())
            }
            acdc::InlineMacro::Stem(stem) => Inline::Stem {
                latex: matches!(stem.notation, acdc::StemNotation::Latexmath),
                source: stem.content.to_owned(),
            },
            acdc::InlineMacro::IndexTerm(term) => {
                let primary = self.inlines(term.term());
                let mut terms = vec![Inline::plain_text(&primary)];
                for level in [term.secondary(), term.tertiary()].into_iter().flatten() {
                    terms.push(Inline::plain_text(&self.inlines(level)));
                }
                Inline::IndexTerm {
                    terms,
                    visible: if term.is_visible() {
                        primary
                    } else {
                        Vec::new()
                    },
                }
            }
            _ => return Vec::new(),
        }]
    }
}

fn image_kind(source: &acdc::Source<'_>, metadata: &acdc::BlockMetadata<'_>) -> BlockKind {
    let named = |name: &str| {
        metadata
            .attributes
            .get_string(name)
            .map(|value| value.into_owned())
    };
    BlockKind::Image {
        target: source.to_string(),
        alt: named("alt")
            .or_else(|| metadata.positional_attribute(0).map(str::to_owned))
            .unwrap_or_default(),
        width: named("width").or_else(|| metadata.positional_attribute(1).map(str::to_owned)),
        height: named("height").or_else(|| metadata.positional_attribute(2).map(str::to_owned)),
    }
}

/// A block image macro acdc left as a paragraph: it rejects targets containing `:` or `$`,
/// which every Antora resource id (`ROOT:image$x.svg`) can.
fn block_image_macro(source: &str) -> Option<BlockKind> {
    let line = source.trim_end();
    let rest = line.strip_prefix("image::")?;
    if line.contains('\n') {
        return None;
    }
    let (target, attributes) = rest.strip_suffix(']')?.split_once('[')?;
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    let mut positional = Vec::new();
    let (mut alt, mut width, mut height) = (None, None, None);
    for attribute in attributes.split(',').map(str::trim) {
        match attribute.split_once('=') {
            Some(("alt", value)) => alt = Some(value.trim_matches('"').to_owned()),
            Some(("width", value)) => width = Some(value.trim_matches('"').to_owned()),
            Some(("height", value)) => height = Some(value.trim_matches('"').to_owned()),
            Some(_) => {}
            None => positional.push(attribute.trim_matches('"').to_owned()),
        }
    }
    let mut positional = positional.into_iter();
    let alt = alt.or(positional.next()).unwrap_or_default();
    let width = width
        .or(positional.next())
        .filter(|value| !value.is_empty());
    let height = height
        .or(positional.next())
        .filter(|value| !value.is_empty());
    Some(BlockKind::Image {
        target: target.to_owned(),
        alt,
        width,
        height,
    })
}

/// Drops the comment lines acdc leaves in text. It keeps tag directives (`// end::x[]`) for
/// include filtering, and they stay in the paragraph, at times with the comment beside them.
fn drop_line_comments(inlines: Vec<Inline>) -> Vec<Inline> {
    inlines
        .into_iter()
        .filter_map(|inline| {
            let Inline::Text(text) = inline else {
                return Some(inline);
            };
            if !text.contains("\n//") {
                return Some(Inline::Text(text));
            }
            let mut lines = text.split('\n');
            let mut kept = lines.next().unwrap_or_default().to_owned();
            for line in lines.filter(|line| !is_line_comment(line)) {
                kept.push('\n');
                kept.push_str(line);
            }
            (!kept.is_empty()).then_some(Inline::Text(kept))
        })
        .collect()
}

/// A line comment as Asciidoctor reads one: `//` not followed by a third `/`.
fn is_line_comment(line: &str) -> bool {
    line.strip_prefix("//")
        .is_some_and(|rest| !rest.starts_with('/'))
}

/// Inline image macros acdc left as text because their target holds `:` or `$`.
fn split_inline_images(inline: Inline) -> Vec<Inline> {
    let Inline::Text(text) = inline else {
        return vec![inline];
    };
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("image:") {
        let after = &rest[start + "image:".len()..];
        let preceded_by_word = rest[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let parsed = after.split_once('[').and_then(|(target, tail)| {
            let (attributes, tail) = tail.split_once(']')?;
            let antora = target.contains([':', '$']);
            (antora
                && !preceded_by_word
                && !target.is_empty()
                && !target.starts_with(':')
                && !target.contains(char::is_whitespace))
            .then_some((target, attributes, tail))
        });
        let Some((target, attributes, tail)) = parsed else {
            let skip = start + "image:".len();
            out.push(Inline::Text(rest[..skip].to_owned()));
            rest = &rest[skip..];
            continue;
        };
        if start > 0 {
            out.push(Inline::Text(rest[..start].to_owned()));
        }
        let alt = attributes
            .split(',')
            .map(str::trim)
            .find_map(|attribute| attribute.strip_prefix("alt="))
            .or_else(|| {
                attributes
                    .split(',')
                    .next()
                    .filter(|first| !first.contains('='))
            })
            .unwrap_or("")
            .trim()
            .trim_matches('"');
        out.push(Inline::Image {
            target: target.to_owned(),
            alt: alt.to_owned(),
        });
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(Inline::Text(rest.to_owned()));
    }
    // Rejoin text the scan split without finding an image.
    let mut merged: Vec<Inline> = Vec::new();
    for inline in out {
        if let (Inline::Text(next), Some(Inline::Text(previous))) = (&inline, merged.last_mut()) {
            previous.push_str(next);
        } else {
            merged.push(inline);
        }
    }
    merged
}

fn source_language(metadata: &acdc::BlockMetadata<'_>) -> Option<String> {
    metadata
        .attributes
        .get_string("language")
        .map(|language| language.into_owned())
        .or_else(|| {
            // `[source,rust]`: the language is the second positional attribute.
            (metadata.style == Some("source"))
                .then(|| metadata.positional_attribute(1))
                .flatten()
                .map(str::to_owned)
        })
        .filter(|language| !language.is_empty())
}

fn inline_text(nodes: &[acdc::InlineNode<'_>]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            acdc::InlineNode::PlainText(text) => text.content,
            acdc::InlineNode::RawText(text) => text.content,
            acdc::InlineNode::VerbatimText(text) => text.content,
            _ => "",
        })
        .collect()
}

fn attribution(metadata: &acdc::BlockMetadata<'_>) -> Option<String> {
    metadata
        .attribution
        .as_ref()
        .map(|attribution| inline_text(attribution))
        .filter(|text| !text.is_empty())
}

fn citation(metadata: &acdc::BlockMetadata<'_>) -> Option<String> {
    metadata
        .citetitle
        .as_ref()
        .map(|cite| inline_text(cite))
        .filter(|text| !text.is_empty())
}

fn align(halign: &acdc::HorizontalAlignment) -> Align {
    match halign {
        acdc::HorizontalAlignment::Left => Align::Start,
        acdc::HorizontalAlignment::Center => Align::Centre,
        acdc::HorizontalAlignment::Right => Align::End,
    }
}

fn cell_style(style: &acdc::ColumnStyle) -> CellStyle {
    match style {
        acdc::ColumnStyle::AsciiDoc => CellStyle::AsciiDoc,
        acdc::ColumnStyle::Default => CellStyle::Default,
        acdc::ColumnStyle::Emphasis => CellStyle::Emphasis,
        acdc::ColumnStyle::Header => CellStyle::Header,
        acdc::ColumnStyle::Literal => CellStyle::Literal,
        acdc::ColumnStyle::Monospace => CellStyle::Monospace,
        acdc::ColumnStyle::Strong => CellStyle::Strong,
        _ => CellStyle::Default,
    }
}
