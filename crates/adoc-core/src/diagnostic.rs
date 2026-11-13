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
    AntoraMissingKey,
    AntoraUnknownKey,
    AntoraMissingNavFile,
    AntoraInvalidAttributeValue,
    AntoraUnresolvedStartPage,
    /// A constrained code span whose closing backtick is followed by an apostrophe or a word
    /// character, so Asciidoctor does not close it there (#593).
    BrokenCodeSpan,
    /// A keyboard, button or menu macro where the `experimental` attribute is not set, so
    /// Asciidoctor shows it as text (#600).
    UiMacroNotEnabled,
}

impl DiagnosticCode {
    /// Every code, in declaration order.
    pub const ALL: [Self; 20] = [
        Self::UnresolvedXrefFile,
        Self::UnresolvedAnchor,
        Self::UnresolvedInclude,
        Self::UnresolvedImage,
        Self::DuplicateAnchor,
        Self::AntoraUnknownModule,
        Self::AntoraUnknownResource,
        Self::AntoraInvalidFamily,
        Self::AntoraInvalidCoordinate,
        Self::AntoraInvalidDescriptor,
        Self::AntoraResolvedViaAlias,
        Self::AntoraNavUnresolved,
        Self::AntoraVersionNotOnBranch,
        Self::AntoraMissingKey,
        Self::AntoraUnknownKey,
        Self::AntoraMissingNavFile,
        Self::AntoraInvalidAttributeValue,
        Self::AntoraUnresolvedStartPage,
        Self::BrokenCodeSpan,
        Self::UiMacroNotEnabled,
    ];

    /// The code whose stable name ([`DiagnosticCode::as_str`]) is `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == name)
    }

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
            Self::AntoraMissingKey => "adoc.antora.missing-key",
            Self::AntoraUnknownKey => "adoc.antora.unknown-key",
            Self::AntoraMissingNavFile => "adoc.antora.missing-nav-file",
            Self::AntoraInvalidAttributeValue => "adoc.antora.invalid-attribute-value",
            Self::AntoraUnresolvedStartPage => "adoc.antora.unresolved-start-page",
            Self::BrokenCodeSpan => "adoc.broken-code-span",
            Self::UiMacroNotEnabled => "adoc.ui-macro-not-enabled",
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
    /// Other places in the same file the problem involves, such as the first declaration of a
    /// duplicate anchor, for a front end to link to.
    pub related: Vec<Related>,
}

/// A place in the same file that a [`Diagnostic`] involves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Related {
    pub message: String,
    pub range: SourceRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Its place in [`DiagnosticCode::ALL`]. The match is exhaustive, so a new code does not
    /// compile until it is given a place, and the test below then fails until `ALL` lists it.
    fn place(code: DiagnosticCode) -> usize {
        match code {
            DiagnosticCode::UnresolvedXrefFile => 0,
            DiagnosticCode::UnresolvedAnchor => 1,
            DiagnosticCode::UnresolvedInclude => 2,
            DiagnosticCode::UnresolvedImage => 3,
            DiagnosticCode::DuplicateAnchor => 4,
            DiagnosticCode::AntoraUnknownModule => 5,
            DiagnosticCode::AntoraUnknownResource => 6,
            DiagnosticCode::AntoraInvalidFamily => 7,
            DiagnosticCode::AntoraInvalidCoordinate => 8,
            DiagnosticCode::AntoraInvalidDescriptor => 9,
            DiagnosticCode::AntoraResolvedViaAlias => 10,
            DiagnosticCode::AntoraNavUnresolved => 11,
            DiagnosticCode::AntoraVersionNotOnBranch => 12,
            DiagnosticCode::AntoraMissingKey => 13,
            DiagnosticCode::AntoraUnknownKey => 14,
            DiagnosticCode::AntoraMissingNavFile => 15,
            DiagnosticCode::AntoraInvalidAttributeValue => 16,
            DiagnosticCode::AntoraUnresolvedStartPage => 17,
            DiagnosticCode::BrokenCodeSpan => 18,
            DiagnosticCode::UiMacroNotEnabled => 19,
        }
    }

    #[test]
    fn all_lists_every_code_once_in_order() {
        for (index, code) in DiagnosticCode::ALL.iter().enumerate() {
            assert_eq!(place(*code), index, "{code:?}");
        }
        assert_eq!(
            DiagnosticCode::ALL.len(),
            DiagnosticCode::ALL
                .iter()
                .map(|code| place(*code))
                .max()
                .unwrap()
                + 1
        );
    }

    #[test]
    fn from_name_reads_back_every_code() {
        for code in DiagnosticCode::ALL {
            assert_eq!(DiagnosticCode::from_name(code.as_str()), Some(code));
        }
        assert_eq!(DiagnosticCode::from_name("adoc.no-such-code"), None);
        assert_eq!(DiagnosticCode::from_name(""), None);
    }
}
