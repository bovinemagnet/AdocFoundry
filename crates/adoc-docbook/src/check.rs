//! The export's own check of a document, run on every one before it is handed out. It is a
//! structural check, not schema validation: the tests check documents against the DocBook
//! 5.2.1 RELAX NG schema as well (`testing`), but an export does not carry the schema.
//!
//! What it checks: the XML is well formed, with no `DOCTYPE` (and so no entity declared) and no
//! processing instruction; the root is an `article` or a `book` in the DocBook namespace with
//! the [`crate::VERSION`]; every element is in the DocBook namespace; every `xml:id` is a valid
//! name and unique; every `linkend` and `linkends` names one, and a `footnoteref` names a
//! `footnote` (a rule of the schema's Schematron); every `xlink:href` has a value; and every
//! `fileref` is a relative path that stays below the document's folder.

use std::collections::{HashMap, HashSet};

use crate::{NAMESPACE, VERSION, XLINK};

const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// The problems found in `xml`, empty for a document that passes.
#[must_use]
pub fn check(xml: &str) -> Vec<String> {
    let document = match roxmltree::Document::parse(xml) {
        Ok(document) => document,
        Err(error) => return vec![format!("not well-formed XML: {error}")],
    };
    let mut problems = Vec::new();
    let root = document.root_element();
    if root.tag_name().namespace() != Some(NAMESPACE) {
        problems.push(format!(
            "the root is not in the DocBook namespace {NAMESPACE}"
        ));
    }
    if !matches!(root.tag_name().name(), "article" | "book") {
        problems.push(format!(
            "the root is <{}>, not <article> or <book>",
            root.tag_name().name()
        ));
    }
    if root.attribute("version") != Some(VERSION) {
        problems.push(format!("the root's version is not {VERSION}"));
    }

    let mut ids: HashMap<&str, &str> = HashMap::new();
    for node in document.descendants() {
        if node.is_pi() {
            problems.push("the document holds a processing instruction".to_owned());
            continue;
        }
        if !node.is_element() {
            continue;
        }
        let name = node.tag_name().name();
        if node.tag_name().namespace() != Some(NAMESPACE) {
            problems.push(format!("<{name}> is not in the DocBook namespace"));
        }
        if let Some(id) = node.attribute((XML, "id")) {
            if !is_name(id) {
                problems.push(format!("xml:id \u{201c}{id}\u{201d} is not a valid name"));
            }
            if ids.insert(id, name).is_some() {
                problems.push(format!(
                    "xml:id \u{201c}{id}\u{201d} is used more than once"
                ));
            }
        }
    }

    let mut reported = HashSet::new();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        let mut targets: Vec<&str> = node.attribute("linkend").into_iter().collect();
        if let Some(linkends) = node.attribute("linkends") {
            targets.extend(linkends.split_ascii_whitespace());
        }
        for target in targets {
            match ids.get(target) {
                None => {
                    if reported.insert(target) {
                        problems.push(format!(
                            "<{name}> links to \u{201c}{target}\u{201d}, which no element has as its xml:id"
                        ));
                    }
                }
                Some(&kind) if name == "footnoteref" && kind != "footnote" => {
                    problems.push(format!(
                    "<footnoteref> links to \u{201c}{target}\u{201d}, a <{kind}>, not a footnote"
                ))
                }
                Some(_) => {}
            }
        }
        if let Some(href) = node.attribute((XLINK, "href")) {
            if href.trim().is_empty() {
                problems.push(format!("<{name}> has an empty xlink:href"));
            }
        }
        if let Some(fileref) = node.attribute("fileref") {
            if !is_relative_below(fileref) {
                problems.push(format!(
                    "<{name}> refers to \u{201c}{fileref}\u{201d}, not a relative path below the document"
                ));
            }
        }
    }
    problems
}

/// Whether `id` is an XML name with no colon (an `NCName`), as `xml:id` must be. Conservative:
/// letters as Unicode counts them, digits and `.`, `-` and `_`.
pub(crate) fn is_name(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '\u{B7}'))
}

/// Whether `path` is relative, uses `/`, names no scheme and never climbs above where it starts.
fn is_relative_below(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><article xmlns=\"{NAMESPACE}\" xmlns:xlink=\"{XLINK}\" version=\"{VERSION}\"><title>T</title>{body}</article>"
        )
    }

    #[test]
    fn a_sound_document_passes() {
        let xml = document(
            "<para xml:id=\"p\"><xref linkend=\"p\"/><footnote xml:id=\"f\"><para>n</para></footnote><footnoteref linkend=\"f\"/><link xlink:href=\"https://example.com\">x</link></para><mediaobject><imageobject><imagedata fileref=\"a_assets/i1.png\"/></imageobject></mediaobject>",
        );
        assert_eq!(check(&xml), Vec::<String>::new());
    }

    #[test]
    fn each_problem_is_found() {
        for (body, expected) in [
            ("<para", "not well-formed"),
            ("<para xml:id=\"1x\"/>", "not a valid name"),
            ("<para xml:id=\"a\"/><para xml:id=\"a\"/>", "more than once"),
            ("<xref linkend=\"nowhere\"/>", "no element has"),
            (
                "<para xml:id=\"p\"/><footnoteref linkend=\"p\"/>",
                "not a footnote",
            ),
            ("<link xlink:href=\" \">x</link>", "empty xlink:href"),
            (
                "<imagedata fileref=\"/etc/passwd\"/>",
                "not a relative path",
            ),
            ("<imagedata fileref=\"../x.png\"/>", "not a relative path"),
            ("<imagedata fileref=\"C:\\x.png\"/>", "not a relative path"),
            ("<imagedata fileref=\"file:x.png\"/>", "not a relative path"),
            ("<?evil x?>", "processing instruction"),
            (
                "<p xmlns=\"http://www.w3.org/1999/xhtml\"/>",
                "not in the DocBook namespace",
            ),
        ] {
            let problems = check(&document(body));
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "{body}: {problems:?}"
            );
        }
    }

    #[test]
    fn the_root_is_checked() {
        let doctype = format!(
            "<!DOCTYPE article [<!ENTITY e \"x\">]><article xmlns=\"{NAMESPACE}\" version=\"{VERSION}\"/>"
        );
        assert!(
            check(&doctype)[0].contains("not well-formed"),
            "a DOCTYPE is refused"
        );
        for (xml, expected) in [
            (format!("<article xmlns=\"{NAMESPACE}\"/>"), "version"),
            (
                format!("<article xmlns=\"{NAMESPACE}\" version=\"5.0\"/>"),
                "version",
            ),
            (
                format!("<chapter xmlns=\"{NAMESPACE}\" version=\"{VERSION}\"/>"),
                "not <article>",
            ),
            (
                format!("<article version=\"{VERSION}\"/>"),
                "DocBook namespace",
            ),
        ] {
            let problems = check(&xml);
            assert!(
                problems.iter().any(|problem| problem.contains(expected)),
                "{xml}: {problems:?}"
            );
        }
    }

    #[test]
    fn names_follow_xml() {
        for id in ["a", "_a", "a-b.c_d", "é1", "日本"] {
            assert!(is_name(id), "{id}");
        }
        for id in ["", "1a", "-a", "a:b", "a b", ".a"] {
            assert!(!is_name(id), "{id}");
        }
    }
}
