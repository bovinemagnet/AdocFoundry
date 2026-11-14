use std::{borrow::Cow, collections::HashMap};

use adoc_core::{
    Anchor, Diagnostic, DiagnosticCode, DiagnosticSeverity, Document, DocumentTitle,
    IncludeDirective, LineIndex, Related, Section, SourceRange,
};

use crate::line_parser::{
    broken_code_spans, content, find_anchors, find_images, find_includes, find_references,
    find_ui_macros, is_block_metadata, is_dynamic_id, is_line_comment, mask_passthroughs,
    parse_attribute, parse_heading, trailing_anchor, Conditional, Heading, Verbatim,
    VerbatimBlocks,
};
use crate::table::Tables;

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
    let mut tables = Tables::default();
    let mut pending_anchor: Option<String> = None;
    // The last line read as text: where it starts, its content and the anchor before it, for
    // a two-line title whose underline follows.
    let mut previous: Option<(usize, &str, Option<String>)> = None;
    // How many `ifdef`, `ifndef` and `ifeval` blocks are open.
    let mut conditionals = 0_usize;
    // The last attribute entry's value ended in ` \`, so the next line continues it.
    let mut continuing = false;
    // Whether body content has begun, so an include can no longer be in the header.
    let mut body = false;
    // The block attribute lines since the last block, for the block image that may follow.
    let mut block_attributes: Vec<String> = Vec::new();
    // Code spans that do not close where they seem to (#593).
    let mut broken_spans = Vec::new();
    // Asciidoctor drops a leading byte order mark; ranges still count it.
    let (source, mut offset) = match text.strip_prefix('\u{feff}') {
        Some(source) => (source, '\u{feff}'.len_utf8()),
        None => (text, 0),
    };
    let start = offset;

    for line in source.split_inclusive('\n') {
        let line_content = content(line);
        let trimmed = line_content.trim();

        if let Some(directive) = verbatim.directive(line_content) {
            match directive {
                Conditional::Open => conditionals += 1,
                Conditional::Close => conditionals = conditionals.saturating_sub(1),
                // The text is a conditional line of its own: what it includes, declares and
                // references, read as any line is, unless it stands in a verbatim block,
                // where only an include counts.
                Conditional::Line { start, text } => {
                    let at = offset + start;
                    let found =
                        find_includes(text, at)
                            .into_iter()
                            .map(|include| IncludeDirective {
                                conditional: true,
                                before_body: !body,
                                ..include
                            });
                    document.includes.extend(found);
                    if verbatim.next() == Verbatim::No {
                        let inline = mask_passthroughs(text);
                        document
                            .anchors
                            .extend(find_anchors(&inline, at).into_iter().map(|anchor| Anchor {
                                conditional: true,
                                ..anchor
                            }));
                        document.references.extend(find_references(&inline, at));
                        document.images.extend(find_images(&inline, at));
                    }
                }
            }
            offset += line.len();
            continue;
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
        let includes = |line: &str, offset: usize, body: bool| {
            find_includes(line, offset)
                .into_iter()
                .map(move |include| IncludeDirective {
                    conditional,
                    before_body: !body,
                    ..include
                })
        };

        // A heading only where a block begins, judged before the line is read.
        let heading = parse_heading(line, offset).filter(|_| verbatim.begins_block());
        let kind = verbatim.line(trimmed);
        // The text of literal table cells, which Asciidoctor shows as it is written.
        let literal = tables.line(
            line_content,
            kind == Verbatim::No,
            &block_attributes,
            &source[offset - start + line.len()..],
        );
        match kind {
            Verbatim::No => {}
            Verbatim::Underline(marker_level) => {
                if let Some((start, title_line, pending)) = previous.take() {
                    if marker_level != 1 || document.title.is_some() {
                        body = true;
                    }
                    block_attributes.clear();
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
                    body = true;
                    block_attributes.clear();
                }
                offset += line.len();
                continue;
            }
            kind => {
                // A comment block's lines are no content.
                if kind != Verbatim::Comment {
                    body = true;
                    block_attributes.clear();
                }
                // Asciidoctor processes includes inside listing, literal and passthrough
                // blocks — a code sample pulled into a `[source]` block is the commonest
                // use of the `example$` family. Everything else in the block is content.
                if kind == Verbatim::Content {
                    document.includes.extend(includes(line, offset, body));
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
        let inline = mask_spans(mask_passthroughs(line), &literal);
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

        // Block metadata, attribute entries and includes leave the body unbegun, and block
        // metadata reaches past blank lines to the block it belongs to.
        let mut metadata = trimmed.is_empty() || anchor_only || is_block_metadata(trimmed);
        if let Some(inside) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if !trimmed.starts_with("[[") {
                block_attributes.push(inside.to_owned());
            }
            metadata = true;
        }
        let line_includes: Vec<_> = includes(line, offset, body).collect();
        let mut header_line = metadata || !line_includes.is_empty();
        let mut entry = false;

        if let Some(heading) = heading.filter(|_| !verbatim.in_compound_block()) {
            if heading.marker_level == 1 && document.title.is_none() {
                header_line = true;
            }
            add_heading(&mut document, &heading, pending_anchor.take());
        } else if let Some(mut attribute) = parse_attribute(line, offset) {
            if let Some(value) = &attribute.value {
                let (value, again) = without_continuation(value);
                continuing = again;
                attribute.value = Some(value.to_owned()).filter(|value| !value.is_empty());
            }
            document.attributes.push(attribute);
            header_line = true;
            entry = true;
        } else if !trimmed.is_empty() && !anchor_only && !is_block_metadata(trimmed) {
            pending_anchor = None;
        }

        // Prose only: not metadata or an attribute entry, not an indented literal line, and
        // not a STEM macro, whose content is not AsciiDoc.
        if !metadata
            && !entry
            && !line.starts_with([' ', '\t'])
            && !line.contains("math:[")
            && !line.contains("stem:[")
        {
            broken_spans.extend(broken_code_spans(&inline, offset));
            document.ui_macros.extend(find_ui_macros(&inline, offset));
        }
        document.references.extend(find_references(&inline, offset));
        document.includes.extend(line_includes);
        for mut image in find_images(&inline, offset) {
            if !image.inline {
                image.block_attributes.clone_from(&block_attributes);
            }
            document.images.push(image);
        }
        if !metadata {
            block_attributes.clear();
        }
        body |= !header_line;
        offset += line.len();
    }

    document.diagnostics = duplicate_anchors(text, &document.anchors);
    document
        .diagnostics
        .extend(broken_spans.into_iter().map(|(range, message)| Diagnostic {
            code: DiagnosticCode::BrokenCodeSpan,
            severity: DiagnosticSeverity::Warning,
            message,
            range,
            related: Vec::new(),
        }));
    document
        .diagnostics
        .sort_by_key(|diagnostic| diagnostic.range);
    ParseResult { document }
}

/// `line` with the byte ranges `spans` blanked out, offsets kept.
fn mask_spans<'a>(line: Cow<'a, str>, spans: &[(usize, usize)]) -> Cow<'a, str> {
    if spans.is_empty() {
        return line;
    }
    let mut masked = line.into_owned();
    for &(start, end) in spans {
        masked.replace_range(start..end, &" ".repeat(end - start));
    }
    Cow::Owned(masked)
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
    let lines = lines.get_or_insert_with(|| LineIndex::new(text));
    let (line, column) = lines.line_character(text, offset);
    (line + 1, column + 1)
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

    /// A block attribute line giving an id replaces an anchor before it; content clears it. (A
    /// heading right under `Text.` would be part of its paragraph, so a blank line comes first.)
    #[test]
    fn a_later_block_id_replaces_an_anchor_and_content_clears_it() {
        assert_eq!(
            section_ids_and_titles("[[a1]]\n[#b1]\n== Replaced\n\n[[gone]]\nText.\n\n== Plain\n"),
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
    fn block_images_record_the_attribute_lines_above_them() {
        let source = "[alt=\"A sunset\"]\nimage::a.png[]\n\n.Caption\n[#fig,alt=Sunset]\n\nimage::b.png[]\n\nText.\n\nimage::c.png[]\n[.right]\n// note\n[A sunset]\nimage::e.png[] \n\n[.left]\nSee image:d.png[] inline.\n";
        let document = parse("file:///images.adoc", source).document;

        let attributes: Vec<(&str, Vec<&str>)> = document
            .images
            .iter()
            .map(|image| {
                (
                    image.target.as_str(),
                    image.block_attributes.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            attributes,
            [
                ("a.png", vec!["alt=\"A sunset\""]),
                ("b.png", vec!["#fig,alt=Sunset"]),
                ("c.png", vec![]),
                ("e.png", vec![".right", "A sunset"]),
                ("d.png", vec![]),
            ]
        );
    }

    #[test]
    fn only_includes_before_the_body_are_before_it() {
        let source = "// header\n:toc:\n\ninclude::partial$header.adoc[]\nifdef::x[]\ninclude::partial$maybe.adoc[]\nendif::[]\n[[top]]\nSome text.\n\ninclude::partial$footer.adoc[]\n";
        let document = parse("file:///page.adoc", source).document;
        let before: Vec<(&str, bool)> = document
            .includes
            .iter()
            .map(|include| (include.target.as_str(), include.before_body))
            .collect();
        assert_eq!(
            before,
            [
                ("partial$header.adoc", true),
                ("partial$maybe.adoc", true),
                ("partial$footer.adoc", false),
            ]
        );

        let listing = "[source,java]\n----\ninclude::example$Demo.java[]\n----\n";
        let document = parse("file:///page.adoc", listing).document;
        assert!(!document.includes[0].before_body, "inside a listing block");

        let section = "== Section\ninclude::partial$a.adoc[]\n";
        let document = parse("file:///page.adoc", section).document;
        assert!(!document.includes[0].before_body, "after a section");
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

    /// An open block under a verbatim style is that verbatim block, and a `[pass]` paragraph
    /// is raw (#532).
    #[test]
    fn verbatim_styles_over_open_blocks_and_pass_paragraphs_are_verbatim() {
        for source in [
            "[listing]\n--\n<<x>>\n[[a]]\n--\n",
            "[source,ruby]\n--\n<<x>>\n\n[[a]]\n--\n",
            "[pass]\n<<x>> [[a]]\n",
        ] {
            let document = parse("file:///doc.adoc", source).document;
            assert!(document.references.is_empty(), "{source:?}");
            assert!(document.anchors.is_empty(), "{source:?}");
        }
        assert_eq!(
            reference_targets("[listing]\n--\n<<x>>\n--\n\nSee <<y>>.\n"),
            ["y"]
        );
    }

    /// A heading inside an example, sidebar, quote or open block is no section (#532).
    #[test]
    fn headings_inside_compound_blocks_are_not_sections() {
        let source = "====\n== Not a section\n====\n\n****\n=== In sidebar\n****\n\n\
                      ____\n== Quoted\n____\n\n--\n== Open\n\n====\n== Nested\n====\n--\n\n== Real\n";
        let titles: Vec<_> = parse("file:///doc.adoc", source)
            .document
            .sections
            .into_iter()
            .map(|section| section.title)
            .collect();
        assert_eq!(titles, ["Real"]);
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
    /// A single-line conditional's text is a line of its own, conditional: an include in it is
    /// recorded, and anchors in two of them do not collide (#532).
    #[test]
    fn a_single_line_conditional_holds_a_conditional_line() {
        let source = "ifdef::x[include::part.adoc[]]\nifdef::x[[[a]]]\nifndef::x[[[a]]]\n";
        let document = parse("file:///demo.adoc", source).document;

        let includes: Vec<_> = document
            .includes
            .iter()
            .map(|include| {
                (
                    include.target.as_str(),
                    include.conditional,
                    &source[include.range.start..include.range.end],
                )
            })
            .collect();
        assert_eq!(includes, [("part.adoc", true, "include::part.adoc[]")]);
        let anchors: Vec<_> = document
            .anchors
            .iter()
            .map(|anchor| (anchor.id.as_str(), anchor.conditional))
            .collect();
        assert_eq!(anchors, [("a", true), ("a", true)]);
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
    }

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

    /// Markdown-style `#` markers are headings where Asciidoctor 2.0 reads them: at the start of
    /// a block, from `#` to `######`, a closing marker dropped. Mid-paragraph, without a space,
    /// with seven markers or in a listing block, a `#` line is text (#548).
    #[test]
    fn markdown_heading_markers_are_headings_at_a_block_start() {
        let source = "# Markdown Title\n\nIntro.\n\n## Section One\n\nPara.\n# not a heading\n\n\
                      ### Sub ###\n\n#nospace\n\n----\n# shell comment\n----\n\n\
                      ####### Seven\n\n== Mixed level 1\n";
        let document = parse("file:///doc.adoc", source).document;
        let title = document.title.expect("a title");
        assert_eq!(title.text, "Markdown Title");
        assert_eq!(
            &source[title.selection_range.start..title.selection_range.end],
            "Markdown Title"
        );
        let sections: Vec<_> = document
            .sections
            .iter()
            .map(|section| (section.level, section.title.as_str()))
            .collect();
        assert_eq!(
            sections,
            [(1, "Section One"), (2, "Sub"), (1, "Mixed level 1")]
        );
    }

    /// A `=` heading, like a `#` one, is a heading only where a block begins: after a blank
    /// line, block metadata, a comment, a delimited block or another heading. Within a paragraph,
    /// a list item or the header's author and revision lines, Asciidoctor 2.0 reads it as text.
    #[test]
    fn equals_headings_are_headings_only_where_a_block_begins() {
        let source = "= Doc\nAuthor Name\n:attr: x\n== After header lines\n\n\
                      Para.\n== Mid paragraph\n\n* item\n== After list item\n\n\
                      [[sec-id]]\n== After anchor\n\n.Title\n== After block title\n\n\
                      // comment\n== After comment\n\n----\ncode\n----\n== After listing\n\n\
                      == A\n== B directly after A\n\n|===\n| cell\n|===\n== After table\n\n\
                      Para.\n= Level zero mid paragraph\n";
        let document = parse("file:///doc.adoc", source).document;
        assert_eq!(document.title.expect("a title").text, "Doc");
        let sections: Vec<_> = document
            .sections
            .iter()
            .map(|section| (section.level, section.title.as_str()))
            .collect();
        assert_eq!(
            sections,
            [
                (1, "After anchor"),
                (1, "After block title"),
                (1, "After comment"),
                (1, "After listing"),
                (1, "A"),
                (1, "B directly after A"),
                (1, "After table"),
            ]
        );
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

    /// Every prefix of a page holding the constructs read since the 11 Nov review, as typed one
    /// character at a time, parses and completes without panicking, and its section ids are
    /// generated.
    #[test]
    fn half_typed_constructs_never_panic() {
        let source = "\u{feff}= Doc =\n:desc: one \\\ntwo\n\nTitle é\n=======\n\n[[a1]]\n[source#code,ruby]\n--\n<<x>>\n--\n\n\
                      == What's [[t]] new? [[w,Ref]]\n\nSee <<other.adoc#d,D>>, <<<, a << b, $$<<p>>$$, [#12], \\[[e]], \
                      [.r#two]#m# and anchor:lab[L] x[#w]#y# **[#z]**b**.\n\nifdef::x[include::p.adoc[]]\nifndef::x[[[a]]]\n\
                      ====\n== In\n====\n/// c\n|===\n| image::c.png[] | b\n|===\n";
        for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
            let document = parse("file:///doc.adoc", &source[..end]).document;
            for style in [adoc_core::IdStyle::Antora, adoc_core::IdStyle::Asciidoctor] {
                let _ = adoc_core::section_ids(&document.sections, &document.anchors, style);
            }
            let _ = crate::completion_context(&source[..end], end);
        }
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

    /// The UI macros of `source`: each as its name and the text it covers.
    fn ui_macros(source: &str) -> Vec<(&'static str, String)> {
        parse("file:///doc.adoc", source)
            .document
            .ui_macros
            .iter()
            .map(|found| {
                (
                    found.name,
                    source[found.range.start..found.range.end].to_owned(),
                )
            })
            .collect()
    }

    /// Issue #600: the keyboard, button and menu macros, which Asciidoctor reads only while
    /// `experimental` is set, are recorded wherever prose is read, headings included.
    #[test]
    fn records_keyboard_button_and_menu_macros_in_prose() {
        let source =
            "= Keys\n\n== Save with kbd:[Ctrl+S]\n\nThen btn:[Save] or menu:File[Save As…], \
                      kbd:[Ctrl+\\]] and menu:View[].\n";
        assert_eq!(
            ui_macros(source),
            [
                ("kbd", "kbd:[Ctrl+S]".to_owned()),
                ("btn", "btn:[Save]".to_owned()),
                ("menu", "menu:File[Save As…]".to_owned()),
                ("kbd", "kbd:[Ctrl+\\]]".to_owned()),
                ("menu", "menu:View[]".to_owned()),
            ]
        );
    }

    /// Issue #600: nothing Asciidoctor would not read as a UI macro is recorded.
    #[test]
    fn leaves_out_ui_macros_escaped_passed_through_verbatim_or_incomplete() {
        let source = "Write \\kbd:[Ctrl+S], +kbd:[Ctrl+S]+, `+btn:[OK]+` or pass:[btn:[OK]].\n\n\
                      ----\nkbd:[Ctrl+S]\n----\n\n// kbd:[Ctrl+S]\n\n:shortcut: kbd:[Ctrl+S]\n\n  \
                      kbd:[Ctrl+S]\n\nNot menu:[x], menu: File[x], kbd:[] or btn:[OK.\n";
        assert!(ui_macros(source).is_empty(), "{:?}", ui_macros(source));
    }

    /// The broken code spans of `source`: each as the text it covers and its message.
    fn broken_code_spans(source: &str) -> Vec<(String, String)> {
        parse("file:///doc.adoc", source)
            .document
            .diagnostics
            .iter()
            .filter(|d| d.code == adoc_core::DiagnosticCode::BrokenCodeSpan)
            .map(|d| {
                assert_eq!(d.severity, adoc_core::DiagnosticSeverity::Warning);
                (
                    source[d.range.start..d.range.end].to_owned(),
                    d.message.clone(),
                )
            })
            .collect()
    }

    /// Issue #593: Asciidoctor reads a backtick followed by an apostrophe as a curved
    /// apostrophe, so `` `x`'s `` loses the backtick that would close the span.
    #[test]
    fn a_code_span_followed_by_an_apostrophe_is_broken() {
        let found = broken_code_spans("The `adoc-render`'s `ViewResolver` is used.\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "`adoc-render`'");
        assert!(found[0].1.contains("``adoc-render``'s"), "{}", found[0].1);
    }

    /// Issue #593: a constrained span cannot close before a word character, so `` `x`s ``
    /// runs on to the next backtick.
    #[test]
    fn a_code_span_followed_by_a_letter_or_digit_is_broken() {
        let found =
            broken_code_spans("It publishes `EngineEvent`s. Every action is a `Command`.\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "`EngineEvent`s");
        assert!(found[0].1.contains("``EngineEvent``s"), "{}", found[0].1);
        assert_eq!(broken_code_spans("Count `x`2 times.\n").len(), 1);
    }

    #[test]
    fn broken_code_spans_are_found_in_titles_lists_and_cells() {
        let source = "== The `Engine`'s pump\n\n* `Hunk`s\n\n|===\n|`Node`s |ok\n|===\n";
        let found: Vec<String> = broken_code_spans(source).into_iter().map(|f| f.0).collect();
        assert_eq!(found, ["`Engine`'", "`Hunk`s", "`Node`s"]);
    }

    /// Conservative, as every diagnostic is: nothing that is not a constrained code span
    /// closed in the wrong place.
    #[test]
    fn well_formed_and_unrelated_backticks_are_not_broken() {
        let quiet = [
            // Closed well, and the unconstrained form.
            "A `x` and `y`, `z`. and (`w`) here.\n",
            "A ``ViewResolver``'s field and ``EngineEvent``s.\n",
            // Curved quotes and a curved apostrophe.
            "Say '`single`' and \"`double`\" and Sam`'s hat.\n",
            // Passthroughs and an escaped backtick.
            "Pass `+x`s+` and +`x`s+ and pass:[`x`s] here.\n",
            "An escaped \\`x`s here.\n",
            // A lone backtick, and a span left open.
            "A lone ` here, and `open on this line\n",
            // An attribute entry, a comment and block metadata.
            ":note: `x`s\n// `x`s\n[source,`x`s]\nText.\n",
        ];
        for source in quiet {
            assert_eq!(broken_code_spans(source), Vec::new(), "{source:?}");
        }
    }

    #[test]
    fn verbatim_and_comment_blocks_have_no_broken_code_spans() {
        let source = "----\n`x`s\n----\n\n....\n`x`'s\n....\n\n////\n`x`s\n////\n\n\
                      [source]\n`x`s\n\n++++\n`x`s\n++++\n\n```\n`x`s\n```\n\n literal `x`s\n";
        assert_eq!(broken_code_spans(source), Vec::new());
    }

    /// Half-typed input is normal: a backtick at the very end, or before a multi-byte
    /// character, must not panic.
    #[test]
    fn half_typed_code_spans_do_not_panic() {
        for source in [
            "`",
            "``",
            "`x`",
            "a `x`",
            "`é`s",
            "`x`é",
            "`x`’s",
            "`\u{1f600}`s\n",
        ] {
            let _ = broken_code_spans(source);
        }
        assert_eq!(broken_code_spans("`é`s\n").len(), 1);
    }

    /// The targets of `source`'s cross references, in order.
    fn targets(source: &str) -> Vec<String> {
        parse("file:///doc.adoc", source)
            .document
            .references
            .into_iter()
            .map(|reference| reference.target)
            .collect()
    }

    /// Issue #532: Asciidoctor shows a literal table cell (`l|`, or a cell of a column `cols`
    /// marks `l`) as it is written, so what looks like a reference or an anchor in it is
    /// text. Each case was checked against Asciidoctor 2.0.26.
    #[test]
    fn a_literal_table_cell_holds_no_references_or_anchors() {
        let cell_specs = "|===\n\
                          |a l|<<raw1>> [[c1]] |<<real1>>\n\
                          2+l|span <<raw2>>\n\
                          .2+^.>l|<<raw3>> |<<real2>>\n\
                          |<<real3>>\n\
                          3*l|<<raw4>>\n\
                          |xl|<<real4>>\n\
                          |===\n";
        assert_eq!(targets(cell_specs), ["real1", "real2", "real3", "real4"]);
        let document = parse("file:///doc.adoc", cell_specs).document;
        assert!(document.anchors.is_empty(), "{:?}", document.anchors);

        // A column's style, which a cell's own style overrides; the header row takes none.
        let columns = "[cols=\"1,1l\"]\n|===\n|<<hdr1>> |<<hdr2>>\n\n\
                       |<<body1>> |<<raw1>>\n|<<body3>> d|<<body4>>\n|===\n";
        assert_eq!(
            targets(columns),
            ["hdr1", "hdr2", "body1", "body3", "body4"]
        );

        // A literal cell runs on over the lines that follow it, to the next cell.
        let lines = "|===\nl|first <<raw1>>\nsecond <<raw2>>\n|<<after>>\n|===\n";
        assert_eq!(targets(lines), ["after"]);

        // A rowspan from above does not move a cell to another column's style: a cell takes
        // the style of the column at its place among the row's cells, as Asciidoctor reads it.
        let rowspan = "[cols=\"1,1,1l\"]\n|===\n.2+|span |b |<<raw1>>\n|<<rs2>> |<<rs3>>\n|===\n";
        assert_eq!(targets(rowspan), ["rs2", "rs3"]);
        let colspan = "[cols=\"1,1,1l\"]\n|===\n2+|<<cs1>> |<<cs2>>\n|===\n";
        assert_eq!(targets(colspan), ["cs1", "cs2"]);

        // `2*l` styles two columns; a column count names no style.
        let repeated = "[cols=\"2*l,1\"]\n|===\n|<<raw1>> |<<raw2>> |<<m3>>\n|===\n";
        assert_eq!(targets(repeated), ["m3"]);
        let counted = "[cols=\"2\"]\n|===\n  l|<<raw1>> |<<sp2>>\n|===\n";
        assert_eq!(targets(counted), ["sp2"]);
    }

    /// The header row, implicit or asked for, is never literal, even under `l|`; without a
    /// header the first row is a row like any other.
    #[test]
    fn a_header_row_is_never_literal() {
        let implicit = "|===\nl|<<eh1>> |<<eh2>>\n\n|a |b\n|===\n";
        assert_eq!(targets(implicit), ["eh1", "eh2"]);
        let option = "[%header]\n|===\nl|<<oh1>> |x\n|a |b\n|===\n";
        assert_eq!(targets(option), ["oh1"]);
        let named = "[cols=\"1l\",options=\"header\"]\n|===\n|<<h3>>\n|<<raw1>>\n|===\n";
        assert_eq!(targets(named), ["h3"]);
        let none = "[%noheader,cols=\"1l,1\"]\n|===\n|<<raw1>> |<<nh2>>\n\n|<<raw2>> |b\n|===\n";
        assert_eq!(targets(none), ["nh2"]);
        let unseparated = "[cols=\"1l,1\"]\n|===\n|<<raw1>> |<<nb2>>\n|<<raw2>> |b\n|===\n";
        assert_eq!(targets(unseparated), ["nb2"]);
    }

    /// What a literal cell leaves out is only its own text: an escaped bar does not end it, a
    /// line comment in a table is no cell's, an include still counts, and after the table
    /// prose reads as before.
    #[test]
    fn a_literal_cell_ends_at_the_next_cell_and_only_there() {
        let source =
            "|===\nl|a \\| <<raw1>>\n// |<<comment>>\nmore <<raw2>>\ninclude::part.adoc[]\n\
                      |<<cell>>\n|===\n\n<<prose>> [[anchor]]\n";
        assert_eq!(targets(source), ["cell", "prose"]);
        let document = parse("file:///doc.adoc", source).document;
        assert_eq!(document.includes.len(), 1);
        let anchors: Vec<_> = document
            .anchors
            .iter()
            .map(|anchor| anchor.id.as_str())
            .collect();
        assert_eq!(anchors, ["anchor"]);
        // Not a table: a `|===` inside a listing block opens none.
        let listing = "----\n|===\n----\nl|<<prose2>>\n";
        assert_eq!(targets(listing), ["prose2"]);
    }
}
