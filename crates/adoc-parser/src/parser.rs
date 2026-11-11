use std::collections::HashMap;

use adoc_core::{
    Anchor, Diagnostic, DiagnosticCode, DiagnosticSeverity, Document, DocumentTitle,
    IncludeDirective, Section, SourceRange,
};

use crate::line_parser::{
    conditional_directive, content, find_anchors, find_images, find_includes, find_references,
    mask_passthroughs, parse_attribute, parse_heading, Conditional, Verbatim, VerbatimBlocks,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseResult {
    pub document: Document,
}

impl ParseResult {
    /// The parse's diagnostics, which the document carries.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.document.diagnostics
    }
}

pub trait DocumentParser: Send + Sync {
    fn parse(&self, uri: &str, text: &str) -> ParseResult;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AsciiDocParser;

impl DocumentParser for AsciiDocParser {
    fn parse(&self, uri: &str, text: &str) -> ParseResult {
        parse_document(uri, text)
    }
}

#[must_use]
pub fn parse(uri: &str, text: &str) -> ParseResult {
    adoc_core::probe::hit(adoc_core::probe::Probe::Parse);
    AsciiDocParser.parse(uri, text)
}

fn parse_document(uri: &str, text: &str) -> ParseResult {
    let mut document = Document::new(uri, text);
    let mut verbatim = VerbatimBlocks::default();
    let mut pending_anchor: Option<String> = None;
    // How many `ifdef`, `ifndef` and `ifeval` blocks are open.
    let mut conditionals = 0_usize;
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        let line_content = content(line);
        let trimmed = line_content.trim();

        // The preprocessor reads conditionals everywhere but in comment blocks, and removes
        // their lines before the parser sees them.
        if verbatim.next() != Verbatim::Comment {
            if let Some(directive) = conditional_directive(line_content) {
                conditionals = match directive {
                    Conditional::Open => conditionals + 1,
                    Conditional::Close => conditionals.saturating_sub(1),
                };
                offset += line.len();
                continue;
            }
        }
        let conditional = conditionals > 0;
        let includes = |line: &str, offset: usize| {
            find_includes(line, offset)
                .into_iter()
                .map(move |include| IncludeDirective {
                    conditional,
                    ..include
                })
        };

        match verbatim.line(trimmed) {
            Verbatim::No => {}
            Verbatim::Delimiter => {
                pending_anchor = None;
                offset += line.len();
                continue;
            }
            kind => {
                // Asciidoctor processes includes inside listing, literal and passthrough
                // blocks — a code sample pulled into a `[source]` block is the commonest
                // use of the `example$` family. Everything else in the block is content.
                if kind == Verbatim::Content {
                    document.includes.extend(includes(line, offset));
                }
                pending_anchor = None;
                offset += line.len();
                continue;
            }
        }

        if trimmed.starts_with("//") {
            pending_anchor = None;
            offset += line.len();
            continue;
        }

        // Inline extraction sees passthroughs blanked out; includes are whole-line directives.
        let inline = mask_passthroughs(line);
        let anchors = find_anchors(&inline, offset);
        let anchor_only = anchors.len() == 1
            && anchors[0].range == SourceRange::new(offset, offset + line_content.len());
        for mut anchor in anchors {
            anchor.conditional = conditional;
            if anchor_only {
                pending_anchor = Some(anchor.id.clone());
            }
            document.anchors.push(anchor);
        }

        if let Some(heading) = parse_heading(line, offset) {
            if heading.marker_level == 1 && document.title.is_none() {
                document.title = Some(DocumentTitle {
                    text: heading.title.to_owned(),
                    range: heading.range,
                    selection_range: heading.selection_range,
                });
                pending_anchor = None;
            } else {
                document.sections.push(Section {
                    level: heading.marker_level.saturating_sub(1),
                    title: heading.title.to_owned(),
                    id: pending_anchor.take(),
                    range: heading.range,
                    selection_range: heading.selection_range,
                });
            }
        } else if let Some(attribute) = parse_attribute(line, offset) {
            document.attributes.push(attribute);
            pending_anchor = None;
        } else if !trimmed.is_empty() && !anchor_only {
            pending_anchor = None;
        }

        document.references.extend(find_references(&inline, offset));
        document.includes.extend(includes(line, offset));
        document.images.extend(find_images(&inline, offset));
        offset += line.len();
    }

    document.diagnostics = duplicate_anchors(&document.anchors);
    ParseResult { document }
}

/// A warning for each anchor whose id an earlier one declared. Anchors inside conditionals
/// are left out: two branches commonly declare the same id, and only one is ever built.
fn duplicate_anchors(anchors: &[Anchor]) -> Vec<Diagnostic> {
    let mut first = HashMap::<&str, SourceRange>::new();
    let mut diagnostics = Vec::new();
    for anchor in anchors.iter().filter(|anchor| !anchor.conditional) {
        match first.get(anchor.id.as_str()) {
            Some(first_range) => diagnostics.push(Diagnostic {
                code: DiagnosticCode::DuplicateAnchor,
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "duplicate anchor `{}`; first declared at byte {}",
                    anchor.id, first_range.start
                ),
                range: anchor.range,
            }),
            None => {
                first.insert(&anchor.id, anchor.range);
            }
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use adoc_core::{ReferenceKind, SourceRange};

    use super::parse;

    #[test]
    fn parses_initial_semantic_surface() {
        let source = "= Product Guide\n\
:toc: left\n\
\n\
[[intro]]\n\
== Introduction\n\
\n\
See xref:other.adoc#details[Details] and <<intro,above>>.\n\
include::partials/setup.adoc[leveloffset=+1]\n\
image::diagram.png[Architecture]\n";

        let result = parse("file:///guide.adoc", source);
        let document = result.document;

        assert_eq!(
            document.title.as_ref().map(|title| title.text.as_str()),
            Some("Product Guide")
        );
        assert_eq!(document.sections[0].title, "Introduction");
        assert_eq!(document.sections[0].level, 1);
        assert_eq!(document.sections[0].id.as_deref(), Some("intro"));
        assert_eq!(document.attributes[0].name, "toc");
        assert_eq!(document.attributes[0].value.as_deref(), Some("left"));
        assert_eq!(document.anchors[0].id, "intro");
        assert_eq!(document.references.len(), 2);
        assert_eq!(document.references[0].kind, ReferenceKind::Xref);
        assert_eq!(document.references[0].target, "other.adoc#details");
        assert_eq!(document.references[1].kind, ReferenceKind::LocalAnchor);
        assert_eq!(document.includes[0].target, "partials/setup.adoc");
        assert_eq!(document.images[0].target, "diagram.png");
        assert!(document.diagnostics.is_empty());
    }

    #[test]
    fn treats_extensionless_xref_targets_as_id_references() {
        let source = "= Demo\n\nSee xref:smtp[SMTP], xref:#later[] and xref:page.adoc[Page].\n";
        let document = parse("file:///demo.adoc", source).document;

        let kinds = document
            .references
            .iter()
            .map(|reference| (reference.kind, reference.target.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                (ReferenceKind::LocalAnchor, "smtp"),
                (ReferenceKind::LocalAnchor, "later"),
                (ReferenceKind::Xref, "page.adoc"),
            ]
        );
    }

    fn anchor_ids(source: &str) -> Vec<String> {
        parse("file:///doc.adoc", source)
            .document
            .anchors
            .into_iter()
            .map(|anchor| anchor.id)
            .collect()
    }

    #[test]
    fn anchor_shorthand_stops_at_role() {
        assert_eq!(anchor_ids("[#intro.lead]\n== Intro\n"), ["intro"]);
        assert_eq!(anchor_ids("[#intro,role=lead]\n== Intro\n"), ["intro"]);
        // Inline, before marked text.
        assert_eq!(anchor_ids("Some [#key.big]#word# here.\n"), ["key"]);
    }

    #[test]
    fn anchor_shorthand_stops_at_option() {
        assert_eq!(
            anchor_ids("[#notes%collapsible]\n====\nHidden.\n====\n"),
            ["notes"]
        );
    }

    #[test]
    fn anchor_long_form_id_attribute() {
        assert_eq!(anchor_ids("[id=intro]\n== Intro\n"), ["intro"]);
        assert_eq!(
            anchor_ids("[source,id=\"code\"]\n----\nx\n----\n"),
            ["code"]
        );
        assert_eq!(
            anchor_ids("[quote, id='said', Someone]\n____\nHi.\n____\n"),
            ["said"]
        );
        // Only a block attribute line; prose mentioning one is not an anchor.
        assert!(anchor_ids("Write [id=intro] above a block.\n").is_empty());
        assert!(anchor_ids("[id=]\n").is_empty());
    }

    #[test]
    fn reads_bibliography_anchors() {
        let source = "= Refs\n\nSee <<GOOGLE-LIMITS>>.\n\n[bibliography]\n- [[[GOOGLE-LIMITS, 2]]] Google sending limits\n";
        let document = parse("file:///refs.adoc", source).document;

        assert_eq!(document.anchors.len(), 1);
        assert_eq!(document.anchors[0].id, "GOOGLE-LIMITS");
    }

    /// A block image starts its line, or a table cell; mentioned in prose or a list item it is
    /// text, and Asciidoctor's inline `image:` cannot take a target starting with `:` either.
    #[test]
    fn block_images_start_a_line_or_a_table_cell() {
        let source = "= Demo\n\nimage::real.png[]\n* `image::example.png[]` is how to write one\n\
                      |===\n| image::cell.png[]\na| image::styled.png[]\n|===\n";
        let targets: Vec<_> = parse("file:///doc.adoc", source)
            .document
            .images
            .into_iter()
            .map(|image| image.target)
            .collect();
        assert_eq!(targets, ["real.png", "cell.png", "styled.png"]);
    }

    /// The images a source records, as (target, inline, attributes).
    fn images(source: &str) -> Vec<(String, bool, Option<String>)> {
        parse("file:///doc.adoc", source)
            .document
            .images
            .into_iter()
            .map(|image| (image.target, image.inline, image.attributes))
            .collect()
    }

    fn inline(target: &str, attributes: Option<&str>) -> (String, bool, Option<String>) {
        (target.to_owned(), true, attributes.map(str::to_owned))
    }

    #[test]
    fn records_inline_images_in_prose() {
        let source = "= Demo\n\nClick image:icons/save.png[Save] to save.\n";
        let document = parse("file:///doc.adoc", source).document;

        assert_eq!(
            images(source),
            [inline("icons/save.png", Some("Save"))],
            "{:?}",
            document.images
        );
        let start = source.find("image:icons").unwrap();
        let end = source.find("] to").unwrap() + 1;
        assert_eq!(document.images[0].range, SourceRange::new(start, end));
    }

    #[test]
    fn records_inline_images_in_a_table_cell_and_beside_a_block_image() {
        let source = "|===\n| image:cell.png[] | b\n|===\n\nimage::block.png[Block]\n";
        assert_eq!(
            images(source),
            [
                inline("cell.png", None),
                ("block.png".to_owned(), false, Some("Block".to_owned())),
            ]
        );
    }

    /// Asciidoctor shows an image inside plain monospace (`` `image:x.png[]` `` renders an
    /// `<img>` inside `<code>`), but nothing inside a passthrough or literal monospace.
    #[test]
    fn inline_images_follow_asciidoctor_inside_monospace_and_passthroughs() {
        assert_eq!(
            images("Write `+image:literal.png[]+` or ++image:pass.png[]++ to show one.\n"),
            []
        );
        assert_eq!(
            images("The `image:shown.png[Shown]` icon.\n"),
            [inline("shown.png", Some("Shown"))]
        );
    }

    #[test]
    fn records_no_inline_images_inside_verbatim_blocks() {
        let source = "----\nimage:listing.png[]\n----\n\n....\nimage:literal.png[]\n....\n\n\
                      [source]\nimage:styled.png[]\n\n////\nimage:comment.png[]\n////\n\
                      // image:line-comment.png[]\n";
        assert_eq!(images(source), []);
    }

    #[test]
    fn half_typed_and_escaped_inline_images_record_nothing() {
        for source in [
            "image:foo\n",
            "Text image:foo and more\n",
            "image:\n",
            "image:[]\n",
            "image: spaced.png[]\n",
            "image:trailing.png [x]\n",
            "image:foo[unclosed\n",
            "\\image:escaped.png[]\n",
            "notimage:word.png[]\n",
            "image:é\n",
        ] {
            assert_eq!(images(source), [], "{source:?}");
        }
    }

    #[test]
    fn block_image_is_not_also_inline() {
        assert_eq!(
            images("image::block.png[]\n"),
            [("block.png".to_owned(), false, None)]
        );
        // A block macro in prose is neither: an inline target cannot start with `:`.
        assert_eq!(images("Text image::prose.png[] here.\n"), []);
    }

    #[test]
    fn records_several_inline_images_on_one_line() {
        assert_eq!(
            images("image:a.png[] and image:b.png[B, 20] and image:{icon}[]\n"),
            [
                inline("a.png", None),
                inline("b.png", Some("B, 20")),
                inline("{icon}", None),
            ]
        );
    }

    /// An include is a preprocessor directive: Asciidoctor recognises it only at the start of
    /// a line with nothing after its closing bracket, so prose that mentions one, an indented
    /// line and a line with trailing text are not includes.
    #[test]
    fn include_directives_must_stand_alone_at_the_start_of_a_line() {
        let source = "= Demo\n\nUse `include::partial$<file>.adoc[]` in a page.\n\
                      include::real.adoc[]\n  include::indented.adoc[]\n\
                      include::trailing.adoc[] and more\ninclude::spaced.adoc[]  \n";
        let targets: Vec<_> = parse("file:///doc.adoc", source)
            .document
            .includes
            .into_iter()
            .map(|include| include.target)
            .collect();
        assert_eq!(targets, ["real.adoc", "spaced.adoc"]);
    }

    /// Asciidoctor processes `include::` inside listing, literal and passthrough blocks —
    /// pulling a code sample into a `[source]` block is the commonest use of the
    /// `example$` family — but not inside a comment block.
    #[test]
    fn records_includes_inside_verbatim_blocks() {
        for delimiter in ["----", "....", "++++"] {
            let source = format!(
                "= Demo\n\n[source,sql]\n{delimiter}\ninclude::example$q.sql[]\n{delimiter}\n"
            );
            let document = parse("file:///demo.adoc", &source).document;

            assert_eq!(document.includes.len(), 1, "{delimiter}");
            assert_eq!(document.includes[0].target, "example$q.sql", "{delimiter}");
        }
    }

    #[test]
    fn ignores_includes_inside_comment_blocks() {
        let source = "= Demo\n\n////\ninclude::example$q.sql[]\n////\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.includes.is_empty());
    }

    #[test]
    fn ignores_an_escaped_include() {
        let source = "= Demo\n\n----\n\\include::example$q.sql[]\n----\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.includes.is_empty());
    }

    /// An include inside a verbatim block is real, but the rest of that block is content.
    #[test]
    fn still_ignores_other_semantics_inside_verbatim_blocks() {
        let source =
            "= Demo\n\n----\ninclude::example$q.sql[]\nxref:not-real.adoc[]\n[[not-real]]\n----\n";
        let document = parse("file:///demo.adoc", source).document;

        assert_eq!(document.includes.len(), 1);
        assert!(document.references.is_empty());
        assert!(document.anchors.is_empty());
    }

    #[test]
    fn ignores_semantics_inside_verbatim_blocks() {
        let source = "= Demo\n\n[source,java]\n----\nxref:not-real.adoc[]\n[[not-real]]\n----\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.references.is_empty());
        assert!(document.anchors.is_empty());
    }

    #[test]
    fn long_delimiter_hides_references() {
        let source = "= Demo\n\n------\nxref:x.adoc[]\n[[x]]\n------\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.references.is_empty());
        assert!(document.anchors.is_empty());
    }

    /// A block closes only on a line matching its own delimiter, so a longer one can show a
    /// shorter one, even half of one, as content.
    #[test]
    fn nested_delimiters_close_by_length() {
        let source = "= Demo\n\n------\n----\nxref:inner.adoc[]\n------\n\n[[later]]\n== Later\n\nSee xref:after.adoc[].\n";
        let document = parse("file:///demo.adoc", source).document;

        let anchors: Vec<_> = document.anchors.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(anchors, ["later"]);
        let targets: Vec<_> = document
            .references
            .iter()
            .map(|r| r.target.as_str())
            .collect();
        assert_eq!(targets, ["after.adoc"]);
        assert_eq!(document.sections.len(), 1);
    }

    #[test]
    fn backtick_fence_is_verbatim() {
        let source = "= Demo\n\n```ruby\nxref:not-real.adoc[]\n[[not-real]]\ninclude::example$q.rb[]\n```\n\nSee xref:real.adoc[].\n";
        let document = parse("file:///demo.adoc", source).document;

        let targets: Vec<_> = document
            .references
            .iter()
            .map(|r| r.target.as_str())
            .collect();
        assert_eq!(targets, ["real.adoc"]);
        assert!(document.anchors.is_empty());
        assert_eq!(document.includes.len(), 1);
    }

    #[test]
    fn source_styled_paragraph_is_verbatim() {
        for style in [
            "[source]",
            "[source,ruby]",
            "[listing]",
            "[literal]",
            "[source#id.role,ruby]",
        ] {
            let source = format!(
                "= Demo\n\n{style}\nxref:not-real.adoc[]\n[[not-real]]\n\nSee xref:real.adoc[].\n"
            );
            let document = parse("file:///demo.adoc", &source).document;

            let targets: Vec<_> = document
                .references
                .iter()
                .map(|r| r.target.as_str())
                .collect();
            assert_eq!(targets, ["real.adoc"], "{style}");
            assert!(document.anchors.is_empty(), "{style}");
        }
    }

    /// The style also reaches a paragraph past blank lines, a title, a comment and an anchor
    /// line, and a delimited block after it is read as the block it is.
    #[test]
    fn verbatim_style_reaches_past_titles_and_blank_lines() {
        let source = "= Demo\n\n[source]\n.Sample\n[[sample]]\n// a note\n\nxref:not-real.adoc[]\n\n[source]\n----\nxref:in-block.adoc[]\n----\nxref:real.adoc[]\n";
        let document = parse("file:///demo.adoc", source).document;

        let targets: Vec<_> = document
            .references
            .iter()
            .map(|r| r.target.as_str())
            .collect();
        assert_eq!(targets, ["real.adoc"]);
        let anchors: Vec<_> = document.anchors.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(anchors, ["sample"]);
    }

    #[test]
    fn longer_comment_blocks_hide_includes() {
        let source = "= Demo\n\n//////\n////\ninclude::example$q.sql[]\n////\n//////\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.includes.is_empty());
    }

    #[test]
    fn short_and_mixed_runs_are_not_delimiters() {
        let source = "= Demo\n\n---\nxref:a.adoc[]\n-----.\nxref:b.adoc[]\n";
        let document = parse("file:///demo.adoc", source).document;

        assert_eq!(document.references.len(), 2);
    }

    #[test]
    fn anchors_in_conditional_branches_do_not_collide() {
        let source = "= Demo\n\nifdef::backend-pdf[]\n[[intro]]\n== Introduction (print)\nendif::[]\nifndef::backend-pdf[]\n[[intro]]\n== Introduction\nendif::[]\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        assert_eq!(document.anchors.len(), 2);
        assert!(document.anchors.iter().all(|anchor| anchor.conditional));
        let ids: Vec<_> = document.sections.iter().map(|s| s.id.as_deref()).collect();
        assert_eq!(ids, [Some("intro"), Some("intro")]);
    }

    /// Duplicates outside any conditional are still reported, after a conditional one and
    /// after the conditional closes; nested conditionals close one at a time.
    #[test]
    fn unconditional_duplicates_are_still_reported() {
        let source = "[[a]]\nifdef::x[]\nifeval::[{n} > 1]\n[[b]]\nendif::[]\n[[c]]\nendif::x[]\n[[a]]\n[[b]]\n[[b]]\n[[c]]\n";
        let document = parse("file:///demo.adoc", source).document;

        let conditional: Vec<_> = document
            .anchors
            .iter()
            .map(|anchor| (anchor.id.as_str(), anchor.conditional))
            .collect();
        assert_eq!(
            conditional,
            [
                ("a", false),
                ("b", true),
                ("c", true),
                ("a", false),
                ("b", false),
                ("b", false),
                ("c", false)
            ]
        );
        let duplicates: Vec<_> = document.diagnostics.iter().map(|d| d.range.start).collect();
        let second_a = source.rfind("[[a]]").unwrap();
        let second_b = source.rfind("[[b]]").unwrap();
        assert_eq!(duplicates, [second_a, second_b]);
    }

    /// `ifdef::attr[text]` is a single line, not the start of a conditional block, and
    /// directives are lines of their own.
    #[test]
    fn single_line_and_indented_directives_open_nothing() {
        let source = "ifdef::x[Some text.]\n  ifdef::y[]\n[[a]]\ninclude::a.adoc[]\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(!document.anchors[0].conditional);
        assert!(!document.includes[0].conditional);
    }

    #[test]
    fn includes_in_conditionals_and_verbatim_blocks_are_conditional() {
        let source = "ifndef::env-site[]\n----\ninclude::example$a.rb[]\n----\nendif::[]\ninclude::b.adoc[]\n";
        let document = parse("file:///demo.adoc", source).document;

        let conditional: Vec<_> = document.includes.iter().map(|i| i.conditional).collect();
        assert_eq!(conditional, [true, false]);
    }

    #[test]
    fn tolerates_incomplete_constructs() {
        let source = "= Draft\n\nxref:unfinished[\ninclude::partial.adoc[\n<<anchor\n";
        let result = parse("untitled:Draft", source);

        assert_eq!(result.document.title.unwrap().text, "Draft");
        assert!(result.document.references.is_empty());
        assert!(result.document.includes.is_empty());
    }

    #[test]
    fn reports_duplicate_anchors_without_failing() {
        let source = "[[same]]\n== First\n\n[[same]]\n== Second\n";
        let result = parse("file:///duplicates.adoc", source);

        assert_eq!(result.document.sections.len(), 2);
        assert_eq!(result.diagnostics().len(), 1);
        assert!(result.diagnostics()[0].message.contains("duplicate anchor"));
    }

    fn reference_targets(text: &str) -> Vec<String> {
        parse("file:///guide.adoc", text)
            .document
            .references
            .into_iter()
            .map(|reference| reference.target)
            .collect()
    }

    #[test]
    fn ignores_references_inside_inline_passthroughs() {
        // Asciidoctor pulls passthroughs out before anything else, so the `<<` of one and
        // the `>>` of the next must not pair up into a reference.
        assert!(reference_targets("| `+<<+` / `+>>+` | Strictly left\n").is_empty());
        assert!(reference_targets(
            "Use ++<<a>>++, +++<<b>>+++, pass:[<<c>>], pass:q[<<d>>] and +xref:e.adoc[]+ here.\n"
        )
        .is_empty());
    }

    #[test]
    fn still_finds_references_beside_a_passthrough() {
        let text = "See +literal+ and <<real>>.\n";
        let references = parse("file:///guide.adoc", text).document.references;

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].target, "real");
        let start = text.find("<<real>>").unwrap();
        assert_eq!(references[0].range.start, start);
        assert_eq!(references[0].range.end, start + "<<real>>".len());
    }

    #[test]
    fn ignores_escaped_references() {
        // Asciidoctor shows a reference with a backslash before it as written.
        assert!(reference_targets("Type \\<<id>> or \\xref:other.adoc[] here.\n").is_empty());
        assert_eq!(reference_targets("\\<<a>> then <<b>>\n"), ["b"]);
    }

    #[test]
    fn treats_plus_signs_that_open_no_passthrough_as_text() {
        // A spaced `+`, a word-internal `+` and an escaped `\+` open no passthrough.
        assert_eq!(reference_targets("1 + 2 <<a>> 3 + 4\n"), ["a"]);
        assert_eq!(reference_targets("x+<<b>>+y\n"), ["b"]);
        assert_eq!(reference_targets("\\+<<c>>+\n"), ["c"]);
    }

    #[test]
    fn ignores_anchors_and_images_inside_inline_passthroughs() {
        let document = parse(
            "file:///guide.adoc",
            "Show +[[fake]]+ and +image:x.png[]+ text\n",
        )
        .document;

        assert!(document.anchors.is_empty(), "{:?}", document.anchors);
        assert!(document.images.is_empty(), "{:?}", document.images);
    }
}
