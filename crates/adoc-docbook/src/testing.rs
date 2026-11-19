//! What tests need to check a document: the export's own [`crate::check`], and validation
//! against the DocBook 5.2.1 RELAX NG schema (`schema/docbook.rng`, kept in the repository and
//! never downloaded), in process through `oxixml-relaxng`. The schema's embedded Schematron
//! rules are not run.

use oxixml_relaxng::RelaxNg;

/// The DocBook 5.2.1 RELAX NG schema, in XML syntax.
pub const SCHEMA: &str = include_str!("../schema/docbook.rng");

thread_local! {
    /// The compiled schema, once per test thread (it is not shared between threads).
    static COMPILED: RelaxNg = RelaxNg::from_xml(SCHEMA).expect("the DocBook schema compiles");
}

/// What the DocBook 5.2.1 RELAX NG schema finds wrong with `xml`, empty for a valid document.
#[must_use]
pub fn schema_problems(xml: &str) -> Vec<String> {
    COMPILED.with(|schema| match schema.validate(xml) {
        Ok(()) => Vec::new(),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    })
}

/// Fails unless `xml` passes both the export's own check and the RELAX NG schema.
///
/// # Panics
///
/// When it does not, naming the problems and showing the document.
pub fn assert_valid(xml: &str) {
    let mut problems = crate::check(xml);
    problems.extend(schema_problems(xml));
    assert!(
        problems.is_empty(),
        "the document is not valid DocBook 5.2:\n{}\n\n{xml}",
        problems.join("\n")
    );
}

/// The document parsed, for tests that look inside it.
///
/// # Panics
///
/// When it is not well-formed XML.
#[must_use]
pub fn parse(xml: &str) -> roxmltree::Document<'_> {
    roxmltree::Document::parse(xml).expect("well-formed XML")
}
