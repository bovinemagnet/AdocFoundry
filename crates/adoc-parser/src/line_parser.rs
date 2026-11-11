use std::borrow::Cow;

use adoc_core::{
    Anchor, AttributeDeclaration, ImageDirective, IncludeDirective, Reference, ReferenceKind,
    SourceRange,
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
    let marker_len = line.bytes().take_while(|byte| *byte == b'=').count();
    if !(1..=6).contains(&marker_len) || line.as_bytes().get(marker_len) != Some(&b' ') {
        return None;
    }

    let raw_title = &line[marker_len + 1..];
    let title = raw_title.trim();
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

pub(crate) fn parse_attribute(line: &str, offset: usize) -> Option<AttributeDeclaration> {
    let line = content(line);
    let rest = line.strip_prefix(':')?;
    let separator = rest.find(':')?;
    let name = &rest[..separator];
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }

    let value = rest[separator + 1..].trim();
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

/// The id the shorthand of an attribute list's first entry gives, `#id` in `[#id.role%option]`
/// or `[#id,name=value]`. An entry holding a space has no shorthand, as in Asciidoctor.
fn shorthand_id(list: &str) -> Option<&str> {
    let first = list.split(',').next()?.trim();
    if first.contains(char::is_whitespace) {
        return None;
    }
    let id = first.strip_prefix('#')?.split(['.', '%', '#']).next()?;
    (!id.is_empty()).then_some(id)
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
            before.is_empty() || before.ends_with('|')
        })
        .map(|mac| ImageDirective {
            target: mac.target.to_owned(),
            attributes: nonempty(mac.attributes),
            range: mac.range,
            inline: false,
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
        });
        cursor = close + 1;
    }
    found
}

/// How a preprocessor conditional line changes the nesting: `ifdef::attr[]`, `ifndef::attr[]`
/// and `ifeval::[…]` open a block (`ifdef::attr[text]` is a whole conditional on one line)
/// and `endif::[]` closes one. Directives start their line.
pub(crate) fn conditional_directive(line: &str) -> Option<Conditional> {
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
    let (_, text) = rest.split_once('[')?;
    (text == "]").then_some(Conditional::Open)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Conditional {
    Open,
    Close,
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
}

impl<'a> VerbatimBlocks<'a> {
    pub(crate) fn line(&mut self, trimmed: &'a str) -> Verbatim {
        if let Some(close) = self.open {
            if trimmed == close {
                self.open = None;
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
        if let Some(close) = verbatim_delimiter(trimmed) {
            self.open = Some(close);
            self.styled = false;
            return Verbatim::Delimiter;
        }
        if self.styled {
            // Block attributes reach past blank lines, titles, comments and further
            // attribute lines.
            if trimmed.is_empty()
                || trimmed.starts_with('[')
                || trimmed.starts_with("//")
                || is_block_title(trimmed)
            {
                return Verbatim::No;
            }
            self.styled = false;
            if !trimmed.starts_with('=') && !is_compound_delimiter(trimmed) {
                self.paragraph = true;
                return Verbatim::Content;
            }
            return Verbatim::No;
        }
        self.styled = has_verbatim_style(trimmed);
        Verbatim::No
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

/// A `.Title` line.
fn is_block_title(line: &str) -> bool {
    line.strip_prefix('.')
        .and_then(|title| title.chars().next())
        .is_some_and(|first| first != '.' && !first.is_whitespace())
}

/// A block attribute line whose style is `source`, `listing` or `literal`, such as
/// `[source,ruby]` or `[source#id.role]`.
fn has_verbatim_style(line: &str) -> bool {
    let Some(attributes) = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .filter(|attributes| !attributes.starts_with('['))
    else {
        return false;
    };
    let first = attributes.split(',').next().unwrap_or_default();
    let style = first
        .split(['#', '.', '%'])
        .next()
        .unwrap_or_default()
        .trim();
    matches!(style, "source" | "listing" | "literal")
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
        let target = line[target_start..open].trim();
        let Some(close_relative) = line[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_relative;
        if !target.is_empty() {
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
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // An escaped character opens nothing. Only ASCII bytes are compared below, so
            // stepping into the middle of a multi-byte character is harmless.
            b'\\' => index += 2,
            b'p' if line[index..].starts_with("pass:") && is_boundary(line, index) => {
                match pass_macro_end(line, index) {
                    Some(end) => {
                        spans.push((index, end));
                        index = end;
                    }
                    None => index += 1,
                }
            }
            b'$' if line[index..].starts_with("$$") => match line[index + 2..].find("$$") {
                Some(relative) => {
                    let end = index + 2 + relative + 2;
                    spans.push((index, end));
                    index = end;
                }
                None => index += 2,
            },
            b'+' => {
                let run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'+')
                    .count();
                match plus_passthrough_end(line, index, run) {
                    Some(end) => {
                        spans.push((index, end));
                        index = end;
                    }
                    None => index += run,
                }
            }
            _ => index += 1,
        }
    }
    spans
}

/// The end of `pass:subs[content]` starting at `start`.
fn pass_macro_end(line: &str, start: usize) -> Option<usize> {
    let after = start + "pass:".len();
    let open = after + line[after..].find('[')?;
    let substitutions = &line[after..open];
    if !substitutions
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == ',')
    {
        return None;
    }
    let close = open + 1 + line[open + 1..].find(']')?;
    Some(close + 1)
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
            let opens = line[..start].chars().next_back().is_none_or(|character| {
                !character.is_alphanumeric() && !matches!(character, '_' | ';' | ':')
            });
            let content = start + 1;
            if !opens || line[content..].starts_with(|character: char| character.is_whitespace()) {
                return None;
            }
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
