mod access;
mod diagnostic;
mod document;
pub mod probe;
mod reference;
mod symbol;

pub use access::FileAccess;
pub use diagnostic::{Diagnostic, DiagnosticCode, DiagnosticSeverity};
pub use document::{
    AttributeDeclaration, Document, DocumentTitle, LineIndex, Location, SourceRange,
};
pub use reference::{ImageDirective, IncludeDirective, Reference, ReferenceKind};
pub use symbol::{alphanumeric_id, asciidoctor_id, canonical_id, Anchor, Section};

/// The folders a workspace leaves out unless `[workspace] ignore` says otherwise: the index,
/// Antora discovery and the file watcher never enter them, and the Files view hides them.
pub const DEFAULT_IGNORED_DIRECTORIES: &[&str] = &[
    ".cache",
    ".git",
    ".zed",
    "node_modules",
    "build",
    "target",
    "dist",
    ".idea",
    ".gradle",
];
