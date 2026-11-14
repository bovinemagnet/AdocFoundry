use std::borrow::Cow;

use adoc_core::{
    Anchor, AttributeDeclaration, ImageDirective, IncludeDirective, Reference, ReferenceKind,
    SourceRange, UiMacro,
};

pub(crate) struct Heading<'a> {
    pub marker_level: u8,
    pub title: &'a str,
    pub range: SourceRange,
    pub selection_range: SourceRange,
}

pub(crate) fn content(line: &str) -> &str {
    line.trim_end_matches(&['\r', '\n'][..])
}

pub(crate) fn parse_heading(line: &str, offset: usize) -> Option<Heading<'_>> {
    let line = content(line);
    // `=` or, as Asciidoctor also reads, Markdown's `#`.
    let marker = match line.bytes().next() {
        Some(marker @ (b'=' | b'#')) => marker,
        _ => return None,
    };
    let marker_len = line.bytes().take_while(|byte| *byte == marker).count();
    if !(1..=6).contains(&marker_len)
        || !matches!(line.as_bytes().get(marker_len), Some(b' ' | b'\t'))
    {
        return None;
    }

    let raw_title = &line[marker_len + 1..];
    let mut title = raw_title.trim();
    // `== Title ==`: a closing marker as long as the opening one, after a space, is no part of
    // the title.
    if let Some(before) = title.strip_suffix(&line[..marker_len]) {
        if before.ends_with([' ', '\t']) && !before.trim().is_empty() {
            title = before.trim_end();
        }
    }
    if title.is_empty() {
        return None;
    }

    let leading_space = raw_title.len() - raw_title.trim_start().len();
    let selection_start = offset + marker_len + 1 + leading_space;
    Some(Heading {
        marker_level: marker_len as u8,
        title,
        range: SourceRange::new(offset, offset + line.len()),
        selection_range: SourceRange::new(selection_start, selection_start + title.len()),
    })
}

/// An attribute entry, `:name: value`, as Asciidoctor matches one: the name starts with a word
/// character (after a `!` that unsets it), and a space or the end of the line follows its
/// second colon, so `:a:b` and `:30:00 is the time` are text.
pub(crate) fn parse_attribute(line: &str, offset: usize) -> Option<AttributeDeclaration> {
    let line = content(line);
    let rest = line.strip_prefix(':')?;
    let separator = rest.find(':')?;
    let name = &rest[..separator];
    let starts_a_name = name
        .trim_start_matches('!')
        .chars()
        .next()
        .is_some_and(|first| first.is_alphanumeric() || first == '_');
    let after = &rest[separator + 1..];
    if !starts_a_name
        || name.chars().any(char::is_whitespace)
        || !(after.is_empty() || after.starts_with([' ', '\t']))
    {
        return None;
    }

    let value = after.trim();
    Some(AttributeDeclaration {
        name: name.to_owned(),
        value: (!value.is_empty()).then(|| value.to_owned()),
        range: SourceRange::new(offset, offset + line.len()),
    })
}

pub(crate) fn find_anchors(line: &str, offset: usize) -> Vec<Anchor> {
    let line = content(line);
    let mut anchors = find_delimited(line, "[[", "]]", offset)
        .into_iter()
        // A backslash before an anchor shows it as written.
        .filter(|(_, range)| !line[..range.start - offset].ends_with('\\'))
        .filter_map(|(value, range)| {
            // A bibliography entry is written `[[[id, label]]]`, so the extra bracket rides
            // along on the id.
            let id = value.trim_start_matches('[').split(',').next()?;
            is_anchor_id(id).then(|| Anchor {
                id: id.to_owned(),
                range,
                conditional: false,
            })
        })
        .collect::<Vec<_>>();

    anchors.extend(shorthand_anchors(line, offset));
    anchors.extend(id_attribute(line, offset));
    anchors.extend(anchor_macros(line, offset));
    anchors
}

/// Whether `id` may be an anchor's id: as Asciidoctor's pattern has it, a letter, `_` or `:`
/// followed by word characters, `-`, `:` and `.`. An id made with an attribute reference is
/// taken as written, since its value is not known here.
fn is_anchor_id(id: &str) -> bool {
    if is_dynamic_id(id) {
        return !id.contains(char::is_whitespace);
    }
    let mut characters = id.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || matches!(first, '_' | ':'))
        && characters.all(|character| {
            character.is_alphanumeric() || matches!(character, '_' | '-' | ':' | '.')
        })
}

/// The title and id of a section title ending in an anchor, `Title [[id]]` or
/// `Title [[id,reftext]]`, which Asciidoctor takes as the section's id.
pub(crate) fn trailing_anchor(title: &str) -> Option<(&str, &str)> {
    let inside = title.strip_suffix("]]")?;
    let open = inside.rfind("[[")?;
    let before = &title[..open];
    if !before.ends_with(' ') {
        return None;
    }
    let id = inside[open + 2..].split(',').next()?;
    is_anchor_id(id).then(|| (before.trim_end(), id))
}

/// A block attribute line or a block title, which Asciidoctor carries to the next block with
/// any anchor before it.
pub(crate) fn is_block_metadata(trimmed: &str) -> bool {
    block_attribute_list(trimmed).is_some() || is_block_title(trimmed)
}

/// Whether `id` is made with an attribute reference, so it cannot be compared before the
/// attribute is substituted.
pub(crate) fn is_dynamic_id(id: &str) -> bool {
    id.contains('{')
}

/// The list of a line that is a whole block attribute line, such as `[source#id,ruby]`: not an
/// anchor line, and holding no further brackets.
fn block_attribute_list(line: &str) -> Option<&str> {
    let list = line.trim_end().strip_prefix('[')?.strip_suffix(']')?;
    (!list.starts_with('[') && !list.contains(['[', ']'])).then_some(list)
}

/// The style and id of an attribute list's first entry, read as Asciidoctor's shorthand: an
/// optional style, then any of `#id`, `.role` and `%option` in any order, as in
/// `[source#code.lead,ruby]`. A named attribute first (`[link=#top]`) has neither, and an entry
/// holding a space has no shorthand.
fn shorthand(list: &str) -> (&str, Option<&str>) {
    let first = list.split(',').next().unwrap_or_default().trim();
    if first.contains('=') {
        return ("", None);
    }
    let style_end = first.find(['#', '.', '%']).unwrap_or(first.len());
    let style = &first[..style_end];
    if first.contains(char::is_whitespace) {
        return (style.trim(), None);
    }
    let id = first[style_end..]
        .split_once('#')
        .and_then(|(_, rest)| rest.split(['.', '%', '#']).next())
        .filter(|id| !id.is_empty());
    (style, id)
}

/// The id the shorthand of an attribute list's first entry gives.
fn shorthand_id(list: &str) -> Option<&str> {
    shorthand(list).1
}

/// `anchor:id[]` and `anchor:id[reftext]`, the anchor macro, unless escaped. Like Asciidoctor,
/// the macro needs no word boundary before it.
fn anchor_macros(line: &str, offset: usize) -> Vec<Anchor> {
    const PREFIX: &str = "anchor:";
    let mut anchors = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find(PREFIX) {
        let start = cursor + relative;
        let id_start = start + PREFIX.len();
        cursor = id_start;
        let Some(open_relative) = line[id_start..].find('[') else {
            break;
        };
        let open = id_start + open_relative;
        let Some(close_relative) = line[open + 1..].find(']') else {
            break;
        };
        let id = &line[id_start..open];
        if line[..start].ends_with('\\') || !is_anchor_id(id) {
            continue;
        }
        let end = open + 1 + close_relative + 1;
        anchors.push(Anchor {
            id: id.to_owned(),
            range: SourceRange::new(offset + start, offset + end),
            conditional: false,
        });
        cursor = end;
    }
    anchors
}

/// Anchors given by the `#id` shorthand: on a whole block attribute line, or inline in the
/// `[…]` straight before a formatting mark (`[#key]#word#`). Anywhere else, as in link text
/// (`https://example.com/issues/12[#12]`) or prose, the brackets are text.
fn shorthand_anchors(line: &str, offset: usize) -> Vec<Anchor> {
    let anchor = |id: &str, start: usize, end: usize| Anchor {
        id: id.to_owned(),
        range: SourceRange::new(offset + start, offset + end),
        conditional: false,
    };
    if let Some(list) = block_attribute_list(line) {
        return shorthand_id(list)
            .map(|id| anchor(id, 0, line.trim_end().len()))
            .into_iter()
            .collect();
    }
    let mut anchors = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find('[') {
        let open = cursor + relative;
        cursor = open + 1;
        let Some(close_relative) = line[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        let list = &line[open + 1..close];
        if list.contains('[')
            || line[..open].ends_with(['[', '\\'])
            || !before_a_formatting_mark(line, open, close + 1)
        {
            continue;
        }
        if let Some(id) = shorthand_id(list) {
            anchors.push(anchor(id, open, close + 1));
        }
        cursor = close + 1;
    }
    anchors
}

/// Whether the `[…]` from `open` to `after` stands straight before a formatting mark that
/// takes it: `#`, `*`, `_` or `` ` `` opening constrained text after a character that is not
/// a word character, or doubled anywhere, or `^` or `~`, closed later on the line.
fn before_a_formatting_mark(line: &str, open: usize, after: usize) -> bool {
    let rest = &line[after..];
    let Some(mark) = rest.chars().next().filter(|mark| "#*_`^~".contains(*mark)) else {
        return false;
    };
    let doubled = rest[1..].starts_with(mark);
    let opens = doubled
        || matches!(mark, '^' | '~')
        || line[..open]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric() && before != '_');
    let mark_len = if doubled { 2 } else { 1 };
    opens && rest[mark_len..].contains(mark)
}

/// The `id=…` entry of a line that is a whole block attribute list, `[id=intro]` or
/// `[source,id="code"]`.
fn id_attribute(line: &str, offset: usize) -> Option<Anchor> {
    let trimmed = line.trim_end();
    let list = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    if list.starts_with('[') || list.contains(['[', ']']) {
        return None;
    }
    let id = list.split(',').find_map(|entry| {
        let value = entry.trim().strip_prefix("id=")?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        Some(value.trim())
    })?;
    (!id.is_empty()).then(|| Anchor {
        id: id.to_owned(),
        range: SourceRange::new(offset, offset + trimmed.len()),
        conditional: false,
    })
}

pub(crate) fn find_references(line: &str, offset: usize) -> Vec<Reference> {
    let line = content(line);
    let mut references = find_macros(line, "xref:", offset)
        .into_iter()
        .filter(|mac| starts_a_target(&line[mac.range.start - offset + "xref:".len()..]))
        .map(|mac| {
            let (kind, target) = classify_xref_target(mac.target);
            Reference {
                kind,
                target: target.to_owned(),
                text: nonempty(mac.attributes),
                range: mac.range,
            }
        })
        .collect::<Vec<_>>();

    references.extend(find_angle_references(line, offset).into_iter().filter_map(
        |(value, range)| {
            let (target, text) = value
                .split_once(',')
                .map_or((value, None), |(target, text)| (target, nonempty(text)));
            let (kind, target) = classify_angle_target(target.trim());
            (!target.is_empty()).then(|| Reference {
                kind,
                target: target.to_owned(),
                text,
                range,
            })
        },
    ));
    // A backslash before a reference shows it as written.
    references.retain(|reference| !line[..reference.range.start - offset].ends_with('\\'));
    references
}

/// Whether `rest`, what follows `<<` or `xref:`, starts a target as Asciidoctor's pattern
/// wants it: with a word character or one of `#/.:{`.
fn starts_a_target(rest: &str) -> bool {
    rest.chars().next().is_some_and(|first| {
        first.is_alphanumeric() || matches!(first, '_' | '#' | '/' | '.' | ':' | '{')
    })
}

/// Each `<<…>>` on `line` whose target starts as `starts_a_target` wants. An opening that
/// starts none (`a << b`, `<<<`) is text, and the scan goes on from its second `<`, so a
/// later reference on the line is still found.
fn find_angle_references(line: &str, offset: usize) -> Vec<(&str, SourceRange)> {
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = line[cursor..].find("<<") {
        let start = cursor + relative_start;
        let value_start = start + 2;
        if !starts_a_target(&line[value_start..]) {
            cursor = start + 1;
            continue;
        }
        let Some(close_relative) = line[value_start..].find(">>") else {
            break;
        };
        let end = value_start + close_relative + 2;
        found.push((
            &line[value_start..value_start + close_relative],
            SourceRange::new(offset + start, offset + end),
        ));
        cursor = end;
    }
    found
}

/// Asciidoctor reads an `xref:` target as another document only when it names an AsciiDoc file
/// or an Antora resource; otherwise the target is an id in the current document.
fn classify_xref_target(target: &str) -> (ReferenceKind, &str) {
    if let Some(fragment) = target.strip_prefix('#') {
        return (ReferenceKind::LocalAnchor, fragment);
    }
    let names_a_document = target.contains('#')
        || target.contains('$')
        || target
            .rsplit('.')
            .next()
            .is_some_and(|extension| matches!(extension, "adoc" | "asciidoc" | "ad"));
    if names_a_document {
        (ReferenceKind::Xref, target)
    } else {
        (ReferenceKind::LocalAnchor, target)
    }
}

/// Asciidoctor reads a `<<…>>` target as another document only when a `#` follows its start
/// (`<<other.adoc#id>>`, `<<other#id>>`); unlike the `xref:` macro, `<<other.adoc>>` is an id.
fn classify_angle_target(target: &str) -> (ReferenceKind, &str) {
    if let Some(fragment) = target.strip_prefix('#') {
        return (ReferenceKind::LocalAnchor, fragment);
    }
    if target.contains('#') {
        (ReferenceKind::Xref, target)
    } else {
        (ReferenceKind::LocalAnchor, target)
    }
}

/// The include directive on `line`, if it is one. Like Asciidoctor's preprocessor, only a line
/// that starts with `include::` and ends at the directive's closing bracket counts; an include
/// mentioned in prose, indented, escaped or followed by text is not one.
pub(crate) fn find_includes(line: &str, offset: usize) -> Vec<IncludeDirective> {
    let line = content(line).trim_end();
    if !line.starts_with("include::") {
        return Vec::new();
    }
    find_macros(line, "include::", offset)
        .into_iter()
        .take(1)
        .filter(|mac| mac.range.start == offset && mac.range.end == offset + line.len())
        .map(|mac| IncludeDirective {
            target: mac.target.to_owned(),
            attributes: nonempty(mac.attributes),
            range: mac.range,
            conditional: false,
            before_body: false,
        })
        .collect()
}

/// Block and inline images on `line`, in order. A block macro (`image::…`) starts its line, or
/// a table cell (`| image::…`, `a| image::…`); one mentioned anywhere else is text. An inline
/// macro (`image:…[…]`) may stand anywhere.
pub(crate) fn find_images(line: &str, offset: usize) -> Vec<ImageDirective> {
    let line = content(line);
    let mut images: Vec<_> = find_macros(line, "image::", offset)
        .into_iter()
        .filter(|mac| {
            let before = line[..mac.range.start - offset].trim_end();
            let ends_the_line = line[mac.range.end - offset..].trim().is_empty();
            (before.is_empty() && ends_the_line) || before.ends_with('|')
        })
        .map(|mac| ImageDirective {
            target: mac.target.to_owned(),
            attributes: nonempty(mac.attributes),
            range: mac.range,
            inline: false,
            block_attributes: Vec::new(),
        })
        .collect();
    let inline = find_inline_images(line, offset);
    if !inline.is_empty() {
        images.extend(inline);
        images.sort_by_key(|image| image.range.start);
    }
    images
}

/// Inline image macros, `image:target[attributes]`, as Asciidoctor matches them: the target
/// neither starts with `:` (that is the block macro) nor starts or ends with whitespace, and
/// holds no `[`. A backslash before the macro escapes it.
fn find_inline_images(line: &str, offset: usize) -> Vec<ImageDirective> {
    const PREFIX: &str = "image:";
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = line[cursor..].find(PREFIX) {
        let start = cursor + relative_start;
        let target_start = start + PREFIX.len();
        cursor = target_start;
        let opens_target = line[target_start..]
            .chars()
            .next()
            .is_some_and(|first| first != ':' && first != '[' && !first.is_whitespace());
        if !is_boundary(line, start) || !opens_target {
            continue;
        }
        // With no `[` or `]` left on the line, no later macro can close either.
        let Some(open_relative) = line[target_start..].find('[') else {
            break;
        };
        let open = target_start + open_relative;
        let target = &line[target_start..open];
        if target.ends_with(char::is_whitespace) {
            continue;
        }
        let Some(close_relative) = line[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        found.push(ImageDirective {
            target: target.to_owned(),
            attributes: nonempty(&line[open + 1..close]),
            range: SourceRange::new(offset + start, offset + close + 1),
            inline: true,
            block_attributes: Vec::new(),
        });
        cursor = close + 1;
    }
    found
}

/// The preprocessor conditional `line` is: `ifdef::attr[]`, `ifndef::attr[]` and
/// `ifeval::[…]` open a block and `endif::[]` closes one, while `ifdef::attr[text]` is a whole
/// conditional on one line, whose text the preprocessor reads as a line of its own.
/// Directives start their line.
pub(crate) fn conditional_directive(line: &str) -> Option<Conditional<'_>> {
    let line = line.trim_end();
    if !line.ends_with(']') {
        return None;
    }
    if line.starts_with("endif::") {
        return Some(Conditional::Close);
    }
    if line.starts_with("ifeval::[") {
        return Some(Conditional::Open);
    }
    let rest = line
        .strip_prefix("ifdef::")
        .or_else(|| line.strip_prefix("ifndef::"))?;
    let open = line.len() - rest.len() + rest.find('[')?;
    let text = &line[open + 1..line.len() - 1];
    Some(if text.is_empty() {
        Conditional::Open
    } else {
        Conditional::Line {
            start: open + 1,
            text,
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Conditional<'a> {
    Open,
    Close,
    /// A single-line conditional's text, starting at byte `start` of its line.
    Line {
        start: usize,
        text: &'a str,
    },
}

/// Where a line falls among the blocks whose content Asciidoctor does not parse: listing,
/// literal, passthrough and comment blocks, ```` ``` ```` fences, and paragraphs styled
/// `[source]`, `[listing]` or `[literal]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verbatim {
    /// Ordinary text.
    No,
    /// A line that opens or closes a delimited verbatim block.
    Delimiter,
    /// Content of a verbatim block or paragraph, where only includes are processed.
    Content,
    /// Content of a comment block, where nothing is.
    Comment,
    /// The underline of a two-line title, whose text is the line before; the heading's marker
    /// level, 1 for `=` to 5 for `+`.
    Underline(u8),
}

/// Follows the verbatim blocks line by line (each line trimmed).
#[derive(Debug, Default)]
pub(crate) struct VerbatimBlocks<'a> {
    /// The line that closes the open delimited block.
    open: Option<&'a str>,
    /// A verbatim style was given and its block has not begun.
    styled: bool,
    /// Inside a verbatim-styled paragraph.
    paragraph: bool,
    /// Inside a block of text, so a line does not begin a block.
    in_text: bool,
    /// The line before, when it began a block of text and could be a two-line title.
    title: Option<&'a str>,
    /// The delimiters of the example, sidebar, quote, open and table blocks open, innermost
    /// last, inside which a heading is no section.
    compound: Vec<&'a str>,
}

impl<'a> VerbatimBlocks<'a> {
    pub(crate) fn line(&mut self, trimmed: &'a str) -> Verbatim {
        let title = self.title.take();
        if let Some(close) = self.open {
            if trimmed == close {
                self.open = None;
                self.in_text = false;
                return Verbatim::Delimiter;
            }
            return if close.starts_with('/') {
                Verbatim::Comment
            } else {
                Verbatim::Content
            };
        }
        if self.paragraph {
            if trimmed.is_empty() {
                self.paragraph = false;
                return Verbatim::No;
            }
            return Verbatim::Content;
        }
        // Asciidoctor reads a two-line title before a delimiter, so `Section` over `-------`
        // is a section, not the start of a listing block; inside a block it reads none.
        if let Some(level) = title
            .filter(|_| self.compound.is_empty())
            .and_then(|title| setext_level(title, trimmed))
        {
            self.in_text = false;
            return Verbatim::Underline(level);
        }
        if let Some(close) = verbatim_delimiter(trimmed) {
            self.open = Some(close);
            self.styled = false;
            self.in_text = false;
            return Verbatim::Delimiter;
        }
        if self.styled {
            // Block attributes reach past blank lines, titles, comments and further
            // attribute lines.
            if trimmed.is_empty()
                || trimmed.starts_with('[')
                || is_line_comment(trimmed)
                || is_block_title(trimmed)
            {
                return Verbatim::No;
            }
            self.styled = false;
            // An open block takes the verbatim style: `[listing]` over `--` is a listing.
            if trimmed == "--" {
                self.open = Some(trimmed);
                self.in_text = false;
                return Verbatim::Delimiter;
            }
            if !trimmed.starts_with('=') && !is_compound_delimiter(trimmed) {
                self.paragraph = true;
                return Verbatim::Content;
            }
            self.in_text = false;
            self.follow_compound(trimmed);
            return Verbatim::No;
        }
        self.styled = has_verbatim_style(trimmed);
        self.follow_text(trimmed);
        Verbatim::No
    }

    /// Follows where blocks of text begin and end, keeping a block's first line as a possible
    /// two-line title. Block metadata (attribute lines, anchors, titles, comments and
    /// attribute entries) before a block leaves its start where it is.
    fn follow_text(&mut self, trimmed: &'a str) {
        self.follow_compound(trimmed);
        if trimmed.is_empty()
            || is_compound_delimiter(trimmed)
            || (self.begins_block() && parse_heading(trimmed, 0).is_some())
        {
            self.in_text = false;
        } else if !self.in_text
            && !(trimmed.starts_with('[') && trimmed.ends_with(']'))
            && !is_line_comment(trimmed)
            && !is_block_title(trimmed)
            && parse_attribute(trimmed, 0).is_none()
        {
            self.in_text = true;
            self.title = Some(trimmed);
        }
    }

    /// Opens or closes a compound block on its delimiter.
    fn follow_compound(&mut self, trimmed: &'a str) {
        if !is_compound_delimiter(trimmed) {
            return;
        }
        if self.compound.last() == Some(&trimmed) {
            self.compound.pop();
        } else {
            self.compound.push(trimmed);
        }
    }

    /// Whether the line read last stands inside an example, sidebar, quote, open or table
    /// block, where Asciidoctor reads a heading as text.
    pub(crate) fn in_compound_block(&self) -> bool {
        !self.compound.is_empty()
    }

    /// Whether the line about to be read may begin a block, and so be a heading: within a
    /// block of text (a paragraph, a list item, the header's author and revision lines) a `=` or
    /// `#` line is part of it, as Asciidoctor reads it.
    pub(crate) fn begins_block(&self) -> bool {
        !self.in_text
    }

    /// The preprocessor conditional `line` is, which the preprocessor reads everywhere but in
    /// a comment block and removes before blocks are read. Whoever follows the blocks asks
    /// here first and gives the line no further.
    pub(crate) fn directive<'l>(&self, line: &'l str) -> Option<Conditional<'l>> {
        if self.next() == Verbatim::Comment {
            return None;
        }
        conditional_directive(line)
    }

    /// What a line of text typed next would be.
    pub(crate) fn next(&self) -> Verbatim {
        match self.open {
            Some(close) if close.starts_with('/') => Verbatim::Comment,
            Some(_) => Verbatim::Content,
            None if self.paragraph || self.styled => Verbatim::Content,
            None => Verbatim::No,
        }
    }
}

/// The marker level of the two-line title whose text is `title` and whose underline may be
/// `underline`: a run of one of `=`, `-`, `~`, `^` or `+` (levels 1 to 5) within one character
/// of the title's length, under a title that does not start with `.` and holds a word
/// character, as Asciidoctor reads them.
fn setext_level(title: &str, underline: &str) -> Option<u8> {
    let first = underline.chars().next()?;
    let level = match first {
        '=' => 1,
        '-' => 2,
        '~' => 3,
        '^' => 4,
        '+' => 5,
        _ => return None,
    };
    let titled = !title.starts_with('.')
        && title
            .chars()
            .any(|character| character.is_alphanumeric() || character == '_');
    (underline.len() >= 2
        && underline.chars().all(|character| character == first)
        && titled
        && title.chars().count().abs_diff(underline.len()) < 2)
        .then_some(level)
}

/// The line that closes the verbatim block `line` opens: a run of four or more of one of
/// `-`, `.`, `+` or `/`, closed by the same run, or a ```` ``` ```` fence, perhaps naming
/// its language.
fn verbatim_delimiter(line: &str) -> Option<&str> {
    if line
        .strip_prefix("```")
        .is_some_and(|rest| !rest.starts_with('`'))
    {
        return Some("```");
    }
    let first = line.chars().next()?;
    ("-.+/".contains(first) && is_run(line, first)).then_some(line)
}

/// A delimiter of a block that holds other blocks.
fn is_compound_delimiter(line: &str) -> bool {
    line == "--"
        || line.starts_with("|===")
        || line
            .chars()
            .next()
            .is_some_and(|first| "=*_".contains(first) && is_run(line, first))
}

fn is_run(line: &str, character: char) -> bool {
    line.len() >= 4 && line.chars().all(|c| c == character)
}

/// A line comment: `//`, not followed by a third `/`.
pub(crate) fn is_line_comment(trimmed: &str) -> bool {
    trimmed.starts_with("//") && !trimmed.starts_with("///")
}

/// A `.Title` line.
pub(crate) fn is_block_title(line: &str) -> bool {
    line.strip_prefix('.')
        .and_then(|title| title.chars().next())
        .is_some_and(|first| first != '.' && !first.is_whitespace())
}

/// A block attribute line whose style is `source`, `listing`, `literal` or `pass`, such as
/// `[source,ruby]` or `[source#id.role]`.
fn has_verbatim_style(line: &str) -> bool {
    block_attribute_list(line)
        .is_some_and(|list| matches!(shorthand(list).0, "source" | "listing" | "literal" | "pass"))
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

struct Macro<'a> {
    target: &'a str,
    attributes: &'a str,
    range: SourceRange,
}

fn find_macros<'a>(line: &'a str, prefix: &str, offset: usize) -> Vec<Macro<'a>> {
    let mut found = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = line[cursor..].find(prefix) {
        let start = cursor + relative_start;
        if !is_boundary(line, start) {
            cursor = start + prefix.len();
            continue;
        }

        let target_start = start + prefix.len();
        let Some(open_relative) = line[target_start..].find('[') else {
            break;
        };
        let open = target_start + open_relative;
        let target = &line[target_start..open];
        let Some(close_relative) = line[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        // A target neither starts nor ends with a space: `include:: foo.adoc[]` is text.
        if !target.is_empty()
            && !target.starts_with(char::is_whitespace)
            && !target.ends_with(char::is_whitespace)
        {
            found.push(Macro {
                target,
                attributes: &line[open + 1..close],
                range: SourceRange::new(offset + start, offset + close + 1),
            });
        }
        cursor = close + 1;
    }

    found
}

fn find_delimited<'a>(
    line: &'a str,
    open: &str,
    close: &str,
    offset: usize,
) -> Vec<(&'a str, SourceRange)> {
    let mut found = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = line[cursor..].find(open) {
        let start = cursor + relative_start;
        let value_start = start + open.len();
        let Some(close_relative) = line[value_start..].find(close) else {
            break;
        };
        let end = value_start + close_relative + close.len();
        found.push((
            &line[value_start..value_start + close_relative],
            SourceRange::new(offset + start, offset + end),
        ));
        cursor = end;
    }

    found
}

/// The constrained code spans of `line` (passthroughs already blanked) that Asciidoctor does
/// not close where they seem to close (#593): a closing backtick followed by an apostrophe,
/// which it reads as a curved apostrophe (`` `x`'s ``), or by a word character, before which a
/// constrained span cannot close (`` `x`s ``). Each is the span from its opening backtick to
/// the character after the closing one, with a message giving the unconstrained form.
///
/// It follows Asciidoctor's constrained monospace: an opening backtick at the start or after a
/// character other than a word character or one of ``;:"'`}``, then a non-space character.
/// Anything else is left alone: a lone or escaped backtick, curved quotes (`` '`x`' ``), a
/// curved apostrophe (``Sam`'s``) and unconstrained spans (``` ``x`` ```).
pub(crate) fn broken_code_spans(line: &str, offset: usize) -> Vec<(SourceRange, String)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let char_at = |at: usize| line.get(at..).and_then(|rest| rest.chars().next());
    let char_before = |at: usize| line[..at].chars().next_back();
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(relative) = line[index..].find('`') {
        let open = index + relative;
        // An unconstrained span is read first and holds anything.
        if line[open + 1..].starts_with('`') {
            index = match line[open + 2..].find("``") {
                Some(end) => open + 2 + end + 2,
                None => open + 2,
            };
            continue;
        }
        let opens = !char_before(open).is_some_and(|c| is_word(c) || ";:\"'`}\\".contains(c))
            && char_at(open + 1).is_some_and(|c| !c.is_whitespace());
        if !opens {
            index = open + 1;
            continue;
        }
        let Some(relative) = line[open + 1..].find('`') else {
            break;
        };
        let close = open + 1 + relative;
        let last = char_before(close).unwrap_or(' ');
        if last.is_whitespace() || last == '\\' {
            // Not where a span closes: read on from that backtick afresh.
            index = close;
            continue;
        }
        let after = char_at(close + 1);
        let reason = match after {
            Some('\'') => {
                "Asciidoctor reads a backtick followed by an apostrophe as a curved apostrophe"
            }
            Some(c) if is_word(c) => {
                "a backtick followed by a letter or digit cannot close a code span"
            }
            _ => {
                index = close + 1;
                continue;
            }
        };
        index = close + 1;
        // What follows the span, to write after its unconstrained form: `'s`, `s`, `2`.
        let rest = &line[close + 1..];
        let suffix_len = rest
            .char_indices()
            .find(|(at, c)| !(is_word(*c) || (*at == 0 && *c == '\'')))
            .map_or(rest.len(), |(at, _)| at);
        let suffix = &rest[..suffix_len];
        let content = &line[open + 1..close];
        let end = close + 1 + after.map_or(0, char::len_utf8);
        found.push((
            SourceRange::new(offset + open, offset + end),
            format!(
                "code span `{content}` does not close here: {reason}; \
                 write ``{content}``{suffix} or reword it"
            ),
        ));
    }
    found
}

/// The keyboard, button and menu macros in `line` (passthroughs already blanked), as
/// Asciidoctor's patterns read them: `kbd:[…]` and `btn:[…]` with text, `menu:target[…]` with a
/// target beginning with a word character or `&`, none written with a backslash before it.
pub(crate) fn find_ui_macros(line: &str, offset: usize) -> Vec<UiMacro> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(relative) = line[index..].find(':') {
        let colon = index + relative;
        index = colon + 1;
        let Some(name) = ["kbd", "btn", "menu"]
            .into_iter()
            .find(|name| line[..colon].ends_with(name))
        else {
            continue;
        };
        let start = colon - name.len();
        if line[..start].ends_with('\\') {
            continue;
        }
        let rest = &line[colon + 1..];
        let end = if name == "menu" {
            menu_macro_end(rest)
        } else {
            rest.strip_prefix('[').and_then(|text| {
                let close = bracket_end(text)?;
                (close > 0).then_some(close + 2)
            })
        };
        if let Some(end) = end {
            found.push(UiMacro {
                name,
                range: SourceRange::new(offset + start, offset + colon + 1 + end),
            });
            index = colon + 1 + end;
        }
    }
    found
}

/// Where `text`'s first `]` without a backslash before it is.
fn bracket_end(text: &str) -> Option<usize> {
    text.match_indices(']')
        .map(|(at, _)| at)
        .find(|&at| !text[..at].ends_with('\\'))
}

/// The length of a menu macro's `target[…]`, read after `menu:`.
fn menu_macro_end(rest: &str) -> Option<usize> {
    let open = rest.find(['[', '\n'])?;
    let target = &rest[..open];
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let first = target.chars().next()?;
    let last = target.chars().next_back()?;
    let shaped = if target.len() == first.len_utf8() {
        is_word(first)
    } else {
        (is_word(first) || first == '&') && !last.is_whitespace()
    };
    if !shaped || !rest[open..].starts_with('[') {
        return None;
    }
    let close = bracket_end(&rest[open + 1..])?;
    Some(open + 1 + close + 1)
}

/// `line` with every inline passthrough blanked to spaces of the same byte length.
///
/// Asciidoctor extracts passthroughs before any other substitution, so nothing inside one is a
/// reference, anchor or image. Blanking rather than removing keeps every byte offset valid.
pub(crate) fn mask_passthroughs(line: &str) -> Cow<'_, str> {
    let spans = passthrough_spans(line);
    if spans.is_empty() {
        return Cow::Borrowed(line);
    }
    let mut masked = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in spans {
        masked.push_str(&line[cursor..start]);
        masked.extend(std::iter::repeat_n(' ', end - start));
        cursor = end;
    }
    masked.push_str(&line[cursor..]);
    Cow::Owned(masked)
}

/// The byte spans of `pass:[…]`, `+++…+++`, `++…++`, `$$…$$` and constrained `+…+`
/// passthroughs.
fn passthrough_spans(line: &str) -> Vec<(usize, usize)> {
    scan_passthroughs(line).spans
}

/// Where the passthrough that `line` leaves open begins, read as `passthrough_spans` reads
/// passthroughs: the first opening that could begin one and finds no close. Completion asks
/// with the text before the caret, which then stands inside it.
///
/// An unclosed run of plus signs counts only where a constrained `+` could open, after a
/// non-word character and before a non-space one: a closed `++` may open after a word, but
/// an unclosed one there is far more often `C++`.
pub(crate) fn open_passthrough(line: &str) -> Option<usize> {
    scan_passthroughs(line).open
}

#[derive(Default)]
struct Passthroughs {
    spans: Vec<(usize, usize)>,
    open: Option<usize>,
}

fn scan_passthroughs(line: &str) -> Passthroughs {
    let bytes = line.as_bytes();
    let mut found = Passthroughs::default();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // An escaped character opens nothing. Only ASCII bytes are compared below, so
            // stepping into the middle of a multi-byte character is harmless.
            b'\\' => index += 2,
            b'p' if line[index..].starts_with("pass:") && is_boundary(line, index) => {
                match pass_macro_end(line, index) {
                    Some(end) => {
                        found.spans.push((index, end));
                        index = end;
                    }
                    None => {
                        if pass_macro_content(line, index).is_some() {
                            found.open.get_or_insert(index);
                        }
                        index += 1;
                    }
                }
            }
            b'$' if line[index..].starts_with("$$") => match line[index + 2..].find("$$") {
                Some(relative) => {
                    let end = index + 2 + relative + 2;
                    found.spans.push((index, end));
                    index = end;
                }
                None => {
                    found.open.get_or_insert(index);
                    index += 2;
                }
            },
            b'+' => {
                let run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'+')
                    .count();
                match plus_passthrough_end(line, index, run) {
                    Some(end) => {
                        found.spans.push((index, end));
                        index = end;
                    }
                    None => {
                        if run <= 3 && plus_opens(line, index, run) {
                            found.open.get_or_insert(index);
                        }
                        index += run;
                    }
                }
            }
            _ => index += 1,
        }
    }
    found
}

/// Where the content of `pass:subs[` starting at `start` begins, when its substitutions are
/// a list Asciidoctor reads.
fn pass_macro_content(line: &str, start: usize) -> Option<usize> {
    let after = start + "pass:".len();
    let open = after + line[after..].find('[')?;
    let substitutions = &line[after..open];
    substitutions
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == ',')
        .then_some(open + 1)
}

/// The end of `pass:subs[content]` starting at `start`.
fn pass_macro_end(line: &str, start: usize) -> Option<usize> {
    let content = pass_macro_content(line, start)?;
    let close = content + line[content..].find(']')?;
    Some(close + 1)
}

/// Whether a run of `run` plus signs at `start` may open a constrained passthrough: after a
/// non-word character, and with no space after it.
fn plus_opens(line: &str, start: usize, run: usize) -> bool {
    line[..start].chars().next_back().is_none_or(|character| {
        !character.is_alphanumeric() && !matches!(character, '_' | ';' | ':')
    }) && !line[start + run..].starts_with(|character: char| character.is_whitespace())
}

/// The end of a passthrough opened by a run of `run` plus signs at `start`.
fn plus_passthrough_end(line: &str, start: usize, run: usize) -> Option<usize> {
    match run {
        // `++…++` and `+++…+++` are unconstrained: they may open and close anywhere.
        2 | 3 => {
            let delimiter = &line[start..start + run];
            let content = start + run;
            let close = content + line[content..].find(delimiter)?;
            (close > content).then_some(close + run)
        }
        // `+…+` is constrained: it opens after a non-word character, and its content starts
        // and ends with a non-space character before a closing `+` that ends a word.
        1 => {
            if !plus_opens(line, start, run) {
                return None;
            }
            let content = start + 1;
            line[content..]
                .match_indices('+')
                .map(|(relative, _)| content + relative)
                .find(|&close| {
                    close > content
                        && !line[..close].ends_with(|character: char| character.is_whitespace())
                        && !line[close + 1..].starts_with(|character: char| {
                            character.is_alphanumeric() || character == '_'
                        })
                })
                .map(|close| close + 1)
        }
        _ => None,
    }
}

pub(crate) fn is_boundary(line: &str, start: usize) -> bool {
    // A leading backslash escapes a macro, which is how a document shows one literally.
    line[..start].chars().next_back().is_none_or(|character| {
        !character.is_alphanumeric() && character != '_' && character != '\\'
    })
}
