//! The export's own check of a document, run on every one before it is handed out. It is a
//! structural check, not schema validation: the tests check documents against the DocBook
//! 5.2.1 RELAX NG schema as well (`testing`), but an export does not carry the schema.
//!
//! What it checks: the XML is well formed, with no `DOCTYPE` (and so no entity declared) and no
//! processing instruction but Asciidoctor's hard line break, `<?asciidoc-br?>`; the root is an `article` or a `book` in the DocBook namespace with
//! the [`crate::VERSION`]; every element is in the DocBook namespace; every `xml:id` is a valid
//! name and unique; every `linkend` and `linkends` names one, and a `footnoteref` names a
//! `footnote` (a rule of the schema's Schematron); every `xlink:href` has a value; and every
//! `fileref` is a relative path that stays below the document's folder, or a web address (a
//! remote picture, never fetched); and no element is inside one the schema's Schematron keeps
//! it out of ([`EXCLUSIONS`]: no admonition in an admonition, no table in a table's cell…). A document whose pictures are referred to where they are
//! may have `fileref`s that climb out of its folder.

use std::collections::{HashMap, HashSet};

use crate::{NAMESPACE, VERSION, XLINK};

const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// The problems found in `xml`, empty for a document that passes.
#[must_use]
pub fn check(xml: &str) -> Vec<String> {
    check_document(xml, false)
}

/// [`check`], with `referenced` true for a document whose pictures are referred to where they
/// are ([`crate::AssetPolicy::Reference`]), whose `fileref`s may climb out of its folder.
pub(crate) fn check_document(xml: &str, referenced: bool) -> Vec<String> {
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
            let allowed = node
                .pi()
                .is_some_and(|pi| pi.target == crate::inlines::LINE_BREAK && pi.value.is_none());
            if !allowed {
                problems.push("the document holds a processing instruction".to_owned());
            }
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

    problems.extend(exclusion_problems(&document));

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
            let fine = is_relative_below(fileref)
                || is_web_address(fileref)
                || (referenced && is_relative(fileref));
            if !fine {
                problems.push(format!(
                    "<{name}> refers to \u{201c}{fileref}\u{201d}, not a relative path below the document"
                ));
            }
        }
    }
    problems
}

/// The schema's Schematron rules that keep an element out of another's descendants (`caution
/// must not occur among the children or descendants of note` and the like), as the 5.2.1
/// schema states them: each container, and what may not be inside it. Checked here without an
/// XPath engine, since each is only a test of names.
pub const EXCLUSIONS: &[(&str, &[&str])] = &[
    ("annotation", &["annotation"]),
    (
        "caption",
        &[
            "caution",
            "danger",
            "equation",
            "example",
            "figure",
            "important",
            "note",
            "sidebar",
            "table",
            "task",
            "tip",
            "warning",
        ],
    ),
    ("caution", ADMONITIONS),
    ("danger", ADMONITIONS),
    ("important", ADMONITIONS),
    ("note", ADMONITIONS),
    ("tip", ADMONITIONS),
    ("warning", ADMONITIONS),
    ("entry", &["informaltable", "table"]),
    ("equation", &["equation", "example", "figure", "table"]),
    ("example", &["equation", "example", "figure", "table"]),
    ("figure", &["equation", "example", "figure", "table"]),
    (
        "footnote",
        &[
            "caution",
            "danger",
            "epigraph",
            "equation",
            "example",
            "figure",
            "footnote",
            "important",
            "note",
            "sidebar",
            "table",
            "task",
            "tip",
            "warning",
        ],
    ),
    ("sidebar", &["sidebar"]),
    ("table", &["equation", "example", "figure"]),
];

const ADMONITIONS: &[&str] = &["caution", "danger", "important", "note", "tip", "warning"];

/// Whether an element named `inner` may not be inside one named `container`.
pub(crate) fn excludes(container: &str, inner: &str) -> bool {
    EXCLUSIONS
        .iter()
        .any(|(name, forbidden)| *name == container && forbidden.contains(&inner))
}

/// What [`EXCLUSIONS`] finds in `document`: each element inside one it may not be inside.
fn exclusion_problems(document: &roxmltree::Document<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        let Some((_, forbidden)) = EXCLUSIONS.iter().find(|(container, _)| *container == name)
        else {
            continue;
        };
        for inner in node
            .descendants()
            .skip(1)
            .filter(roxmltree::Node::is_element)
        {
            let inner_name = inner.tag_name().name();
            if forbidden.contains(&inner_name) {
                problems.push(format!(
                    "{inner_name} must not occur among the children or descendants of {name}"
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

/// Whether `address` is a web address, which a remote picture keeps.
fn is_web_address(address: &str) -> bool {
    (address.starts_with("https://") || address.starts_with("http://"))
        && !address.contains(char::is_whitespace)
}

/// Whether `path` is relative, uses `/` and names no scheme, wherever it leads.
fn is_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|part| !part.is_empty() && part != ".")
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
            "<para>a<?asciidoc-br?>b</para><para xml:id=\"p\"><xref linkend=\"p\"/><footnote xml:id=\"f\"><para>n</para></footnote><footnoteref linkend=\"f\"/><link xlink:href=\"https://example.com\">x</link></para><mediaobject><imageobject><imagedata fileref=\"a_assets/i1.png\"/></imageobject></mediaobject>",
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
            (
                "<imagedata fileref=\"ftp://x/a.png\"/>",
                "not a relative path",
            ),
            ("<?evil x?>", "processing instruction"),
            ("<para>a<?asciidoc-br x?>b</para>", "processing instruction"),
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
    fn referenced_pictures_may_climb_out() {
        let climbing = document(
            "<mediaobject><imageobject><imagedata fileref=\"../docs/a.png\"/></imageobject></mediaobject>",
        );
        assert_eq!(check(&climbing).len(), 1);
        assert_eq!(check_document(&climbing, true), Vec::<String>::new());
        let remote = document(
            "<mediaobject><imageobject><imagedata fileref=\"https://example.com/a.png\"/></imageobject></mediaobject>",
        );
        assert_eq!(check(&remote), Vec::<String>::new());
        let absolute = document(
            "<mediaobject><imageobject><imagedata fileref=\"/etc/a.png\"/></imageobject></mediaobject>",
        );
        assert_eq!(check_document(&absolute, true).len(), 1);
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
