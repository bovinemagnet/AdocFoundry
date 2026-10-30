//! Source highlighting with the editor's tree-sitter grammars and the queries the preview uses,
//! coloured as the preview's light theme colours them, so a listing in the PDF looks as it does
//! in the app. It follows gpui-component's highlighter: one query of injections, locals and
//! highlights (the injection patterns disabled), injected languages highlighted after their host,
//! overlapping captures resolved with the innermost winning, and capture names falling back to
//! their first part (`keyword.modifier` is styled as `keyword`).

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator, Tree};

/// How a run of code is styled; the default leaves the code's own colour.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Style {
    /// Red, green and blue.
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
}

/// One language: its grammar and its queries.
struct Grammar {
    name: &'static str,
    language: Language,
    highlights: &'static str,
    injections: &'static str,
    locals: &'static str,
}

/// The grammar for a source block's language, by the names the preview accepts.
fn grammar(name: &str) -> Option<Grammar> {
    let (name, language, highlights, injections, locals): (_, Language, _, _, _) = match name {
        "json" | "jsonc" => (
            "json",
            tree_sitter_json::LANGUAGE.into(),
            include_str!("queries/json/highlights.scm"),
            "",
            "",
        ),
        "bash" | "sh" => (
            "bash",
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            "",
        ),
        "go" => (
            "go",
            tree_sitter_go::LANGUAGE.into(),
            include_str!("queries/go/highlights.scm"),
            "",
            "",
        ),
        "html" => (
            "html",
            tree_sitter_html::LANGUAGE.into(),
            include_str!("queries/html/highlights.scm"),
            include_str!("queries/html/injections.scm"),
            "",
        ),
        "java" => (
            "java",
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        "javascript" | "js" => (
            "javascript",
            tree_sitter_javascript::LANGUAGE.into(),
            include_str!("queries/javascript/highlights.scm"),
            include_str!("queries/javascript/injections.scm"),
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        "python" | "py" => (
            "python",
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        "rust" | "rs" => (
            "rust",
            tree_sitter_rust::LANGUAGE.into(),
            include_str!("queries/rust/highlights.scm"),
            include_str!("queries/rust/injections.scm"),
            "",
        ),
        "sql" => (
            "sql",
            tree_sitter_sequel::LANGUAGE.into(),
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        "toml" => (
            "toml",
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        "typescript" | "ts" => (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            include_str!("queries/typescript/highlights.scm"),
            include_str!("queries/javascript/injections.scm"),
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        "yaml" | "yml" => (
            "yaml",
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        _ => return None,
    };
    Some(Grammar {
        name,
        language,
        highlights,
        injections,
        locals,
    })
}

/// The preview's light theme: gpui-component's "Default Light" syntax colours.
fn theme_style(name: &str) -> Option<Style> {
    let color = |hex: u32| Style {
        color: Some([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]),
        bold: false,
        italic: false,
    };
    let style = match name {
        "attribute" => color(0x957931),
        "boolean" | "constant" | "variable.special" => color(0xC5060B),
        "comment" | "comment.doc" => color(0x007FFF),
        "constructor" | "keyword" | "number" | "tag" | "title" => color(0x0433FF),
        "embedded" | "property" | "variable" => color(0x333333),
        "emphasis" => Style {
            italic: true,
            ..Style::default()
        },
        "emphasis.strong" => Style {
            bold: true,
            ..Style::default()
        },
        "function" | "link_text" => color(0x0000A2),
        "link_uri" => Style {
            italic: true,
            ..color(0x6A7293)
        },
        "string" | "string.escape" | "string.regex" => color(0x036A07),
        "string.special" | "string.special.symbol" => color(0xD21F07),
        "text.literal" | "text.code.span" | "type" => color(0x6F42C1),
        _ => {
            // `keyword.modifier` falls back to `keyword`.
            let (first, _) = name.split_once('.')?;
            return theme_style(first);
        }
    };
    Some(style)
}

/// A language ready to highlight: its parser's language and its compiled queries.
struct Compiled {
    language: Language,
    query: Query,
    /// The injected languages' query, `None` when the language injects nothing.
    injections: Option<Query>,
    /// The highlights query alone, used when this language is injected into another.
    highlights: Query,
}

/// Highlighters for the languages of one export, each compiled on first use.
#[derive(Default)]
pub struct Highlighter {
    compiled: HashMap<&'static str, Option<Compiled>>,
}

/// A capture: the bytes it covers and its name.
struct Capture {
    range: Range<usize>,
    name: String,
}

impl Highlighter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a language has a grammar, under any name the preview accepts for it.
    #[must_use]
    pub fn supports(language: &str) -> bool {
        grammar(&language.to_ascii_lowercase()).is_some()
    }

    fn compiled(&mut self, language: &str) -> Option<&Compiled> {
        let grammar = grammar(&language.to_ascii_lowercase())?;
        self.compiled
            .entry(grammar.name)
            .or_insert_with(|| compile(&grammar))
            .as_ref()
    }

    /// `code`'s runs in order, together covering it, each with its style; `None` when the
    /// language has no grammar or the code cannot be parsed.
    pub fn highlight(&mut self, language: &str, code: &str) -> Option<Vec<(Range<usize>, Style)>> {
        let compiled = self.compiled(language)?;
        let tree = parse(&compiled.language, code, None)?;
        let mut captures = captures(&compiled.query, &tree, code, None);
        let injected = compiled
            .injections
            .as_ref()
            .map(|query| injections(query, &tree, code))
            .unwrap_or_default();
        let mut injected_captures = Vec::new();
        for (name, ranges) in injected {
            let Some(layer) = self.compiled(&name) else {
                continue;
            };
            let Some(tree) = parse(&layer.language, code, Some(&ranges)) else {
                continue;
            };
            let span =
                ranges.first().map_or(0, |r| r.start_byte)..ranges.last().map_or(0, |r| r.end_byte);
            injected_captures.extend(captures_without_overlap(
                &layer.highlights,
                &tree,
                code,
                span,
            ));
        }
        captures.extend(injected_captures);
        let styled = captures
            .into_iter()
            .filter(|capture| !capture.range.is_empty())
            .map(|capture| {
                (
                    capture.range,
                    theme_style(&capture.name).unwrap_or_default(),
                )
            })
            .collect();
        Some(unique_styles(0..code.len(), styled))
    }
}

fn compile(grammar: &Grammar) -> Option<Compiled> {
    // As gpui-component: one query of injections, locals and highlights, the injection patterns
    // disabled; locals' captures style nothing, but take the name of a range they share.
    let source = format!(
        "{}{}{}",
        grammar.injections, grammar.locals, grammar.highlights
    );
    let mut query = Query::new(&grammar.language, &source).ok()?;
    let locals_start = grammar.injections.len();
    for index in 0..query.pattern_count() {
        if query.start_byte_for_pattern(index) < locals_start {
            query.disable_pattern(index);
        }
    }
    let injections = if grammar.injections.is_empty() {
        None
    } else {
        Query::new(&grammar.language, grammar.injections).ok()
    };
    let highlights = Query::new(&grammar.language, grammar.highlights).ok()?;
    Some(Compiled {
        language: grammar.language.clone(),
        query,
        injections,
        highlights,
    })
}

fn parse(language: &Language, code: &str, ranges: Option<&[tree_sitter::Range]>) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    if let Some(ranges) = ranges {
        parser.set_included_ranges(ranges).ok()?;
    }
    parser.parse(code, None)
}

/// Every capture of `query` in match order; a capture of the same bytes as the one before it
/// takes that one's name, as gpui-component keeps the first.
fn captures(query: &Query, tree: &Tree, code: &str, span: Option<Range<usize>>) -> Vec<Capture> {
    let mut cursor = QueryCursor::new();
    if let Some(span) = span {
        cursor.set_byte_range(span);
    }
    let names = query.capture_names();
    let mut out: Vec<Capture> = Vec::new();
    let mut matches = cursor.matches(query, tree.root_node(), code.as_bytes());
    while let Some(found) = matches.next() {
        for capture in found.captures {
            let range = capture.node.start_byte()..capture.node.end_byte();
            let name = match out.last() {
                Some(last) if last.range == range => last.name.clone(),
                _ => names[capture.index as usize].to_owned(),
            };
            out.push(Capture { range, name });
        }
    }
    out
}

/// An injected layer's captures, skipping any that starts inside the one before it unless its
/// pattern allows overlap, as gpui-component highlights injections.
fn captures_without_overlap(
    query: &Query,
    tree: &Tree,
    code: &str,
    span: Range<usize>,
) -> Vec<Capture> {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(span);
    let names = query.capture_names();
    let mut out = Vec::new();
    let mut last_end = 0;
    let mut matches = cursor.matches(query, tree.root_node(), code.as_bytes());
    while let Some(found) = matches.next() {
        let overlap = query
            .property_settings(found.pattern_index)
            .iter()
            .any(|property| property.key.as_ref() == "highlight.allow-overlap");
        for capture in found.captures {
            let range = capture.node.start_byte()..capture.node.end_byte();
            if !overlap && range.start < last_end {
                continue;
            }
            if !overlap {
                last_end = range.end;
            }
            out.push(Capture {
                range,
                name: names[capture.index as usize].to_owned(),
            });
        }
    }
    out
}

/// The injected languages and the ranges each covers, in order of their first range.
fn injections(query: &Query, tree: &Tree, code: &str) -> Vec<(String, Vec<tree_sitter::Range>)> {
    let names = query.capture_names();
    let content = names.iter().position(|name| *name == "injection.content");
    let language = names.iter().position(|name| *name == "injection.language");
    let mut layers: Vec<(String, Vec<tree_sitter::Range>)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), code.as_bytes());
    while let Some(found) = matches.next() {
        let mut name = query
            .property_settings(found.pattern_index)
            .iter()
            .find(|property| property.key.as_ref() == "injection.language")
            .and_then(|property| property.value.as_deref().map(str::to_owned));
        let mut node = None;
        for capture in found.captures {
            let index = Some(capture.index as usize);
            if index == content {
                node = Some(capture.node);
            } else if index == language {
                name = capture
                    .node
                    .utf8_text(code.as_bytes())
                    .ok()
                    .map(str::to_owned);
            }
        }
        let (Some(name), Some(node)) = (name, node) else {
            continue;
        };
        if grammar(&name.to_ascii_lowercase()).is_none() {
            continue;
        }
        layers.push((name, vec![node.range()]));
    }
    layers
}

/// Overlapping styles resolved as gpui-component resolves them: ranges sorted by where they
/// start, each later one's colour and weight winning where they overlap, and the gaps between
/// them unstyled; adjacent runs of one style are joined unless a style ended between them.
fn unique_styles(
    total: Range<usize>,
    styles: Vec<(Range<usize>, Style)>,
) -> Vec<(Range<usize>, Style)> {
    let mut intervals: Vec<(usize, bool, usize)> = Vec::with_capacity(styles.len() * 2 + 2);
    for (index, (range, _)) in styles.iter().enumerate() {
        intervals.push((range.start, true, index));
        intervals.push((range.end, false, index));
    }
    intervals.push((total.start, true, usize::MAX));
    intervals.push((total.end, false, usize::MAX));
    intervals.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let ends: BTreeSet<usize> = styles.iter().map(|(range, _)| range.end).collect();

    let mut runs: Vec<(Range<usize>, Style)> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut last = total.start;
    for (position, start, index) in intervals {
        if position > last {
            let mut combined = Style::default();
            for &active in &active {
                let style = styles[active].1;
                if style.color.is_some() {
                    combined.color = style.color;
                }
                combined.bold |= style.bold;
                combined.italic |= style.italic;
            }
            runs.push((last..position, combined));
        }
        if index != usize::MAX {
            if start {
                active.push(index);
            } else {
                active.retain(|&other| other != index);
            }
        }
        last = position;
    }

    let mut merged: Vec<(Range<usize>, Style)> = Vec::with_capacity(runs.len());
    for (range, style) in runs {
        if let Some((last_range, last_style)) = merged.last_mut() {
            if last_range.end == range.start && *last_style == style && !ends.contains(&range.start)
            {
                last_range.end = range.end;
                continue;
            }
        }
        merged.push((range, style));
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::{Highlighter, Style};

    fn color_of(
        runs: &[(std::ops::Range<usize>, Style)],
        code: &str,
        word: &str,
    ) -> Option<[u8; 3]> {
        let at = code.find(word).unwrap();
        runs.iter()
            .find(|(range, _)| range.contains(&at))
            .and_then(|(_, style)| style.color)
    }

    #[test]
    fn rust_tokens_take_the_preview_s_light_colours() {
        let code = "// A comment\nfn main() {\n    let answer = \"forty-two\";\n}\n";
        let runs = Highlighter::new().highlight("rust", code).unwrap();
        assert_eq!(runs.first().unwrap().0.start, 0);
        assert_eq!(runs.last().unwrap().0.end, code.len());
        assert!(runs.windows(2).all(|pair| pair[0].0.end == pair[1].0.start));
        assert_eq!(color_of(&runs, code, "fn"), Some([0x04, 0x33, 0xFF]));
        assert_eq!(color_of(&runs, code, "\"forty"), Some([0x03, 0x6A, 0x07]));
        assert_eq!(color_of(&runs, code, "// A"), Some([0x00, 0x7F, 0xFF]));
    }

    #[test]
    fn languages_are_found_by_the_preview_s_names() {
        for name in [
            "rust", "rs", "Python", "js", "ts", "yml", "sh", "jsonc", "sql", "toml", "go", "java",
            "html",
        ] {
            assert!(Highlighter::supports(name), "{name}");
            let runs = Highlighter::new().highlight(name, "x").unwrap();
            assert_eq!(runs.last().unwrap().0.end, 1, "{name}");
        }
        assert!(!Highlighter::supports("xml"));
        assert!(Highlighter::new().highlight("xml", "<a/>").is_none());
    }

    #[test]
    fn half_typed_code_still_highlights() {
        for (language, code) in [
            ("rust", "fn main( {"),
            ("python", "def f(:\n  '"),
            ("json", "{\"a\": [1,"),
        ] {
            let runs = Highlighter::new().highlight(language, code).unwrap();
            assert_eq!(runs.last().unwrap().0.end, code.len(), "{language}");
        }
    }
}
