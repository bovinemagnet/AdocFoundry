//! The workspace's audit policy: rule severities, exclusions, and what fails a gate.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use adoc_antora::{parse_resource_id, ResourceFamily};
use serde::{Deserialize, Serialize};

use crate::{Excluded, Finding, FindingCode, ProviderId, RuleId, Severity};

/// The lowest severity that fails a gate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailOn {
    /// No finding fails it.
    None,
    Warning,
    #[default]
    Error,
}

impl FailOn {
    /// `none`, `warning` or `error`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    /// Whether a finding of `severity` fails the gate.
    #[must_use]
    pub fn met_by(self, severity: Severity) -> bool {
        match self {
            Self::None => false,
            Self::Warning => severity >= Severity::Warning,
            Self::Error => severity == Severity::Error,
        }
    }
}

/// What a report means for a gate, and the exit code a command-line front end gives it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Below the failure threshold.
    Pass,
    /// A finding meets the failure threshold.
    Fail,
    /// The report cannot be trusted as a result: unavailable, or partial while completeness is
    /// required.
    Untrustworthy,
}

impl Verdict {
    /// `0`, `1` or `2`.
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
            Self::Untrustworthy => 2,
        }
    }
}

/// The versions of a component an exclusion applies to.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetVersion {
    /// No version given: every version of the component.
    Any,
    /// `~@`: only the versionless component.
    Versionless,
    Named(String),
}

/// A finding the policy excludes for one resource, with the reason it is acceptable.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Exclusion {
    pub code: FindingCode,
    pub component: String,
    pub version: TargetVersion,
    pub module: String,
    pub family: ResourceFamily,
    /// Relative to the family's folder, with `/` separators.
    pub path: String,
    pub reason: String,
}

impl Exclusion {
    /// An exclusion of `rule` (a rule or diagnostic code) for `resource`, a resource ID naming
    /// at least its component and module (`product:ROOT:internal.adoc`), with a reason.
    pub fn new(rule: &str, resource: &str, reason: &str) -> Result<Self, PolicyError> {
        let code = FindingCode::from_name(rule)
            .ok_or_else(|| PolicyError::UnknownRule(rule.to_owned()))?;
        let id = parse_resource_id(resource).map_err(|problem| PolicyError::InvalidResource {
            resource: resource.to_owned(),
            problem: problem.to_string(),
        })?;
        let (Some(component), Some(module)) = (id.component, id.module) else {
            return Err(PolicyError::UnqualifiedResource(resource.to_owned()));
        };
        if reason.trim().is_empty() {
            return Err(PolicyError::MissingReason {
                rule: rule.to_owned(),
                resource: resource.to_owned(),
            });
        }
        let version = match id.version.as_deref() {
            None => TargetVersion::Any,
            Some("~") => TargetVersion::Versionless,
            Some(version) => TargetVersion::Named(version.to_owned()),
        };
        Ok(Self {
            code,
            component,
            version,
            module,
            family: id.family.unwrap_or(ResourceFamily::Page),
            path: id.path,
            reason: reason.to_owned(),
        })
    }

    /// Whether it excludes `finding`: the same code, for this resource. A finding with no
    /// coordinate is never excluded.
    #[must_use]
    pub fn matches(&self, finding: &Finding) -> bool {
        let Some(coordinate) = &finding.coordinate else {
            return false;
        };
        let version = match &self.version {
            TargetVersion::Any => true,
            TargetVersion::Versionless => coordinate.version.is_none(),
            TargetVersion::Named(version) => coordinate.version.as_deref() == Some(version),
        };
        finding.code == self.code
            && version
            && coordinate.component == self.component
            && coordinate.module == self.module
            && coordinate.family == self.family
            && coordinate.path == self.path
    }

    fn target(
        &self,
    ) -> (
        FindingCode,
        &str,
        &TargetVersion,
        &str,
        ResourceFamily,
        &str,
    ) {
        (
            self.code,
            &self.component,
            &self.version,
            &self.module,
            self.family,
            &self.path,
        )
    }
}

/// A setting the policy refuses, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyError {
    UnknownRule(String),
    UnknownProvider(String),
    /// An existing diagnostic's severity cannot be changed by the audit.
    NotConfigurable(String),
    InvalidLevel(String),
    InvalidResource {
        resource: String,
        problem: String,
    },
    UnqualifiedResource(String),
    MissingReason {
        rule: String,
        resource: String,
    },
    DuplicateExclusion {
        rule: String,
        resource: String,
    },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRule(rule) => write!(formatter, "unknown rule `{rule}`"),
            Self::UnknownProvider(provider) => write!(formatter, "unknown provider `{provider}`"),
            Self::NotConfigurable(code) => write!(
                formatter,
                "`{code}` is an existing diagnostic and keeps its own severity"
            ),
            Self::InvalidLevel(level) => write!(
                formatter,
                "expected \"off\", \"info\", \"warning\" or \"error\", found `{level}`"
            ),
            Self::InvalidResource { resource, problem } => {
                write!(formatter, "`{resource}`: {problem}")
            }
            Self::UnqualifiedResource(resource) => write!(
                formatter,
                "`{resource}` must name its component and module, as `component:module:page.adoc`"
            ),
            Self::MissingReason { rule, resource } => write!(
                formatter,
                "the exclusion of `{rule}` for `{resource}` needs a reason"
            ),
            Self::DuplicateExclusion { rule, resource } => {
                write!(formatter, "`{rule}` is already excluded for `{resource}`")
            }
        }
    }
}

impl std::error::Error for PolicyError {}

/// How an audit is judged: each rule's severity, the exclusions, the optional providers enabled,
/// and the gate.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Policy {
    pub fail_on: FailOn,
    /// Anything short of complete coverage is untrustworthy ([`Verdict::Untrustworthy`]).
    pub require_complete: bool,
    /// Severities set for rules; `None` turns a rule off.
    severities: BTreeMap<RuleId, Option<Severity>>,
    exclusions: Vec<Exclusion>,
    /// The optional providers to run; none by default.
    providers: BTreeSet<ProviderId>,
}

impl Policy {
    /// The severity of `rule`'s findings: the one set, else the rule's default; `None` when off.
    #[must_use]
    pub fn severity(&self, rule: RuleId) -> Option<Severity> {
        self.severities
            .get(&rule)
            .copied()
            .unwrap_or(Some(rule.spec().default_severity))
    }

    /// Sets `rule` to `level`: `off`, `info`, `warning` or `error`. Only the audit's own rules
    /// can be set; an existing diagnostic keeps its severity.
    pub fn set_severity(&mut self, rule: &str, level: &str) -> Result<(), PolicyError> {
        let rule = match FindingCode::from_name(rule) {
            Some(FindingCode::Rule(rule)) => rule,
            Some(FindingCode::Diagnostic(_)) => {
                return Err(PolicyError::NotConfigurable(rule.to_owned()))
            }
            None => return Err(PolicyError::UnknownRule(rule.to_owned())),
        };
        let severity = match level {
            "off" => None,
            level => Some(
                Severity::from_name(level)
                    .ok_or_else(|| PolicyError::InvalidLevel(level.to_owned()))?,
            ),
        };
        self.severities.insert(rule, severity);
        Ok(())
    }

    /// Adds an exclusion ([`Exclusion::new`]), refusing a second one for the same rule and
    /// resource.
    pub fn add_exclusion(
        &mut self,
        rule: &str,
        resource: &str,
        reason: &str,
    ) -> Result<(), PolicyError> {
        let exclusion = Exclusion::new(rule, resource, reason)?;
        if self
            .exclusions
            .iter()
            .any(|known| known.target() == exclusion.target())
        {
            return Err(PolicyError::DuplicateExclusion {
                rule: rule.to_owned(),
                resource: resource.to_owned(),
            });
        }
        self.exclusions.push(exclusion);
        Ok(())
    }

    #[must_use]
    pub fn exclusions(&self) -> &[Exclusion] {
        &self.exclusions
    }

    /// Enables the optional provider named `name` (`crate::PROVIDERS`).
    pub fn enable_provider(&mut self, name: &str) -> Result<(), PolicyError> {
        let provider = ProviderId::from_name(name)
            .ok_or_else(|| PolicyError::UnknownProvider(name.to_owned()))?;
        self.providers.insert(provider);
        Ok(())
    }

    #[must_use]
    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        self.providers.contains(&provider)
    }

    /// The first exclusion matching `finding`.
    #[must_use]
    pub fn exclusion_for(&self, finding: &Finding) -> Option<&Exclusion> {
        self.exclusions
            .iter()
            .find(|exclusion| exclusion.matches(finding))
    }

    /// What the policy makes of `finding`, as a report under it has it: dropped when its rule is
    /// off, else at its rule's severity (an existing diagnostic keeping its own), excluded when
    /// an exclusion matches it.
    #[must_use]
    pub fn apply(&self, mut finding: Finding) -> Applied {
        if let FindingCode::Rule(rule) = finding.code {
            match self.severity(rule) {
                Some(severity) => finding.severity = severity,
                None => return Applied::Off,
            }
        }
        match self.exclusion_for(&finding) {
            Some(exclusion) => Applied::Excluded(Excluded {
                reason: exclusion.reason.clone(),
                finding,
            }),
            None => Applied::Kept(finding),
        }
    }
}

/// A finding under a [`Policy`] ([`Policy::apply`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Applied {
    /// Reported.
    Kept(Finding),
    /// Counted as excluded, not reported.
    Excluded(Excluded),
    /// Its rule is off: neither reported nor counted.
    Off,
}

#[cfg(test)]
mod tests {
    use adoc_core::DiagnosticCode;

    use super::*;
    use crate::{Coordinate, Location};

    fn page(version: Option<&str>, path: &str) -> Finding {
        let mut finding = Finding::new(
            FindingCode::Rule(RuleId::UNLISTED_PAGE),
            Severity::Warning,
            "Not in navigation",
            Location::new(0, format!("modules/ROOT/pages/{path}"), None),
        );
        finding.coordinate = Some(Coordinate {
            component: "product".to_owned(),
            version: version.map(str::to_owned),
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
            path: path.to_owned(),
        });
        finding
    }

    const UNLISTED: &str = "quality.navigation.unlisted-page";

    #[test]
    fn defaults_fail_on_errors_and_allow_partial_reports() {
        let policy = Policy::default();
        assert_eq!(policy.fail_on, FailOn::Error);
        assert!(!policy.require_complete);
        assert_eq!(
            policy.severity(RuleId::UNLISTED_PAGE),
            Some(Severity::Warning)
        );
    }

    #[test]
    fn fail_on_names() {
        assert_eq!(FailOn::from_name("none"), Some(FailOn::None));
        assert_eq!(FailOn::from_name("warning"), Some(FailOn::Warning));
        assert_eq!(FailOn::from_name("error"), Some(FailOn::Error));
        assert_eq!(FailOn::from_name("info"), None);
    }

    #[test]
    fn rule_severities_are_validated() {
        let mut policy = Policy::default();
        policy.set_severity(UNLISTED, "info").unwrap();
        assert_eq!(policy.severity(RuleId::UNLISTED_PAGE), Some(Severity::Info));
        policy.set_severity(UNLISTED, "off").unwrap();
        assert_eq!(policy.severity(RuleId::UNLISTED_PAGE), None);
        assert_eq!(
            policy.set_severity("quality.no-such-rule", "error"),
            Err(PolicyError::UnknownRule("quality.no-such-rule".to_owned()))
        );
        assert_eq!(
            policy.set_severity("adoc.unresolved-xref-file", "info"),
            Err(PolicyError::NotConfigurable(
                "adoc.unresolved-xref-file".to_owned()
            ))
        );
        assert_eq!(
            policy.set_severity(UNLISTED, "fatal"),
            Err(PolicyError::InvalidLevel("fatal".to_owned()))
        );
    }

    #[test]
    fn exclusions_need_a_known_rule_a_qualified_resource_and_a_reason() {
        let mut policy = Policy::default();
        assert!(matches!(
            policy.add_exclusion("quality.no-such-rule", "product:ROOT:a.adoc", "Why"),
            Err(PolicyError::UnknownRule(_))
        ));
        assert!(matches!(
            policy.add_exclusion(UNLISTED, "product:ROOT:bad$a.adoc", "Why"),
            Err(PolicyError::InvalidResource { .. })
        ));
        assert!(matches!(
            policy.add_exclusion(UNLISTED, "a.adoc", "Why"),
            Err(PolicyError::UnqualifiedResource(_))
        ));
        assert!(matches!(
            policy.add_exclusion(UNLISTED, "product:ROOT:a.adoc", "  "),
            Err(PolicyError::MissingReason { .. })
        ));
        assert!(policy.exclusions().is_empty());
    }

    #[test]
    fn a_second_exclusion_for_the_same_resource_is_refused() {
        let mut policy = Policy::default();
        policy
            .add_exclusion(UNLISTED, "product:ROOT:a.adoc", "Staff only")
            .unwrap();
        assert!(matches!(
            policy.add_exclusion(UNLISTED, "product:ROOT:page$a.adoc", "Another reason"),
            Err(PolicyError::DuplicateExclusion { .. })
        ));
        policy
            .add_exclusion(UNLISTED, "3.0@product:ROOT:a.adoc", "Only in 3.0")
            .unwrap();
        assert_eq!(policy.exclusions().len(), 2);
    }

    #[test]
    fn exclusions_match_their_resource_and_versions() {
        let any = Exclusion::new(UNLISTED, "product:ROOT:a.adoc", "Why").unwrap();
        let named = Exclusion::new(UNLISTED, "3.0@product:ROOT:a.adoc", "Why").unwrap();
        let versionless = Exclusion::new(UNLISTED, "~@product:ROOT:a.adoc", "Why").unwrap();
        let versioned = page(Some("3.0"), "a.adoc");
        let older = page(Some("2.0"), "a.adoc");
        let unversioned = page(None, "a.adoc");
        assert!(any.matches(&versioned) && any.matches(&older) && any.matches(&unversioned));
        assert!(
            named.matches(&versioned) && !named.matches(&older) && !named.matches(&unversioned)
        );
        assert!(versionless.matches(&unversioned) && !versionless.matches(&versioned));
        assert!(!any.matches(&page(Some("3.0"), "b.adoc")));

        let mut other_rule = versioned.clone();
        other_rule.code = FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile);
        assert!(!any.matches(&other_rule));
        let mut no_coordinate = versioned;
        no_coordinate.coordinate = None;
        assert!(!any.matches(&no_coordinate));
    }

    #[test]
    fn errors_read_as_settings_warnings() {
        assert_eq!(
            PolicyError::UnqualifiedResource("a.adoc".to_owned()).to_string(),
            "`a.adoc` must name its component and module, as `component:module:page.adoc`"
        );
    }

    #[test]
    fn applying_the_policy_sets_severity_drops_and_excludes() {
        let mut policy = Policy::default();
        policy.set_severity(UNLISTED, "error").unwrap();
        policy
            .add_exclusion(UNLISTED, "3.0@product:ROOT:a.adoc", "Reached otherwise")
            .unwrap();
        let Applied::Kept(kept) = policy.apply(page(Some("3.0"), "b.adoc")) else {
            panic!("b.adoc is kept");
        };
        assert_eq!(kept.severity, Severity::Error);
        assert!(matches!(
            policy.apply(page(Some("3.0"), "a.adoc")),
            Applied::Excluded(Excluded { reason, .. }) if reason == "Reached otherwise"
        ));
        let mut diagnostic = page(Some("3.0"), "b.adoc");
        diagnostic.code = FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile);
        diagnostic.severity = Severity::Info;
        assert_eq!(
            policy.apply(diagnostic.clone()),
            Applied::Kept(diagnostic),
            "a diagnostic keeps its own severity"
        );
        policy.set_severity(UNLISTED, "off").unwrap();
        assert_eq!(policy.apply(page(Some("3.0"), "b.adoc")), Applied::Off);
    }

    #[test]
    fn providers_are_off_until_enabled_by_name() {
        let mut policy = Policy::default();
        assert!(!policy.provider_enabled(ProviderId::VALE));
        policy.enable_provider("vale").unwrap();
        assert!(policy.provider_enabled(ProviderId::VALE));
        assert!(!policy.provider_enabled(ProviderId::EXTERNAL_LINKS));
        assert_eq!(
            policy.enable_provider("spellcheck"),
            Err(PolicyError::UnknownProvider("spellcheck".to_owned()))
        );
        assert_eq!(
            PolicyError::UnknownProvider("spellcheck".to_owned()).to_string(),
            "unknown provider `spellcheck`"
        );
    }
}
