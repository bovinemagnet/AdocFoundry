//! Asciidoctor's typographic replacements, which acdc leaves to the converter: `(C)`, dashes,
//! ellipses, apostrophes and arrows.

/// `text` with the replacements applied, as Asciidoctor's `replacements` substitution does.
#[must_use]
pub(crate) fn replace(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let at = |index: usize, pattern: &str| {
        pattern
            .chars()
            .enumerate()
            .all(|(offset, character)| chars.get(index + offset) == Some(&character))
    };
    let word =
        |character: Option<&char>| character.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut index = 0;
    while index < chars.len() {
        let previous = index
            .checked_sub(1)
            .and_then(|previous| chars.get(previous));
        let next = |offset: usize| chars.get(index + offset);
        // A backslash keeps what follows it literally, and goes.
        if chars[index] == '\\' {
            let escaped = ["(C)", "(R)", "(TM)", "--", "...", "->", "=>", "<-", "<="]
                .into_iter()
                .find(|pattern| at(index + 1, pattern));
            if let Some(pattern) = escaped {
                out.push_str(pattern);
                index += 1 + pattern.chars().count();
                continue;
            }
        }
        let (replacement, consumed): (&str, usize) = if at(index, "(C)") {
            ("\u{a9}", 3)
        } else if at(index, "(R)") {
            ("\u{ae}", 3)
        } else if at(index, "(TM)") {
            ("\u{2122}", 4)
        } else if at(index, "--")
            && previous.is_none_or(|c| matches!(c, ' ' | '\n'))
            && next(2).is_none_or(|c| matches!(c, ' ' | '\n'))
        {
            // A spaced dash takes its spaces: thin space, em dash, thin space.
            if out.ends_with(' ') {
                out.pop();
            }
            let spaced = next(2) == Some(&' ');
            ("\u{2009}\u{2014}\u{2009}", if spaced { 3 } else { 2 })
        } else if at(index, "--") && word(previous) && word(next(2)) {
            ("\u{2014}\u{200b}", 2)
        } else if at(index, "...") {
            ("\u{2026}\u{200b}", 3)
        } else if at(index, "`'") {
            ("\u{2019}", 2)
        } else if chars[index] == '\''
            && previous.is_some_and(char::is_ascii_alphanumeric)
            && next(1) != Some(&'\'')
        {
            ("\u{2019}", 1)
        } else if at(index, "->") {
            ("\u{2192}", 2)
        } else if at(index, "=>") {
            ("\u{21d2}", 2)
        } else if at(index, "<-") {
            ("\u{2190}", 2)
        } else if at(index, "<=") {
            ("\u{21d0}", 2)
        } else {
            out.push(chars[index]);
            index += 1;
            continue;
        };
        out.push_str(replacement);
        index += consumed;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::replace;

    #[test]
    fn replaces_as_asciidoctor_does() {
        assert_eq!(replace("(C) (R) (TM)"), "\u{a9} \u{ae} \u{2122}");
        assert_eq!(replace("a -- b"), "a\u{2009}\u{2014}\u{2009}b");
        assert_eq!(replace("word--word"), "word\u{2014}\u{200b}word");
        assert_eq!(replace("wait..."), "wait\u{2026}\u{200b}");
        assert_eq!(replace("it's"), "it\u{2019}s");
        assert_eq!(
            replace("-> <- => <="),
            "\u{2192} \u{2190} \u{21d2} \u{21d0}"
        );
        assert_eq!(replace("a-->b"), "a-\u{2192}b");
        assert_eq!(replace("x - y, '' and --flag"), "x - y, '' and --flag");
        assert_eq!(replace("日本 -- ok"), "日本\u{2009}\u{2014}\u{2009}ok");
        assert_eq!(replace("\\(C) and \\-> stay"), "(C) and -> stay");
        assert_eq!(replace("a\\b"), "a\\b");
    }
}
