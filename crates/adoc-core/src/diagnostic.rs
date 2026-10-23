use crate::SourceRange;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticCode {
    UnresolvedXrefFile,
    UnresolvedAnchor,
    UnresolvedInclude,
    UnresolvedImage,
    DuplicateAnchor,
    AntoraUnknownModule,
    AntoraUnknownResource,
    AntoraInvalidFamily,
    AntoraInvalidCoordinate,
    AntoraInvalidDescriptor,
    AntoraResolvedViaAlias,
    AntoraNavUnresolved,
    AntoraVersionNotOnBranch,
}

impl DiagnosticCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnresolvedXrefFile => "adoc.unresolved-xref-file",
            Self::UnresolvedAnchor => "adoc.unresolved-anchor",
            Self::UnresolvedInclude => "adoc.unresolved-include",
            Self::UnresolvedImage => "adoc.unresolved-image",
            Self::DuplicateAnchor => "adoc.duplicate-anchor",
            Self::AntoraUnknownModule => "adoc.antora.unknown-module",
            Self::AntoraUnknownResource => "adoc.antora.unknown-resource",
            Self::AntoraInvalidFamily => "adoc.antora.invalid-family",
            Self::AntoraInvalidCoordinate => "adoc.antora.invalid-coordinate",
            Self::AntoraInvalidDescriptor => "adoc.antora.invalid-descriptor",
            Self::AntoraResolvedViaAlias => "adoc.antora.resolved-via-alias",
            Self::AntoraNavUnresolved => "adoc.antora.nav-unresolved",
            Self::AntoraVersionNotOnBranch => "adoc.antora.version-not-on-branch",
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
