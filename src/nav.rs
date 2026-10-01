//! The structure of an Antora navigation file.

/// One entry of a navigation file: a `.Title` line or a list item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavEntry {
    /// `0` for a `.Title`, otherwise the number of `*` marking the item.
    pub level: u8,
    /// The link text of an `xref:`, its target when the text is empty, or the item's text.
    pub text: String,
    /// The `xref:` target, for an entry that links to a page.
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
    let mut entries = Vec::new();
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

        let entry = |level, text: &str, target: Option<&str>| NavEntry {
            level,
            text: text.to_owned(),
            target: target.map(str::to_owned),
            start,
            end: start + content.len(),
        };
        if let Some(title) = block_title(content) {
            entries.push(entry(0, title, None));
        } else if let Some((level, item)) = list_item(content) {
            match xref(item) {
                Some((target, link_text)) => {
                    let text = if link_text.is_empty() {
                        target
                    } else {
                        link_text
                    };
                    entries.push(entry(level, text, Some(target)));
                }
                None => entries.push(entry(level, item, None)),
            }
        }
    }
    entries
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

/// The level and text of a `*` list item.
fn list_item(line: &str) -> Option<(u8, &str)> {
    let line = line.trim_start();
    let stars = line.bytes().take_while(|byte| *byte == b'*').count();
    let text = line[stars..].strip_prefix(' ')?.trim();
    let level = u8::try_from(stars)
        .ok()
        .filter(|level| (1..=5).contains(level))?;
    (!text.is_empty()).then_some((level, text))
}

/// The target and link text of the first `xref:` macro in `item`.
fn xref(item: &str) -> Option<(&str, &str)> {
    let after = &item[item.find("xref:")? + "xref:".len()..];
    let (target, rest) = after.split_once('[')?;
    let (link_text, _) = rest.split_once(']')?;
    Some((target.trim(), link_text.trim()))
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
