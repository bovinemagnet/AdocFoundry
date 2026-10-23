//! Asciidoctor's typographic replacements, which acdc leaves to the converter: `(C)`, dashes,
//! ellipses, apostrophes and arrows.

/// One replaced stretch of text: the bytes it covers and what is shown instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Replacement {
    pub range: std::ops::Range<usize>,
    pub replacement: &'static str,
}

/// `text` with the replacements applied, as Asciidoctor's `replacements` substitution does.
#[must_use]
pub(crate) fn replace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for span in replacement_spans(text) {
        out.push_str(&text[last..span.range.start]);
        out.push_str(span.replacement);
        last = span.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// The stretches of `text` the replacements change, in order, as byte ranges. A backslash that
/// keeps a pattern literal is part of its stretch, which shows the pattern without it.
#[must_use]
pub fn replacement_spans(text: &str) -> Vec<Replacement> {
    let indexed: Vec<(usize, char)> = text.char_indices().collect();
    let chars: Vec<char> = indexed.iter().map(|(_, character)| *character).collect();
    let byte = |index: usize| indexed.get(index).map_or(text.len(), |(byte, _)| *byte);
    let mut spans = Vec::new();
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
                let end = index + 1 + pattern.chars().count();
                spans.push(Replacement {
                    range: byte(index)..byte(end),
                    replacement: pattern,
                });
                index = end;
                continue;
            }
        }
        let mut start = index;
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
            let previous_span_end = spans.last().map_or(0, |span: &Replacement| span.range.end);
            if previous == Some(&' ') && byte(index - 1) >= previous_span_end {
                start = index - 1;
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
            index += 1;
            continue;
        };
        spans.push(Replacement {
            range: byte(start)..byte(index + consumed),
            replacement,
        });
        index += consumed;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::replace;

    #[test]
    fn spans_name_each_replaced_stretch() {
        let text = "a -- b (C) it's \\-> x...";
        let spans = super::replacement_spans(text);
        let found: Vec<(&str, &str)> = spans
            .iter()
            .map(|span| (&text[span.range.clone()], span.replacement))
            .collect();
        assert_eq!(
            found,
            [
                (" -- ", "\u{2009}\u{2014}\u{2009}"),
                ("(C)", "\u{a9}"),
                ("'", "\u{2019}"),
                ("\\->", "->"),
                ("...", "\u{2026}\u{200b}"),
            ]
        );
        for text in [
            "word--word",
            "日本 -- ok",
            "a-->b",
            "x - y, '' and --flag",
            "a\\b",
        ] {
            let mut applied = String::new();
            let mut last = 0;
            for span in super::replacement_spans(text) {
                applied.push_str(&text[last..span.range.start]);
                applied.push_str(span.replacement);
                last = span.range.end;
            }
            applied.push_str(&text[last..]);
            assert_eq!(applied, replace(text), "{text}");
        }
    }

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
