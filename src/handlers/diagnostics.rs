use adoc_core::{Diagnostic, Document};

#[must_use]
pub fn diagnostics(document: &Document) -> &[Diagnostic] {
    &document.diagnostics
}
