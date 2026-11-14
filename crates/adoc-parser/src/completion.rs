use adoc_core::SourceRange;

use crate::line_parser::{
    content, find_anchors, is_block_metadata, is_boundary, is_line_comment, mask_passthroughs,
    open_passthrough, Verbatim, VerbatimBlocks,
};
use crate::table::Tables;

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
    /// The name inside an attribute reference such as `{product-name}`.
    AttributeName,
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

    match block_state(text, line_start, offset) {
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

/// Where the line starting at `line_start` falls among the blocks, the caret at `offset`: a
/// literal table cell is read as verbatim content, as `parse_document` reads it.
fn block_state(text: &str, line_start: usize, offset: usize) -> BlockState {
    let mut verbatim = VerbatimBlocks::default();
    let mut tables = Tables::default();
    let mut attributes = Vec::new();
    let before = &text[..line_start];
    let lines = before.strip_prefix('\u{feff}').unwrap_or(before);
    let mut end = before.len() - lines.len();
    for line in lines.split_inclusive('\n') {
        end += line.len();
        let line = content(line);
        // Conditional lines are removed first, as `parse_document` removes them.
        if verbatim.directive(line).is_some() {
            continue;
        }
        let kind = verbatim.line(line.trim());
        tables.line(line, kind == Verbatim::No, &attributes, &text[end..]);
        keep_block_attributes(&mut attributes, line, kind);
    }
    match verbatim.next() {
        Verbatim::Comment => BlockState::Comment,
        Verbatim::Content => BlockState::Verbatim,
        Verbatim::No | Verbatim::Delimiter | Verbatim::Underline(_) => {
            let rest = &text[line_start..];
            let rest = match rest.strip_prefix('\u{feff}') {
                Some(stripped) if line_start == 0 => stripped,
                _ => rest,
            };
            let caret = offset - (text.len() - rest.len()).min(offset);
            let line = content(rest.split_inclusive('\n').next().unwrap_or_default());
            let literal = if verbatim.directive(line).is_some() {
                Vec::new()
            } else {
                // The line cannot open a table here: the caret on `|===` completes nothing.
                tables.line(line, false, &attributes, "")
            };
            if literal
                .iter()
                .any(|&(start, end)| start <= caret && caret <= end)
            {
                BlockState::Verbatim
            } else {
                BlockState::Body
            }
        }
    }
}

/// Keeps the block attribute lists since the last block as `parse_document` keeps them, for
/// the `cols` and options of a table that opens: a block attribute line adds its list, and
/// other block metadata, a lone anchor, a line comment and a blank line keep those before.
fn keep_block_attributes(attributes: &mut Vec<String>, line: &str, kind: Verbatim) {
    let trimmed = line.trim();
    match kind {
        Verbatim::No => {}
        // A comment block is no block.
        Verbatim::Comment => return,
        Verbatim::Delimiter if trimmed.starts_with('/') => return,
        Verbatim::Delimiter | Verbatim::Content | Verbatim::Underline(_) => {
            attributes.clear();
            return;
        }
    }
    if is_line_comment(trimmed) {
        return;
    }
    if let Some(inside) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        if !trimmed.starts_with("[[") {
            attributes.push(inside.to_owned());
        }
        return;
    }
    let anchors = find_anchors(&mask_passthroughs(line), 0);
    let anchor_only = matches!(
        anchors.as_slice(),
        [anchor] if anchor.range == SourceRange::new(0, line.trim_end().len())
    );
    if !trimmed.is_empty() && !anchor_only && !is_block_metadata(trimmed) {
        attributes.clear();
    }
}

fn line_context(
    typed: &str,
    line_start: usize,
    offset: usize,
    includes_only: bool,
) -> Option<CompletionContext> {
    // A name being typed after `{` is an attribute reference even inside a macro target,
    // where Asciidoctor substitutes it before resolving the target.
    let trimmed = typed.trim_start();
    // The parser records nothing on a line comment.
    if is_line_comment(trimmed) {
        return None;
    }
    if !includes_only || trimmed.starts_with("include::") {
        if let Some(context) = attribute_context(typed, line_start, offset) {
            return Some(context);
        }
    }

    // Nothing inside a passthrough is a reference: none is offered inside one closed before
    // the caret, as the parser reads none there, nor inside one still open (#532). An include
    // is a line of its own, read before passthroughs are.
    let open = open_passthrough(typed).is_some();
    let masked = mask_passthroughs(typed);
    let source = |marker: Marker| {
        if marker == Marker::Include {
            typed
        } else {
            &*masked
        }
    };
    let mut best: Option<(usize, usize, Marker)> = None;

    for (prefix, marker) in [
        ("include::", Marker::Include),
        ("xref:", Marker::Xref),
        ("image:", Marker::Image),
    ] {
        if marker != Marker::Include && (includes_only || open) {
            continue;
        }
        let Some(start) = last_macro_start(source(marker), prefix) else {
            continue;
        };
        let mut target_start = start + prefix.len();
        // `image::` is the block form of the same macro.
        if marker == Marker::Image && masked[target_start..].starts_with(':') {
            target_start += 1;
        }
        if best.is_none_or(|(existing, _, _)| start > existing) {
            best = Some((start, target_start, marker));
        }
    }

    if !includes_only && !open {
        // `<<<` is a page break, and `\<<` shows the brackets as written.
        if let Some(start) = masked
            .rfind("<<")
            .filter(|&start| !masked[..start].ends_with(['<', '\\']))
        {
            if best.is_none_or(|(existing, _, _)| start > existing) {
                best = Some((start, start + 2, Marker::Anchor));
            }
        }
    }

    let (_, target_start, marker) = best?;
    let target = &source(marker)[target_start..];
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
                        // `other#` names `other.adoc`, as Asciidoctor reads it.
                        target: adoc_core::xref_file(target).into_owned(),
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

fn attribute_context(typed: &str, line_start: usize, offset: usize) -> Option<CompletionContext> {
    let brace = typed.rfind('{')?;
    if typed[..brace].ends_with('\\') {
        return None;
    }
    let name = &typed[brace + 1..];
    let is_name_character =
        |character: char| character.is_alphanumeric() || character == '_' || character == '-';
    if name.starts_with('-') || !name.chars().all(is_name_character) {
        return None;
    }

    Some(CompletionContext {
        kind: CompletionKind::AttributeName,
        prefix: name.to_owned(),
        range: SourceRange::new(line_start + brace + 1, offset),
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
    fn reads_a_delimiter_on_the_first_line_after_a_byte_order_mark() {
        let text = "\u{feff}----\nSee xref:get";

        assert_eq!(completion_context(text, text.len()), None);
    }

    /// A two-line title's underline opens no listing block (#526).
    #[test]
    fn completes_after_a_two_line_title() {
        let text = "Section\n-------\n\nSee xref:get";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::XrefTarget);
    }

    /// Nothing is completed on a line comment, where the parser records nothing (#532).
    #[test]
    fn offers_nothing_on_a_comment_line() {
        let text = "// TODO xref:ge";

        assert_eq!(completion_context(text, text.len()), None);
    }

    /// `<<<` is a page break, and `\<<` shows the brackets as written (#532).
    #[test]
    fn offers_no_anchor_after_a_page_break_or_an_escape() {
        for text in ["<<<", "\\<<int"] {
            assert_eq!(completion_context(text, text.len()), None, "{text}");
        }
    }

    /// Nothing inside a passthrough is a reference, so while one opened earlier on the line is
    /// still unclosed no target is offered, though an include still is (#532).
    #[test]
    fn offers_no_reference_inside_a_half_typed_passthrough() {
        for text in [
            "See +xref:ge",
            "(+xref:ge",
            "pass:[xref:ge",
            "pass:q,a[xref:ge",
            "+++xref:ge",
            "See ++xref:ge",
            "$$xref:ge",
            "pass:[xref:other.adoc#det",
            "See +<<int",
            "pass:[<<int",
            "+++image:arch",
            "$$image::arch",
        ] {
            assert_eq!(completion_context(text, text.len()), None, "{text}");
        }

        let text = "include::partial$no";
        let context = completion_context(text, text.len()).expect("a context");
        assert_eq!(context.kind, CompletionKind::IncludeTarget);
    }

    /// A plus sign that cannot open a constrained passthrough, after a word character or
    /// before a space, leaves the line to complete as before, as does an escaped opening.
    #[test]
    fn completes_after_plus_signs_that_open_no_passthrough() {
        for (text, kind) in [
            ("C++ xref:ge", CompletionKind::XrefTarget),
            ("Use C++ or <<int", CompletionKind::LocalAnchor),
            ("a + b xref:ge", CompletionKind::XrefTarget),
            ("1+xref:ge", CompletionKind::XrefTarget),
            ("\\+xref:ge", CompletionKind::XrefTarget),
            ("\\pass:[xref:ge", CompletionKind::XrefTarget),
            ("pass: [xref:ge", CompletionKind::XrefTarget),
            ("costs $5 image:arch", CompletionKind::ImageTarget),
        ] {
            let context = completion_context(text, text.len()).expect(text);
            assert_eq!(context.kind, kind, "{text}");
        }
    }

    /// A closed passthrough reads as the parser reads it: nothing inside it completes, and
    /// what follows it does.
    #[test]
    fn a_closed_passthrough_hides_only_what_it_holds() {
        for text in [
            "See +xref:a+",
            "pass:[xref:a]",
            "+++xref:a+++",
            "$$xref:a$$",
        ] {
            assert_eq!(completion_context(text, text.len()), None, "{text}");
        }
        for (text, kind, prefix) in [
            ("+xref:a+ then xref:ge", CompletionKind::XrefTarget, "ge"),
            ("pass:[xref:a] xref:ge", CompletionKind::XrefTarget, "ge"),
            ("$$a$$ <<int", CompletionKind::LocalAnchor, "int"),
            ("++<<a>>++ image:arch", CompletionKind::ImageTarget, "arch"),
        ] {
            let context = completion_context(text, text.len()).expect(text);
            assert_eq!(context.kind, kind, "{text}");
            assert_eq!(context.prefix, prefix, "{text}");
        }
    }

    /// A literal table cell is shown as written, so only an include completes in it, as in a
    /// listing block (#532).
    #[test]
    fn offers_only_includes_in_a_literal_table_cell() {
        for text in [
            "|===\n|a l|xref:ge",
            "|===\nl|first\nsecond <<int",
            "[cols=\"1,1l\"]\n|===\n|a |xref:ge",
            "[cols=\"1,1l\"]\n.Title\n\n|===\n|a |image:arch",
            "|===\nl|{prod",
        ] {
            assert_eq!(completion_context(text, text.len()), None, "{text}");
        }

        // The caret in a literal cell with more cells after it.
        let text = "|===\nl|xref:ge |b\n|===\n";
        let offset = text.find(" |b").expect("the next cell");
        assert_eq!(completion_context(text, offset), None);

        let text = "|===\nl|x\ninclude::partial$no";
        let context = completion_context(text, text.len()).expect("a context");
        assert_eq!(context.kind, CompletionKind::IncludeTarget);
    }

    /// Other cells, the header row, text after the table and a `|===` inside a listing block
    /// complete as before.
    #[test]
    fn completes_outside_literal_table_cells() {
        for text in [
            "|===\nl|a |xref:ge",
            "[%header]\n|===\nl|xref:ge",
            "[cols=\"1,1l\"]\n|===\n|a |b\n\n|c d|xref:ge",
            "|===\nl|<<a>>\n|===\n\nSee xref:ge",
            "----\n|===\n----\nl|xref:ge",
            "[cols=\"1,1l\"]\n\nSee xref:a[]\n\n|===\n|a |xref:ge",
        ] {
            let context = completion_context(text, text.len()).expect(text);
            assert_eq!(context.kind, CompletionKind::XrefTarget, "{text}");
        }
    }

    /// Conditional lines are removed before blocks are read, as the parser removes them, so
    /// a styled block inside one ends where it would without them (#532).
    #[test]
    fn conditional_lines_are_skipped_as_the_parser_skips_them() {
        let text =
            "[source]\nifdef::x[]\n----\ncode\n\nmore lines\n----\nendif::[]\n\nSee xref:get";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::XrefTarget);
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
    fn offers_nothing_but_includes_in_longer_blocks_fences_and_styled_paragraphs() {
        for opening in [
            "------\n----\n----\n",
            "```ruby\n",
            "[source,ruby]\n",
            "[literal]\nfirst line\n",
        ] {
            let text = format!("= Demo\n\n{opening}See xref:get");
            assert_eq!(completion_context(&text, text.len()), None, "{opening}");

            let text = format!("= Demo\n\n{opening}include::example$q");
            let context = completion_context(&text, text.len()).expect("a context");
            assert_eq!(context.kind, CompletionKind::IncludeTarget, "{opening}");
        }
    }

    #[test]
    fn offers_nothing_in_a_longer_comment_block() {
        let text = "= Demo\n\n//////\n////\n////\ninclude::example$q";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn resumes_after_a_styled_paragraph_ends() {
        let text = "= Demo\n\n[source]\ncode\n\nSee xref:get";
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

    #[test]
    fn detects_an_attribute_name() {
        let text = "= Demo\n\nVersion {prod";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::AttributeName);
        assert_eq!(context.prefix, "prod");
        assert_eq!(context.range, SourceRange::new(text.len() - 4, text.len()));
    }

    #[test]
    fn detects_an_attribute_name_straight_after_the_brace() {
        let text = "Version {";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::AttributeName);
        assert_eq!(context.prefix, "");
    }

    #[test]
    fn prefers_an_attribute_name_inside_a_macro_target() {
        for text in ["include::{par", "See xref:{bas", "image::{img"] {
            let context = completion_context(text, text.len()).expect("a context");

            assert_eq!(context.kind, CompletionKind::AttributeName, "{text}");
        }
    }

    #[test]
    fn detects_an_attribute_name_in_an_attribute_value() {
        let text = ":url: {base";
        let context = completion_context(text, text.len()).expect("a context");

        assert_eq!(context.kind, CompletionKind::AttributeName);
        assert_eq!(context.prefix, "base");
    }

    #[test]
    fn ignores_a_closed_attribute_reference() {
        let text = "Version {product} and ";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn ignores_an_escaped_attribute_reference() {
        let text = "Version \\{prod";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn ignores_a_brace_followed_by_something_other_than_a_name() {
        for text in ["{a b", "{set:foo", "{-x"] {
            assert_eq!(completion_context(text, text.len()), None, "{text}");
        }
    }

    #[test]
    fn ignores_an_attribute_name_on_a_comment_line() {
        let text = "// see {prod";

        assert_eq!(completion_context(text, text.len()), None);
    }

    #[test]
    fn offers_attribute_names_inside_a_verbatim_block_only_on_include_lines() {
        let plain = "----\nVersion {prod";
        assert_eq!(completion_context(plain, plain.len()), None);

        let include = "----\ninclude::{par";
        let context = completion_context(include, include.len()).expect("a context");
        assert_eq!(context.kind, CompletionKind::AttributeName);
    }
}
