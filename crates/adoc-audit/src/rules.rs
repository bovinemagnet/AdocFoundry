//! Rule identifiers and the registry of the audit's own rules.
//!
//! A rule identifier is permanent once released. If what a rule means changes materially, the
//! report's [`crate::SCHEMA_VERSION`] changes with it.

use std::{cmp::Ordering, fmt};

use adoc_core::DiagnosticCode;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::Severity;

/// One of the audit's own rules, named `quality.<category>.<name>`. Only the registry
/// ([`RULES`]) makes them, so every `RuleId` is a known rule.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuleId(&'static str);

impl RuleId {
    /// A page absent from every effective navigation tree of its component version.
    pub const UNLISTED_PAGE: Self = Self("quality.navigation.unlisted-page");
    /// A page with no usable document title.
    pub const MISSING_TITLE: Self = Self("quality.page.missing-title");
    /// An image macro with no meaningful alternative text.
    pub const MISSING_ALT: Self = Self("quality.image.missing-alt");

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The registered rule named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        RULES.iter().map(|rule| rule.id).find(|id| id.0 == name)
    }

    /// The rule's entry in the registry.
    #[must_use]
    pub fn spec(self) -> &'static RuleSpec {
        RULES
            .iter()
            .find(|rule| rule.id == self)
            .expect("every RuleId is registered")
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Serialize for RuleId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for RuleId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::from_name(&name).ok_or_else(|| de::Error::custom(format!("unknown rule `{name}`")))
    }
}

/// What a finding is about, for grouping and filtering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Category {
    Navigation,
    Page,
    Image,
    References,
    Descriptor,
}

/// A rule in the registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleSpec {
    pub id: RuleId,
    pub category: Category,
    /// Its severity unless the policy sets another.
    pub default_severity: Severity,
    /// One line saying what it finds.
    pub summary: &'static str,
}

/// The audit's own rules, in name order.
pub const RULES: &[RuleSpec] = &[
    RuleSpec {
        id: RuleId::MISSING_ALT,
        category: Category::Image,
        default_severity: Severity::Warning,
        summary: "An image macro has no meaningful alternative text",
    },
    RuleSpec {
        id: RuleId::UNLISTED_PAGE,
        category: Category::Navigation,
        default_severity: Severity::Warning,
        summary: "A page is absent from every navigation tree of its component version",
    },
    RuleSpec {
        id: RuleId::MISSING_TITLE,
        category: Category::Page,
        default_severity: Severity::Warning,
        summary: "A page has no usable document title",
    },
];

/// What a finding reports: one of the audit's rules, or an existing diagnostic aggregated
/// unchanged. Ordered, displayed and serialised by its stable name.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FindingCode {
    Rule(RuleId),
    Diagnostic(DiagnosticCode),
}

impl FindingCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rule(rule) => rule.as_str(),
            Self::Diagnostic(code) => code.as_str(),
        }
    }

    /// The rule or diagnostic code named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        RuleId::from_name(name)
            .map(Self::Rule)
            .or_else(|| DiagnosticCode::from_name(name).map(Self::Diagnostic))
    }

    #[must_use]
    pub fn category(self) -> Category {
        match self {
            Self::Rule(rule) => rule.spec().category,
            Self::Diagnostic(code) => diagnostic_category(code),
        }
    }
}

impl Ord for FindingCode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for FindingCode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for FindingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for FindingCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FindingCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::from_name(&name)
            .ok_or_else(|| de::Error::custom(format!("unknown rule or diagnostic code `{name}`")))
    }
}

/// The category an existing diagnostic is grouped under. Exhaustive, so a new diagnostic code
/// is placed before it compiles.
fn diagnostic_category(code: DiagnosticCode) -> Category {
    match code {
        DiagnosticCode::AntoraNavUnresolved
        | DiagnosticCode::AntoraMissingNavFile
        | DiagnosticCode::AntoraUnresolvedStartPage => Category::Navigation,
        DiagnosticCode::UnresolvedXrefFile
        | DiagnosticCode::UnresolvedAnchor
        | DiagnosticCode::UnresolvedInclude
        | DiagnosticCode::UnresolvedImage
        | DiagnosticCode::DuplicateAnchor
        | DiagnosticCode::AntoraUnknownModule
        | DiagnosticCode::AntoraUnknownResource
        | DiagnosticCode::AntoraInvalidFamily
        | DiagnosticCode::AntoraInvalidCoordinate
        | DiagnosticCode::AntoraResolvedViaAlias => Category::References,
        DiagnosticCode::AntoraInvalidDescriptor
        | DiagnosticCode::AntoraVersionNotOnBranch
        | DiagnosticCode::AntoraMissingKey
        | DiagnosticCode::AntoraUnknownKey
        | DiagnosticCode::AntoraInvalidAttributeValue => Category::Descriptor,
        DiagnosticCode::BrokenCodeSpan => Category::Page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_quality_rules_in_name_order_with_unique_names() {
        let names: Vec<&str> = RULES.iter().map(|rule| rule.id.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "registered in name order, each once");
        assert!(names.iter().all(|name| name.starts_with("quality.")));
    }

    #[test]
    fn no_rule_shares_a_name_with_a_diagnostic() {
        for rule in RULES {
            assert_eq!(DiagnosticCode::from_name(rule.id.as_str()), None);
        }
    }

    #[test]
    fn finding_codes_read_back_from_their_names() {
        for rule in RULES {
            let code = FindingCode::Rule(rule.id);
            assert_eq!(FindingCode::from_name(code.as_str()), Some(code));
            assert_eq!(code.category(), rule.category);
        }
        for diagnostic in DiagnosticCode::ALL {
            let code = FindingCode::Diagnostic(diagnostic);
            assert_eq!(FindingCode::from_name(code.as_str()), Some(code));
        }
        assert_eq!(FindingCode::from_name("quality.no-such-rule"), None);
    }

    #[test]
    fn nav_diagnostics_are_navigation_findings() {
        assert_eq!(
            FindingCode::Diagnostic(DiagnosticCode::AntoraNavUnresolved).category(),
            Category::Navigation
        );
        assert_eq!(
            FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile).category(),
            Category::References
        );
    }

    #[test]
    fn codes_order_by_name_not_by_kind() {
        let diagnostic = FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile);
        let rule = FindingCode::Rule(RuleId::UNLISTED_PAGE);
        assert!(diagnostic < rule, "`adoc.` sorts before `quality.`");
    }

    #[test]
    fn unknown_names_do_not_deserialise() {
        assert!(serde_json::from_str::<FindingCode>("\"quality.no-such-rule\"").is_err());
        assert!(serde_json::from_str::<RuleId>("\"adoc.unresolved-anchor\"").is_err());
    }
}
