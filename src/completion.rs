use adoc_core::SourceRange;

use crate::line_parser::{content, is_boundary, is_verbatim_delimiter, COMMENT_DELIMITER};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionKind {
    XrefTarget,
    /// An anchor inside `target`. An empty target means the current file.
    XrefAnchor {
        target: String,
    },
    IncludeTarget,
    ImageTarget,
    LocalAnchor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionContext {
    pub kind: CompletionKind,
    /// What the author has typed between the start of the target and the cursor.
    pub prefix: String,
    /// The span an accepted completion replaces.
    pub range: SourceRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marker {
    Xref,
    Include,
    Image,
    Anchor,
}

/// What the cursor sits inside, or `None` when it sits inside nothing completable.
///
/// This deliberately does not reuse `Document::references`: the parser records only
/// complete macros, and completion happens precisely while one is half-typed.
#[must_use]
pub fn completion_context(text: &str, offset: usize) -> Option<CompletionContext> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }

    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let typed = &text[line_start..offset];

    match block_state(text, line_start) {
        // Asciidoctor expands nothing inside a comment block.
        BlockState::Comment => None,
        // Asciidoctor processes includes inside listing, literal and passthrough blocks,
        // matching `parse_document`. Everything else in the block is content.
        BlockState::Verbatim => line_context(typed, line_start, offset, true),
        BlockState::Body => line_context(typed, line_start, offset, false),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockState {
    Body,
    Verbatim,
    Comment,
}

fn block_state(text: &str, line_start: usize) -> BlockState {
    let mut active: Option<&str> = None;
    for line in text[..line_start].split_inclusive('\n') {
        let trimmed = content(line).trim();
        match active {
            Some(delimiter) if trimmed == delimiter => active = None,
            Some(_) => {}
            None if is_verbatim_delimiter(trimmed) => active = Some(trimmed),
            None => {}
        }
    }
    match active {
        None => BlockState::Body,
        Some(COMMENT_DELIMITER) => BlockState::Comment,
        Some(_) => BlockState::Verbatim,
    }
}

fn line_context(
    typed: &str,
    line_start: usize,
    offset: usize,
    includes_only: bool,
) -> Option<CompletionContext> {
    let mut best: Option<(usize, usize, Marker)> = None;

    for (prefix, marker) in [
        ("include::", Marker::Include),
        ("xref:", Marker::Xref),
        ("image:", Marker::Image),
    ] {
        if includes_only && marker != Marker::Include {
            continue;
        }
        let Some(start) = last_macro_start(typed, prefix) else {
            continue;
        };
        let mut target_start = start + prefix.len();
        // `image::` is the block form of the same macro.
        if marker == Marker::Image && typed[target_start..].starts_with(':') {
            target_start += 1;
        }
        if best.is_none_or(|(existing, _, _)| start > existing) {
            best = Some((start, target_start, marker));
        }
    }

    if !includes_only {
        if let Some(start) = typed.rfind("<<") {
            if best.is_none_or(|(existing, _, _)| start > existing) {
                best = Some((start, start + 2, Marker::Anchor));
            }
        }
    }

    let (_, target_start, marker) = best?;
    let target = &typed[target_start..];
    // A `[` closes the target, and no target spans whitespace: the cursor has left it.
    if target.contains('[') || target.chars().any(char::is_whitespace) {
        return None;
    }
    let range_start = line_start + target_start;

    let kind = match marker {
        Marker::Anchor => {
            if target.contains(">>") || target.contains(',') {
                return None;
            }
            CompletionKind::LocalAnchor
        }
        Marker::Include => CompletionKind::IncludeTarget,
        Marker::Image => CompletionKind::ImageTarget,
        Marker::Xref => {
            if let Some((file, anchor)) = target.split_once('#') {
                return Some(CompletionContext {
                    kind: CompletionKind::XrefAnchor {
                        target: file.to_owned(),
                    },
                    prefix: anchor.to_owned(),
                    range: SourceRange::new(range_start + file.len() + 1, offset),
                });
            }
            CompletionKind::XrefTarget
        }
    };

    Some(CompletionContext {
        kind,
        prefix: target.to_owned(),
        range: SourceRange::new(range_start, offset),
    })
}

/// The last occurrence of `prefix` that starts a macro rather than sitting inside a word
/// or behind an escaping backslash.
fn last_macro_start(typed: &str, prefix: &str) -> Option<usize> {
    let mut best = None;
    let mut base = 0;
    while let Some(found) = typed[base..].find(prefix) {
        let start = base + found;
        if is_boundary(typed, start) {
            best = Some(start);
        }
        base = start + prefix.len();
    }
    best
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;

    use super::{completion_context, CompletionKind};

    #[test]
    fn detects_an_xref_target() {
        let text = "= Demo\n\nSee xref:get";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::XrefTarget);
        assert_eq!(context.prefix, "get");
        assert_eq!(context.range, SourceRange::new(text.len() - 3, text.len()));
    }

    #[test]
    fn detects_an_anchor_after_a_hash() {
        let text = "See xref:other.adoc#det";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(
            context.kind,
            CompletionKind::XrefAnchor {
                target: "other.adoc".to_owned()
            }
        );
        assert_eq!(context.prefix, "det");
    }

    #[test]
    fn treats_a_leading_hash_as_an_anchor_in_this_file() {
        let text = "See xref:#det";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(
            context.kind,
            CompletionKind::XrefAnchor {
                target: String::new()
            }
        );
        assert_eq!(context.prefix, "det");
    }

    #[test]
    fn detects_an_include_target() {
        let text = "include::partial$no";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::IncludeTarget);
        assert_eq!(context.prefix, "partial$no");
    }

    #[test]
    fn detects_block_and_inline_image_targets() {
        for text in ["image::arch", "Shown image:arch"] {
            let context = completion_context(text, text.len()).expect("a context");

            assert_eq!(context.kind, CompletionKind::ImageTarget);
            assert_eq!(context.prefix, "arch");
        }
    }

    #[test]
    fn detects_a_local_anchor() {
        let text = "See <<int";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::LocalAnchor);
        assert_eq!(context.prefix, "int");
    }

    #[test]
    fn offers_nothing_inside_a_comment_block() {
        let text = "= Demo\n\n////\nSee xref:get";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn offers_only_includes_inside_a_verbatim_block() {
        let source_block = "= Demo\n\n----\ninclude::example$q";
        let context = completion_context(source_block, source_block.len()).expect("a context");
        assert_eq!(context.kind, CompletionKind::IncludeTarget);

        let xref_in_block = "= Demo\n\n----\nSee xref:get";
        assert_eq!(completion_context(xref_in_block, xref_in_block.len()), None);
    }

    #[test]
    fn resumes_after_a_verbatim_block_closes() {
        let text = "= Demo\n\n----\ncode\n----\n\nSee xref:get";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::XrefTarget);
    }

    #[test]
    fn ignores_an_escaped_macro() {
        let text = "\\xref:get";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn ignores_a_prefix_inside_a_word() {
        let text = "myxref:get";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn stops_at_the_closing_bracket() {
        let text = "See xref:page.adoc[Page] and ";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn stops_at_whitespace_after_the_target() {
        let text = "See xref:page.adoc then";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn takes_the_nearest_construct_when_a_line_holds_several() {
        let text = "See xref:one.adoc[One] then include::partial$no";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::IncludeTarget);
        assert_eq!(context.prefix, "partial$no");
    }

    #[test]
    fn returns_nothing_for_an_offset_off_the_end() {
        let text = "See xref:get";

        assert_eq!(completion_context(text, text.len() + 5), None);
    }

    #[test]
    fn returns_nothing_on_an_attribute_line() {
        let text = ":toc:";

        assert_eq!(completion_context(text, text.len()), None);
    }
}
