use crate::SourceRange;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticCode {
    UnresolvedXrefFile,
    UnresolvedAnchor,
    UnresolvedInclude,
    DuplicateAnchor,
    AntoraUnknownModule,
    AntoraUnknownResource,
}

impl DiagnosticCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnresolvedXrefFile => "adoc.unresolved-xref-file",
            Self::UnresolvedAnchor => "adoc.unresolved-anchor",
            Self::UnresolvedInclude => "adoc.unresolved-include",
            Self::DuplicateAnchor => "adoc.duplicate-anchor",
            Self::AntoraUnknownModule => "adoc.antora.unknown-module",
            Self::AntoraUnknownResource => "adoc.antora.unknown-resource",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Information,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub range: SourceRange,
}
