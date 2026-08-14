use std::collections::HashMap;

use adoc_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, Document, DocumentTitle, Section, SourceRange,
};

use crate::line_parser::{
    content, find_anchors, find_images, find_includes, find_references, is_verbatim_delimiter,
    parse_attribute, parse_heading,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseResult {
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
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
    AsciiDocParser.parse(uri, text)
}

fn parse_document(uri: &str, text: &str) -> ParseResult {
    let mut document = Document::new(uri, text);
    let mut diagnostics = Vec::new();
    let mut active_delimiter: Option<String> = None;
    let mut pending_anchor: Option<String> = None;
    let mut anchor_offsets = HashMap::<String, SourceRange>::new();
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        let line_content = content(line);
        let trimmed = line_content.trim();

        if let Some(delimiter) = active_delimiter.as_deref() {
            if trimmed == delimiter {
                active_delimiter = None;
            }
            offset += line.len();
            continue;
        }

        if is_verbatim_delimiter(trimmed) {
            active_delimiter = Some(trimmed.to_owned());
            pending_anchor = None;
            offset += line.len();
            continue;
        }

        if trimmed.starts_with("//") {
            pending_anchor = None;
            offset += line.len();
            continue;
        }

        let anchors = find_anchors(line, offset);
        let anchor_only = anchors.len() == 1
            && anchors[0].range == SourceRange::new(offset, offset + line_content.len());
        for anchor in anchors {
            if let Some(first_range) = anchor_offsets.get(&anchor.id) {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::DuplicateAnchor,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "duplicate anchor `{}`; first declared at byte {}",
                        anchor.id, first_range.start
                    ),
                    range: anchor.range,
                });
            } else {
                anchor_offsets.insert(anchor.id.clone(), anchor.range);
            }
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

        document.references.extend(find_references(line, offset));
        document.includes.extend(find_includes(line, offset));
        document.images.extend(find_images(line, offset));
        offset += line.len();
    }

    document.diagnostics.clone_from(&diagnostics);
    ParseResult {
        document,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use adoc_core::ReferenceKind;

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
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn ignores_semantics_inside_verbatim_blocks() {
        let source = "= Demo\n\n[source,java]\n----\nxref:not-real.adoc[]\n[[not-real]]\n----\n";
        let document = parse("file:///demo.adoc", source).document;

        assert!(document.references.is_empty());
        assert!(document.anchors.is_empty());
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
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("duplicate anchor"));
    }
}
