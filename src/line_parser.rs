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
        .filter_map(|(value, range)| {
            // A bibliography entry is written `[[[id, label]]]`, so the extra bracket rides
            // along on the id.
            let id = value.trim_start_matches('[').split(',').next()?.trim();
            (!id.is_empty()).then(|| Anchor {
                id: id.to_owned(),
                range,
            })
        })
        .collect::<Vec<_>>();

    anchors.extend(
        find_delimited(line, "[#", "]", offset)
            .into_iter()
            .filter_map(|(value, range)| {
                let id = value.trim();
                (!id.is_empty()).then(|| Anchor {
                    id: id.to_owned(),
                    range,
                })
            }),
    );
    anchors
}

pub(crate) fn find_references(line: &str, offset: usize) -> Vec<Reference> {
    let line = content(line);
    let mut references = find_macros(line, "xref:", offset)
        .into_iter()
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

    references.extend(
        find_delimited(line, "<<", ">>", offset)
            .into_iter()
            .filter_map(|(value, range)| {
                let (target, text) = value
                    .split_once(',')
                    .map_or((value, None), |(target, text)| (target, nonempty(text)));
                let target = target.trim();
                (!target.is_empty()).then(|| Reference {
                    kind: ReferenceKind::LocalAnchor,
                    target: target.to_owned(),
                    text,
                    range,
                })
            }),
    );
    references
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

pub(crate) fn find_includes(line: &str, offset: usize) -> Vec<IncludeDirective> {
    find_macros(content(line), "include::", offset)
        .into_iter()
        .map(|mac| IncludeDirective {
            target: mac.target.to_owned(),
            attributes: nonempty(mac.attributes),
            range: mac.range,
        })
        .collect()
}

pub(crate) fn find_images(line: &str, offset: usize) -> Vec<ImageDirective> {
    find_macros(content(line), "image::", offset)
        .into_iter()
        .map(|mac| ImageDirective {
            target: mac.target.to_owned(),
            attributes: nonempty(mac.attributes),
            range: mac.range,
        })
        .collect()
}

pub(crate) fn is_verbatim_delimiter(line: &str) -> bool {
    matches!(line.trim(), "----" | "...." | "++++" | "////")
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

fn is_boundary(line: &str, start: usize) -> bool {
    // A leading backslash escapes a macro, which is how a document shows one literally.
    line[..start].chars().next_back().is_none_or(|character| {
        !character.is_alphanumeric() && character != '_' && character != '\\'
    })
}
