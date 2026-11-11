use std::collections::HashMap;

use adoc_core::{
    Anchor, Diagnostic, DiagnosticCode, DiagnosticSeverity, Document, DocumentTitle,
    IncludeDirective, LineIndex, Related, Section, SourceRange,
};

use crate::line_parser::{
    conditional_directive, content, find_anchors, find_images, find_includes, find_references,
    is_block_metadata, is_dynamic_id, is_line_comment, mask_passthroughs, parse_attribute,
    parse_heading, trailing_anchor, Conditional, Heading, Verbatim, VerbatimBlocks,
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
    // The last line read as text: where it starts, its content and the anchor before it, for
    // a two-line title whose underline follows.
    let mut previous: Option<(usize, &str, Option<String>)> = None;
    // How many `ifdef`, `ifndef` and `ifeval` blocks are open.
    let mut conditionals = 0_usize;
    // The last attribute entry's value ended in ` \`, so the next line continues it.
    let mut continuing = false;
    // Asciidoctor drops a leading byte order mark; ranges still count it.
    let (body, mut offset) = match text.strip_prefix('\u{feff}') {
        Some(body) => (body, '\u{feff}'.len_utf8()),
        None => (text, 0),
    };

    for line in body.split_inclusive('\n') {
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
        if continuing {
            if let Some(attribute) = document.attributes.last_mut() {
                let (more, again) = without_continuation(trimmed);
                let value = attribute.value.take().unwrap_or_default();
                let joined = [value.as_str(), more].join(" ");
                attribute.value = Some(joined.trim().to_owned()).filter(|value| !value.is_empty());
                attribute.range.end = offset + line_content.len();
                continuing = again;
            }
            offset += line.len();
            continue;
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
            Verbatim::Underline(marker_level) => {
                if let Some((start, title_line, pending)) = previous.take() {
                    let raw_title = title_line.trim_end();
                    let title = raw_title.trim_start();
                    let selection_start = start + raw_title.len() - title.len();
                    add_heading(
                        &mut document,
                        &Heading {
                            marker_level,
                            title,
                            range: SourceRange::new(start, offset + line_content.trim_end().len()),
                            selection_range: SourceRange::new(
                                selection_start,
                                selection_start + title.len(),
                            ),
                        },
                        pending,
                    );
                }
                offset += line.len();
                continue;
            }
            Verbatim::Delimiter => {
                // A comment block between an anchor and its block is no content.
                if !trimmed.starts_with('/') {
                    pending_anchor = None;
                }
                offset += line.len();
                continue;
            }
            kind => {
                // Asciidoctor processes includes inside listing, literal and passthrough
                // blocks — a code sample pulled into a `[source]` block is the commonest
                // use of the `example$` family. Everything else in the block is content.
                if kind == Verbatim::Content {
                    document.includes.extend(includes(line, offset));
                    pending_anchor = None;
                }
                offset += line.len();
                continue;
            }
        }

        // A line comment, like the block metadata below, leaves an anchor for the next block.
        if is_line_comment(trimmed) {
            offset += line.len();
            continue;
        }

        previous = Some((offset, line_content, pending_anchor.clone()));

        // Inline extraction sees passthroughs blanked out; includes are whole-line directives.
        let inline = mask_passthroughs(line);
        let anchors = find_anchors(&inline, offset);
        let anchor_only = anchors.len() == 1
            && anchors[0].range == SourceRange::new(offset, offset + line_content.trim_end().len());
        for mut anchor in anchors {
            anchor.conditional = conditional;
            if anchor_only {
                pending_anchor = Some(anchor.id.clone());
            }
            document.anchors.push(anchor);
        }

        if let Some(heading) = parse_heading(line, offset) {
            add_heading(&mut document, &heading, pending_anchor.take());
        } else if let Some(mut attribute) = parse_attribute(line, offset) {
            if let Some(value) = &attribute.value {
                let (value, again) = without_continuation(value);
                continuing = again;
                attribute.value = Some(value.to_owned()).filter(|value| !value.is_empty());
            }
            document.attributes.push(attribute);
        } else if !trimmed.is_empty() && !anchor_only && !is_block_metadata(trimmed) {
            pending_anchor = None;
        }

        document.references.extend(find_references(&inline, offset));
        document.includes.extend(includes(line, offset));
        document.images.extend(find_images(&inline, offset));
        offset += line.len();
    }

    document.diagnostics = duplicate_anchors(text, &document.anchors);
    ParseResult { document }
}

/// An attribute value's text without the ` \` that continues it on the next line, and whether
/// it had one.
fn without_continuation(value: &str) -> (&str, bool) {
    match value.strip_suffix('\\') {
        Some(before) if before.is_empty() || before.ends_with([' ', '\t']) => {
            (before.trim_end(), true)
        }
        _ => (value, false),
    }
}

/// Records `heading`: the document's title, when it is the first at level 0, else a section
/// whose id is `pending`, the anchor before it, or one ending its title.
fn add_heading(document: &mut Document, heading: &Heading<'_>, pending: Option<String>) {
    if heading.marker_level == 1 && document.title.is_none() {
        document.title = Some(DocumentTitle {
            text: heading.title.to_owned(),
            range: heading.range,
            selection_range: heading.selection_range,
        });
        return;
    }
    // An anchor before the heading wins over one ending its line.
    let (title, id) = match pending {
        Some(id) => (heading.title, Some(id)),
        None => trailing_anchor(heading.title).map_or((heading.title, None), |(title, id)| {
            (title, Some(id.to_owned()))
        }),
    };
    let selection_start = heading.selection_range.start;
    document.sections.push(Section {
        level: heading.marker_level.saturating_sub(1),
        title: title.to_owned(),
        id,
        range: heading.range,
        selection_range: SourceRange::new(selection_start, selection_start + title.len()),
    });
}

/// A warning for each anchor whose id an earlier one declared, naming the first by its line and
/// column and relating it, for a front end to link to. Anchors inside conditionals are left
/// out: two branches commonly declare the same id, and only one is ever built. So are ids made
/// with an attribute reference, whose values are not known here.
fn duplicate_anchors(text: &str, anchors: &[Anchor]) -> Vec<Diagnostic> {
    let mut first = HashMap::<&str, SourceRange>::new();
    let mut diagnostics = Vec::new();
    let mut lines = None;
    for anchor in anchors
        .iter()
        .filter(|anchor| !anchor.conditional && !is_dynamic_id(&anchor.id))
    {
        match first.get(anchor.id.as_str()) {
            Some(first_range) => {
                let (line, column) = line_and_column(text, &mut lines, first_range.start);
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::DuplicateAnchor,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "duplicate anchor `{}`; first declared at line {line}, column {column}",
                        anchor.id
                    ),
                    range: anchor.range,
                    related: vec![Related {
                        message: format!("`{}` is first declared here", anchor.id),
                        range: *first_range,
                    }],
                });
            }
            None => {
                first.insert(&anchor.id, anchor.range);
            }
        }
    }
    diagnostics
}

/// The line and the character on it of `offset` in `text`, both counted from 1. The line index
/// is built once, by the first duplicate, so each is placed by counting within its own line.
fn line_and_column(text: &str, lines: &mut Option<LineIndex>, offset: usize) -> (usize, usize) {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let lines = lines.get_or_insert_with(|| LineIndex::new(text));
    let (line, column) = lines.line_column(offset);
    let line_start = offset - column;
    (line + 1, text[line_start..offset].chars().count() + 1)
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

    fn section_ids_and_titles(source: &str) -> Vec<(Option<String>, String)> {
        parse("file:///doc.adoc", source)
            .document
            .sections
            .into_iter()
            .map(|section| (section.id, section.title))
            .collect()
    }

    fn section(id: Option<&str>, title: &str) -> (Option<String>, String) {
        (id.map(str::to_owned), title.to_owned())
    }

    /// An anchor reaches its section past block attribute lines, block titles, comments,
    /// attribute entries and blank lines, as Asciidoctor carries block metadata (#527).
    #[test]
    fn an_anchor_reaches_its_section_past_metadata() {
        let source = "[[app-a]]\n[appendix]\n== Appendix A\n\n[[trail]] \n== Trailing\n\n\
                      [[c]]\n// note\n== Commented\n\n[[x1]]\n.Block title\n[.role]\n== Mixed\n\n\
                      [[attr]]\n:foo: bar\n== After Attr\n\n[[c2]]\n////\nblock comment\n////\n== After Block Comment\n\n\
                      [[blank]]\n\n== After Blank\n";
        assert_eq!(
            section_ids_and_titles(source),
            [
                section(Some("app-a"), "Appendix A"),
                section(Some("trail"), "Trailing"),
                section(Some("c"), "Commented"),
                section(Some("x1"), "Mixed"),
                section(Some("attr"), "After Attr"),
                section(Some("c2"), "After Block Comment"),
                section(Some("blank"), "After Blank"),
            ]
        );
    }

    /// A block attribute line giving an id replaces an anchor before it; content clears it.
    #[test]
    fn a_later_block_id_replaces_an_anchor_and_content_clears_it() {
        assert_eq!(
            section_ids_and_titles("[[a1]]\n[#b1]\n== Replaced\n\n[[gone]]\nText.\n== Plain\n"),
            [section(Some("b1"), "Replaced"), section(None, "Plain")]
        );
    }

    /// A `[[id]]` ending the title line is the section's id, and leaves the title, unless an
    /// anchor before the heading gives one (#527).
    #[test]
    fn a_trailing_anchor_on_a_heading_is_its_id() {
        assert_eq!(
            section_ids_and_titles(
                "== Title [[custom]]\n\n== Y [[z,Ref]]\n\n[[t]]\n== X [[y]]\n\n== Not [[ok]] end\n"
            ),
            [
                section(Some("custom"), "Title"),
                section(Some("z"), "Y"),
                section(Some("t"), "X [[y]]"),
                section(None, "Not [[ok]] end"),
            ]
        );
        let source = "== Title [[custom]]\n";
        let section = &parse("file:///doc.adoc", source).document.sections[0];
        assert_eq!(
            &source[section.selection_range.start..section.selection_range.end],
            "Title"
        );
    }

    /// With the explicit id kept, a repeated title is numbered as Asciidoctor numbers it.
    #[test]
    fn a_discrete_heading_keeps_its_anchor() {
        assert_eq!(
            section_ids_and_titles("[[first]]\n[discrete]\n== Overview\n\n== Overview\n"),
            [
                section(Some("first"), "Overview"),
                section(None, "Overview")
            ]
        );
    }

    /// A byte order mark is dropped, as Asciidoctor drops it, and every range still points
    /// into the text as given (#525).
    #[test]
    fn reads_the_first_line_after_a_byte_order_mark() {
        let source = "\u{feff}= Bom\n\n== S\n";
        let document = parse("file:///a.adoc", source).document;
        let title = document.title.expect("a title");
        assert_eq!(title.text, "Bom");
        assert_eq!(title.range, SourceRange::new(3, 8));
        assert_eq!(title.selection_range, SourceRange::new(5, 8));
        assert_eq!(document.sections[0].title, "S");

        let document = parse("file:///a.adoc", "\u{feff}:toc: left\n").document;
        assert_eq!(document.attributes[0].name, "toc");
        assert_eq!(document.attributes[0].range.start, 3);

        let document = parse("file:///a.adoc", "\u{feff}include::part.adoc[]\n").document;
        assert_eq!(document.includes[0].target, "part.adoc");
        assert_eq!(document.includes[0].range.start, 3);

        let document = parse("file:///a.adoc", "\u{feff}----\n<<x>>\n----\n").document;
        assert!(document.references.is_empty());
    }

    /// A `<<…>>` target with a `#` after its start names another document, as Asciidoctor
    /// reads it; anything else, `.adoc` and all, is an id in this one (#521).
    #[test]
    fn double_angle_references_with_a_path_name_another_document() {
        let source = "See <<other.adoc#details,Details>> and <<other.adoc#details>> and <<other#details>>.\n\
                      And <<intro>>, <<#intro>>, <<sec.1>>, <<Section Title>> and <<other.adoc>>.\n";
        let document = parse("file:///demo.adoc", source).document;

        let kinds = document
            .references
            .iter()
            .map(|reference| {
                (
                    reference.kind,
                    reference.target.as_str(),
                    reference.text.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                (ReferenceKind::Xref, "other.adoc#details", Some("Details")),
                (ReferenceKind::Xref, "other.adoc#details", None),
                (ReferenceKind::Xref, "other#details", None),
                (ReferenceKind::LocalAnchor, "intro", None),
                (ReferenceKind::LocalAnchor, "intro", None),
                (ReferenceKind::LocalAnchor, "sec.1", None),
                (ReferenceKind::LocalAnchor, "Section Title", None),
                (ReferenceKind::LocalAnchor, "other.adoc", None),
            ]
        );
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

    /// The `#id` shorthand may follow a style or roles, on a block attribute line and inline
    /// (#522).
    #[test]
    fn anchor_shorthand_after_a_style_or_role() {
        assert_eq!(anchor_ids("[source#code,ruby]\n----\nx\n----\n"), ["code"]);
        assert_eq!(anchor_ids("[quote#q1]\n____\nHi\n____\n"), ["q1"]);
        assert_eq!(anchor_ids("[.lead#intro]\nPara.\n"), ["intro"]);
        assert_eq!(anchor_ids("[%header#tbl.r]\n|===\n|a\n|===\n"), ["tbl"]);
        assert_eq!(anchor_ids("[.r#two]#marked# here.\n"), ["two"]);
        // A named attribute first is no shorthand, and neither is a style alone.
        assert!(anchor_ids("[link=#top]\nPara.\n").is_empty());
        assert!(anchor_ids("[source,ruby]\n----\nx\n----\n").is_empty());
    }

    /// `anchor:id[]` and `anchor:id[reftext]` declare an anchor; an escaped one or one whose id
    /// cannot be an id is text (#522).
    #[test]
    fn anchor_macros_are_anchors() {
        let source =
            "See anchor:legacy[] and anchor:lab[Label], not \\anchor:esc[] or anchor:9x[].\n";
        let document = parse("file:///doc.adoc", source).document;
        let anchors: Vec<_> = document
            .anchors
            .iter()
            .map(|anchor| {
                (
                    anchor.id.as_str(),
                    &source[anchor.range.start..anchor.range.end],
                )
            })
            .collect();
        assert_eq!(
            anchors,
            [("legacy", "anchor:legacy[]"), ("lab", "anchor:lab[Label]")]
        );
    }

    #[test]
    fn anchor_shorthand_stops_at_role() {
        assert_eq!(anchor_ids("[#intro.lead]\n== Intro\n"), ["intro"]);
        assert_eq!(anchor_ids("[#intro,role=lead]\n== Intro\n"), ["intro"]);
        // Inline, before marked text.
        assert_eq!(anchor_ids("Some [#key.big]#word# here.\n"), ["key"]);
    }

    /// `[#…]` is an anchor only as a whole block attribute line or straight before a formatting
    /// mark; link text and prose such as `[#12]` are text (#523).
    #[test]
    fn link_text_and_prose_starting_with_a_hash_are_not_anchors() {
        let source = "* Fixed https://example.com/issues/12[#12] crash\n\
                      * Follow-up to https://example.com/issues/12[#12]\n\n\
                      See [#12] and again [#12].\n\n\
                      x[#w]#y# and [#spaced id]#z#\n";
        let document = parse("file:///doc.adoc", source).document;
        assert!(document.anchors.is_empty(), "{:?}", document.anchors);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );

        assert_eq!(
            anchor_ids("A [#s1]^sup^, [#s2]~sub~, [#d1]**dbl**x, [#c]`code` and [#e]_em_.\n"),
            ["s1", "s2", "d1", "c", "e"]
        );
    }

    /// An escaped anchor is shown as written (#523).
    #[test]
    fn escaped_anchors_are_not_anchors() {
        let document = parse(
            "file:///doc.adoc",
            "Write \\[[literal]] and \\[[literal]] twice.\n",
        )
        .document;
        assert!(document.anchors.is_empty(), "{:?}", document.anchors);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
    }

    /// An inline anchor's id starts with a letter, `_` or `:` and holds only word characters,
    /// `-`, `:` and `.`, as Asciidoctor's pattern has it (#523).
    #[test]
    fn text_that_cannot_be_an_id_is_not_an_anchor() {
        let document = parse(
            "file:///doc.adoc",
            "Index m[[0]] and m[[0]] again; [[two words]] and [[ spaced]].\n",
        )
        .document;
        assert!(document.anchors.is_empty(), "{:?}", document.anchors);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );

        assert_eq!(
            anchor_ids("[[ok-1.2:x]] [[_u]] [[:c]] [[x,Ref Text]] [[é]] [[{p}-a]]\n"),
            ["ok-1.2:x", "_u", ":c", "x", "é", "{p}-a"]
        );
    }

    /// An id made with an attribute reference cannot be compared before it is substituted,
    /// so two of them are no duplicate (#523).
    #[test]
    fn dynamic_ids_are_left_out_of_the_duplicate_check() {
        let document = parse(
            "file:///doc.adoc",
            "[[{prefix}-a]]\nOne.\n\n[[{prefix}-a]]\nTwo.\n",
        )
        .document;
        assert_eq!(document.anchors.len(), 2);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
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
            assert!(
                document
                    .anchors
                    .iter()
                    .all(|anchor| anchor.id != "not-real"),
                "{style}"
            );
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

    /// A two-line title is a title, not the opening of a listing block that hides the rest of
    /// the page (#526).
    #[test]
    fn two_line_titles_are_titles() {
        let source = "Title\n=====\n\nSection\n-------\n\n[[real]]\nText xref:other.adoc[] and <<real>>.\n\n\
                      Sub\n~~~\n\nDeep\n^^^^\n\n[[five]]\nDeeper\n+++++\n";
        let document = parse("file:///doc.adoc", source).document;

        let title = document.title.expect("a title");
        assert_eq!(title.text, "Title");
        assert_eq!(
            &source[title.selection_range.start..title.selection_range.end],
            "Title"
        );
        let sections: Vec<_> = document
            .sections
            .iter()
            .map(|section| (section.level, section.title.as_str(), section.id.as_deref()))
            .collect();
        assert_eq!(
            sections,
            [
                (1, "Section", None),
                (2, "Sub", None),
                (3, "Deep", None),
                (4, "Deeper", Some("five")),
            ]
        );
        let section = &document.sections[0];
        assert_eq!(
            &source[section.range.start..section.range.end],
            "Section\n-------"
        );
        assert_eq!(document.anchors.len(), 2);
        assert_eq!(document.references.len(), 2);
    }

    /// A delimiter under a blank line or a block attribute line, under a paragraph's second line,
    /// or two characters longer or shorter than the line above, still opens its block.
    #[test]
    fn delimiters_that_underline_nothing_still_open_blocks() {
        for source in [
            "Text\n\n----\n<<hidden>>\n----\n",
            "[source]\n----\n<<hidden>>\n----\n",
            "Para one\nPara two\n--------\n<<hidden>>\n--------\n",
            "Abcd\n------\n<<hidden>>\n------\n",
            "Abcdefgh\n------\n<<hidden>>\n------\n",
            ".Title\n----\n<<hidden>>\n----\n",
        ] {
            let document = parse("file:///doc.adoc", source).document;
            assert!(document.sections.is_empty(), "{source:?}");
            assert!(document.references.is_empty(), "{source:?}");
        }
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

    /// A continued attribute value runs on to the next line, which is no body text (#532).
    #[test]
    fn a_continued_attribute_value_takes_the_next_line() {
        let source = ":desc: one \\\ntwo <<x>> \\\nthree\nBody <<y>>.\n";
        let document = parse("file:///doc.adoc", source).document;
        let attribute = &document.attributes[0];
        assert_eq!(attribute.name, "desc");
        assert_eq!(attribute.value.as_deref(), Some("one two <<x>> three"));
        assert_eq!(
            &source[attribute.range.start..attribute.range.end],
            ":desc: one \\\ntwo <<x>> \\\nthree"
        );
        assert_eq!(reference_targets(source), ["y"]);
    }

    /// An attribute entry's name starts with a word character, and a space or the end of the
    /// line follows its second colon (#532).
    #[test]
    fn text_that_only_looks_like_an_attribute_entry_is_not_one() {
        let document = parse(
            "file:///doc.adoc",
            ":a:b\n\n:30:00 is the time\n\n:-x: y\n\n:ok:\n:v:\tvalue\n",
        )
        .document;
        let attributes: Vec<_> = document
            .attributes
            .iter()
            .map(|attribute| (attribute.name.as_str(), attribute.value.as_deref()))
            .collect();
        assert_eq!(attributes, [("ok", None), ("v", Some("value"))]);
    }

    /// `== Title ==` closes with its marker, and a tab may follow the marker (#532).
    #[test]
    fn a_heading_may_close_with_its_marker_or_open_with_a_tab() {
        let source = "= Doc =\n\n== Title ==\n\n==\tTabbed\n\n== Uneven ===\n";
        let document = parse("file:///doc.adoc", source).document;
        assert_eq!(document.title.unwrap().text, "Doc");
        let titles: Vec<_> = document
            .sections
            .iter()
            .map(|section| &source[section.selection_range.start..section.selection_range.end])
            .collect();
        assert_eq!(titles, ["Title", "Tabbed", "Uneven ==="]);
    }

    /// A line comment is `//` not followed by a third `/` (#532).
    #[test]
    fn three_slashes_are_text() {
        assert_eq!(reference_targets("/// see <<x>>\n// see <<y>>\n"), ["x"]);
    }

    /// A macro's target neither starts nor ends with a space, and a block image ends its line
    /// (#532).
    #[test]
    fn spaced_targets_and_trailing_text_are_not_macros() {
        let document = parse(
            "file:///doc.adoc",
            "include:: foo.adoc[]\n\nimage::a.png[] trailing\n\nimage:: b.png[]\n",
        )
        .document;
        assert!(document.includes.is_empty(), "{:?}", document.includes);
        assert!(document.images.is_empty(), "{:?}", document.images);
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

    #[test]
    fn a_duplicate_anchor_names_the_first_by_line_and_column_and_relates_it() {
        let source = "= Guide\n\nSee é [[same]] here.\n\n[[same]]\n== Second\n";
        let result = parse("file:///duplicates.adoc", source);
        let duplicate = &result.diagnostics()[0];

        assert_eq!(
            duplicate.message,
            "duplicate anchor `same`; first declared at line 3, column 7"
        );
        let first = source.find("[[same]]").unwrap();
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(duplicate.related[0].range.start, first);
        assert_eq!(
            duplicate.related[0].message,
            "`same` is first declared here"
        );
    }

    /// Reporting duplicates late in a long page costs about what reporting nothing does: each
    /// is placed without scanning the page from its start again (#530).
    #[test]
    fn duplicate_anchors_late_in_a_long_page_are_placed_in_linear_time() {
        let prose =
            "A line of prose that runs on for a while to make the page long.\n".repeat(20_000);
        let distinct: String = (0..4_000).map(|n| format!("[[a{n}]]\n")).collect();
        let doubled: String = (0..2_000)
            .map(|n| format!("[[a{n}]]\n[[a{n}]]\n"))
            .collect();
        let fastest = |source: &str| {
            (0..3)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let result = parse("file:///long.adoc", source);
                    (started.elapsed(), result.diagnostics().len())
                })
                .min()
                .unwrap()
        };

        let (without, none) = fastest(&format!("{prose}{distinct}"));
        let (with, reported) = fastest(&format!("{prose}{doubled}"));

        assert_eq!((none, reported), (0, 2_000));
        assert!(
            with < without * 3 + std::time::Duration::from_millis(100),
            "{with:?} with duplicates against {without:?} without"
        );
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

    /// A reference's target starts straight after `<<` or `xref:` with a word character or one
    /// of `#/.:{`, as Asciidoctor's pattern has it; shift operators and prose are text (#524).
    #[test]
    fn shift_operators_and_spaced_openings_are_not_references() {
        assert!(reference_targets("Use `x << 2` and `y >> 1` here.\n").is_empty());
        assert_eq!(
            reference_targets("If a << b then see <<real>>.\n"),
            ["real"]
        );
        assert!(reference_targets("Use $$<<a>>$$ here.\n").is_empty());
        assert!(reference_targets("The xref: macro takes a target [and text].\n").is_empty());
        assert!(reference_targets("A page break <<< is not <<-e>> either.\n").is_empty());
        assert_eq!(
            reference_targets("See <<{x}>>, <</a>>, <<:b>>, <<.c>> and <<<d>>.\n"),
            ["{x}", "/a", ":b", ".c", "d"]
        );
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
