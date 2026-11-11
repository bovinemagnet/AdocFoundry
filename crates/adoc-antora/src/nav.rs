//! The structure of an Antora navigation file.

use adoc_core::SourceRange;

/// One entry of a navigation file: a `.Title` line or a list item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavEntry {
    /// `0` for a `.Title`, otherwise the number of `*` marking the item (`1` for `-`).
    pub level: u8,
    /// The link text of an `xref:`, its target when the text is empty, or the item's text.
    pub text: String,
    /// The target of the entry's `xref:`, or of its `<<page.adoc#,text>>` shorthand (without an
    /// empty fragment), for an entry that links to a page.
    pub target: Option<String>,
    /// Byte offsets of the entry's line, without its line ending.
    pub start: usize,
    pub end: usize,
}

/// The titles and list items of a navigation file, in document order.
///
/// Comments and the contents of delimited blocks are skipped: they hold no navigation.
#[must_use]
pub fn parse_nav(text: &str) -> Vec<NavEntry> {
    parse_nav_and_includes(text).0
}

/// The entries of a navigation file as [`parse_nav`] reads them, and the range of each
/// `include::` line it passes over outside comments and delimited blocks, whose entries only the
/// build can know.
#[must_use]
pub fn parse_nav_and_includes(text: &str) -> (Vec<NavEntry>, Vec<SourceRange>) {
    let mut entries = Vec::new();
    let mut includes = Vec::new();
    let mut open_delimiter: Option<&str> = None;
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim();

        if let Some(delimiter) = open_delimiter {
            if trimmed == delimiter {
                open_delimiter = None;
            }
            continue;
        }
        if is_block_delimiter(trimmed) {
            open_delimiter = Some(trimmed);
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("include::") {
            includes.push(SourceRange::new(start, start + content.len()));
            continue;
        }

        let entry = |level, text: &str, target: Option<&str>| NavEntry {
            level,
            text: text.to_owned(),
            target: target.map(str::to_owned),
            start,
            end: start + content.len(),
        };
        // A title, like an item, may link to a page.
        let linked = |level, text: &str| match xref(text) {
            Some((target, link_text)) => {
                let text = if link_text.is_empty() {
                    target
                } else {
                    link_text
                };
                entry(level, text, Some(target))
            }
            None => entry(level, text, None),
        };
        if let Some(title) = block_title(content) {
            entries.push(linked(0, title));
        } else if let Some((level, item)) = list_item(content) {
            entries.push(linked(level, item));
        }
    }
    (entries, includes)
}

/// A run of four or more of the same delimiter character, opening or closing a block.
fn is_block_delimiter(line: &str) -> bool {
    let mut characters = line.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    line.len() >= 4 && "/.-+=*_".contains(first) && characters.all(|character| character == first)
}

/// The text of a `.Title` line.
fn block_title(line: &str) -> Option<&str> {
    let title = line.strip_prefix('.')?;
    let first = title.chars().next()?;
    (first != '.' && !first.is_whitespace()).then(|| title.trim_end())
}

/// The level and text of a list item: `*` to `*****`, or `-`, a list of the first level.
fn list_item(line: &str) -> Option<(u8, &str)> {
    let line = line.trim_start();
    if let Some(text) = line.strip_prefix("- ") {
        let text = text.trim();
        return (!text.is_empty()).then_some((1, text));
    }
    let stars = line.bytes().take_while(|byte| *byte == b'*').count();
    let text = line[stars..].strip_prefix(' ')?.trim();
    let level = u8::try_from(stars)
        .ok()
        .filter(|level| (1..=5).contains(level))?;
    (!text.is_empty()).then_some((level, text))
}

/// The target and link text of the first link to a page in `item`: an `xref:` macro, or the
/// shorthand `<<page.adoc#fragment,text>>`, which Asciidoctor reads as the same reference.
fn xref(item: &str) -> Option<(&str, &str)> {
    let shorthand = item.find("<<");
    if let Some(start) = item.find("xref:") {
        if shorthand.is_none_or(|shorthand| start < shorthand) {
            let after = &item[start + "xref:".len()..];
            let (target, rest) = after.split_once('[')?;
            let (link_text, _) = rest.split_once(']')?;
            return Some((target.trim(), link_text.trim()));
        }
    }
    let after = &item[shorthand? + "<<".len()..];
    let (inside, _) = after.split_once(">>")?;
    let (target, link_text) = inside.split_once(',').unwrap_or((inside, ""));
    let target = target.trim();
    // Only a target naming a document is a page; `<<anchor>>` is a place in this file, and
    // `<<other#details>>` names `other.adoc`, as Asciidoctor reads it.
    let (page, fragment) = target.split_once('#').unwrap_or((target, ""));
    if !adoc_core::xref_file(target).ends_with(".adoc") {
        return None;
    }
    Some((
        if fragment.is_empty() { page } else { target },
        link_text.trim(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{parse_nav, NavEntry};

    fn summary(entries: &[NavEntry]) -> Vec<(u8, &str, Option<&str>)> {
        entries
            .iter()
            .map(|entry| (entry.level, entry.text.as_str(), entry.target.as_deref()))
            .collect()
    }

    #[test]
    fn reads_titles_and_nested_items() {
        let text = ".Getting Started\n* xref:index.adoc[Introduction]\n** xref:install.adoc[Install]\n** Plain category\n*** xref:security:auth.adoc[]\n";

        assert_eq!(
            summary(&parse_nav(text)),
            [
                (0, "Getting Started", None),
                (1, "Introduction", Some("index.adoc")),
                (2, "Install", Some("install.adoc")),
                (2, "Plain category", None),
                (3, "security:auth.adoc", Some("security:auth.adoc")),
            ]
        );
    }

    #[test]
    fn linked_titles_read_their_text_and_target() {
        let text = ".xref:index.adoc[Home]\n* xref:a.adoc[A]\n.xref:b.adoc[]\n";

        assert_eq!(
            summary(&parse_nav(text)),
            [
                (0, "Home", Some("index.adoc")),
                (1, "A", Some("a.adoc")),
                (0, "b.adoc", Some("b.adoc")),
            ]
        );
    }

    #[test]
    fn reads_dash_items_and_the_xref_shorthand() {
        let text = "* <<shorthand.adoc#,Shorthand>>\n- xref:dash.adoc[Dash]\n** <<deep.adoc#part,Deep>>\n* <<bare.adoc#>>\n* <<anchor,Not a page>>\n";

        assert_eq!(
            summary(&parse_nav(text)),
            [
                (1, "Shorthand", Some("shorthand.adoc")),
                (1, "Dash", Some("dash.adoc")),
                (2, "Deep", Some("deep.adoc#part")),
                (1, "bare.adoc", Some("bare.adoc")),
                (1, "<<anchor,Not a page>>", None),
            ]
        );
    }

    #[test]
    fn reports_only_the_includes_it_reads_past() {
        let text = "include::partial$a.adoc[]\n////\ninclude::partial$b.adoc[]\n////\n----\ninclude::partial$c.adoc[]\n----\n// include::partial$d.adoc[]\n";
        let (_, includes) = super::parse_nav_and_includes(text);

        assert_eq!(
            includes
                .iter()
                .map(|range| &text[range.start..range.end])
                .collect::<Vec<_>>(),
            ["include::partial$a.adoc[]"]
        );
    }

    #[test]
    fn records_each_entry_s_line() {
        let text = "* xref:a.adoc[A]\n** xref:b.adoc[B]\n";
        let entries = parse_nav(text);

        assert_eq!((entries[1].start, entries[1].end), (17, text.len() - 1));
    }

    #[test]
    fn ignores_lines_that_are_not_titles_or_items() {
        let text = "// a comment\n////\n* commented out\n////\n....\nliteral\n....\n*bold* text\n:attr: value\n\n* xref:a.adoc[A]\n";

        assert_eq!(summary(&parse_nav(text)), [(1, "A", Some("a.adoc"))]);
    }
}
