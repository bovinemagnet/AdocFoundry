//! The audit report: findings, skips, exclusions and completeness, in a deterministic order.
//!
//! Paths in a report are relative to a workspace root, named by its place in
//! [`AuditReport::roots`], with `/` separators on every platform, so the same workspace gives
//! the same report wherever it is checked out and no absolute path leaves the machine.

use std::path::{Component, Path};

use adoc_antora::{AntoraCoordinate, ResourceFamily};
use adoc_core::{DiagnosticSeverity, SourceRange};
use serde::{Deserialize, Serialize};

use crate::{Applied, FailOn, FindingCode, Policy, RuleId, Verdict, RULES};

/// The version of the report's schema. It changes whenever a field or a rule's meaning does.
pub const SCHEMA_VERSION: u32 = 1;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    /// The severity named `name`: `info`, `warning` or `error`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl From<DiagnosticSeverity> for Severity {
    fn from(severity: DiagnosticSeverity) -> Self {
        match severity {
            DiagnosticSeverity::Information => Self::Info,
            DiagnosticSeverity::Warning => Self::Warning,
            DiagnosticSeverity::Error => Self::Error,
        }
    }
}

/// What was audited.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AuditScope {
    Workspace,
    /// One component; every version of it when `version` is `None`.
    Component {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
}

/// The index and catalog revisions a report was made from, so a front end can tell it is stale.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    pub index: u64,
    pub catalog: u64,
}

/// Where in the workspace something is.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Location {
    /// The workspace root, by its place in [`AuditReport::roots`].
    pub root: usize,
    /// Relative to that root, with `/` separators.
    pub path: String,
    /// Byte offsets in the file, when the finding has a place in it.
    #[serde(default, with = "range", skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
}

impl Location {
    #[must_use]
    pub fn new(root: usize, path: impl Into<String>, range: Option<SourceRange>) -> Self {
        Self {
            root,
            path: path.into(),
            range,
        }
    }
}

/// `relative` with `/` separators, whatever the platform's.
#[must_use]
pub fn portable_path(relative: &Path) -> String {
    let parts: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_owned()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect();
    parts.join("/")
}

/// An Antora resource's coordinate: component, version, module, family and path in the family.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Coordinate {
    pub component: String,
    /// `None` for a versionless component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub module: String,
    #[serde(with = "family")]
    pub family: ResourceFamily,
    /// Relative to the family's folder, with `/` separators.
    pub path: String,
}

impl From<&AntoraCoordinate> for Coordinate {
    fn from(coordinate: &AntoraCoordinate) -> Self {
        Self {
            component: coordinate.component.clone(),
            version: coordinate.version.clone(),
            module: coordinate.module.clone(),
            family: coordinate.family,
            path: portable_path(&coordinate.relative_path),
        }
    }
}

/// Something that supports a finding, such as the navigation file a page was looked for in.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Evidence {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

/// One thing the audit found.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub message: String,
    pub location: Location,
    /// The resource it is about, when the file is an Antora resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<Coordinate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    /// What would put it right, when that is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl Finding {
    /// A finding with no coordinate, evidence or remediation.
    #[must_use]
    pub fn new(
        code: FindingCode,
        severity: Severity,
        message: impl Into<String>,
        location: Location,
    ) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            location,
            coordinate: None,
            evidence: Vec::new(),
            remediation: None,
        }
    }

    /// The report's order: by coordinate, then location, rule and message.
    fn order_key(&self) -> (&Option<Coordinate>, &Location, &str, &str, Severity) {
        (
            &self.coordinate,
            &self.location,
            self.code.as_str(),
            &self.message,
            self.severity,
        )
    }
}

/// A finding the policy excludes, kept in the report with the exclusion's reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Excluded {
    pub finding: Finding,
    pub reason: String,
}

/// A construct a rule could not assert, such as a dynamic include. Counted and listed, but not
/// a gap in coverage.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Skip {
    pub location: Location,
    pub code: FindingCode,
    pub reason: String,
}

/// Why coverage is missing. The names are stable; a new reason comes with a new schema version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    /// A playbook source or referenced component is not in the local catalog.
    RemoteComponentMissing,
    /// An in-scope folder could not be read, or indexing was cut short.
    ScopeNotIndexed,
    /// An enabled optional provider could not run.
    ProviderUnavailable,
}

/// Coverage missing from a report, and what it affects.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Gap {
    pub reason: GapReason,
    pub detail: String,
    /// The components, resources or folders it affects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected: Vec<String>,
}

/// Whether everything in scope was analysed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "gaps", rename_all = "lowercase")]
pub enum Completeness {
    /// Every in-scope resource was analysed by every enabled rule.
    Complete,
    /// Some coverage is missing, for the reasons given.
    Partial(Vec<Gap>),
    /// Nothing in scope could be analysed.
    Unavailable(Vec<Gap>),
}

impl Completeness {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// A rule of the registry and its severity in this report; `None` when the policy turned it off.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuleState {
    pub rule: RuleId,
    #[serde(with = "level")]
    pub severity: Option<Severity>,
}

/// How much of one component version's navigation reaches its pages.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct NavCoverage {
    pub component: String,
    /// `None` for a versionless component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The navigation files its `antora.yml` lists.
    pub navigation_files: usize,
    /// Its published pages: every page but those Antora hides (a name beginning with `_`).
    pub pages: usize,
    /// Pages a navigation entry reaches, each once however many entries reach it; the start
    /// page counts, being linked from the component's title.
    pub listed: usize,
    /// Pages the navigation may or may not reach, because it includes other files, names an
    /// attribute, or could not be read.
    pub undetermined: usize,
}

/// How many findings of each kind a report holds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub info: usize,
    pub warning: usize,
    pub error: usize,
    pub excluded: usize,
    pub skipped: usize,
}

/// A finished audit. It never changes once built; every list in it is in a deterministic order
/// that does not depend on the order findings were found in.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditReport {
    schema_version: u32,
    scope: AuditScope,
    revision: Revision,
    roots: Vec<String>,
    fail_on: FailOn,
    require_complete: bool,
    rules: Vec<RuleState>,
    completeness: Completeness,
    counts: Counts,
    coverage: Vec<NavCoverage>,
    findings: Vec<Finding>,
    excluded: Vec<Excluded>,
    skips: Vec<Skip>,
}

impl AuditReport {
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub fn scope(&self) -> &AuditScope {
        &self.scope
    }

    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The workspace roots, by the names a front end shows; never absolute paths.
    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    #[must_use]
    pub fn fail_on(&self) -> FailOn {
        self.fail_on
    }

    #[must_use]
    pub fn require_complete(&self) -> bool {
        self.require_complete
    }

    /// Every rule of the registry and its severity here.
    #[must_use]
    pub fn rules(&self) -> &[RuleState] {
        &self.rules
    }

    #[must_use]
    pub fn completeness(&self) -> &Completeness {
        &self.completeness
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Navigation coverage of each component version audited, in component and version order.
    #[must_use]
    pub fn coverage(&self) -> &[NavCoverage] {
        &self.coverage
    }

    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    #[must_use]
    pub fn excluded(&self) -> &[Excluded] {
        &self.excluded
    }

    #[must_use]
    pub fn skips(&self) -> &[Skip] {
        &self.skips
    }

    /// What the report means for a gate (PRD §5.4): untrustworthy when unavailable, or partial
    /// while completeness is required; otherwise failed when a finding meets `fail_on`, and
    /// passed if not. Excluded findings never fail it.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        match &self.completeness {
            Completeness::Unavailable(_) => return Verdict::Untrustworthy,
            Completeness::Partial(_) if self.require_complete => return Verdict::Untrustworthy,
            Completeness::Complete | Completeness::Partial(_) => {}
        }
        if self
            .findings
            .iter()
            .any(|finding| self.fail_on.met_by(finding.severity))
        {
            Verdict::Fail
        } else {
            Verdict::Pass
        }
    }
}

/// Collects an audit's findings, skips and gaps in any order, then applies the policy and puts
/// everything in the report's order.
#[derive(Clone, Debug)]
pub struct AuditBuilder {
    scope: AuditScope,
    revision: Revision,
    roots: Vec<String>,
    findings: Vec<Finding>,
    skips: Vec<Skip>,
    coverage: Vec<NavCoverage>,
    partial: Vec<Gap>,
    unavailable: Vec<Gap>,
}

impl AuditBuilder {
    #[must_use]
    pub fn new(scope: AuditScope, revision: Revision, roots: Vec<String>) -> Self {
        Self {
            scope,
            revision,
            roots,
            findings: Vec::new(),
            skips: Vec::new(),
            coverage: Vec::new(),
            partial: Vec::new(),
            unavailable: Vec::new(),
        }
    }

    pub fn finding(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn skip(&mut self, skip: Skip) {
        self.skips.push(skip);
    }

    pub fn coverage(&mut self, coverage: NavCoverage) {
        self.coverage.push(coverage);
    }

    /// Coverage missing from part of the scope: the report is partial.
    pub fn gap(&mut self, gap: Gap) {
        self.partial.push(gap);
    }

    /// Nothing in scope could be analysed: the report is unavailable.
    pub fn unavailable(&mut self, gap: Gap) {
        self.unavailable.push(gap);
    }

    /// The report under `policy`. A rule's findings take the rule's severity from the policy,
    /// and a rule turned off contributes no findings or skips; an existing diagnostic keeps its
    /// own severity. Findings an exclusion matches move to [`AuditReport::excluded`]. Identical
    /// findings are kept once.
    #[must_use]
    pub fn finish(self, policy: &Policy) -> AuditReport {
        let enabled = |code: FindingCode| match code {
            FindingCode::Rule(rule) => policy.severity(rule).is_some(),
            FindingCode::Diagnostic(_) => true,
        };
        let mut findings = Vec::new();
        let mut excluded = Vec::new();
        for finding in self.findings {
            match policy.apply(finding) {
                Applied::Kept(finding) => findings.push(finding),
                Applied::Excluded(finding) => excluded.push(finding),
                Applied::Off => {}
            }
        }
        findings.sort_by(|left, right| left.order_key().cmp(&right.order_key()));
        findings.dedup();
        excluded.sort_by(|left, right| {
            (left.finding.order_key(), &left.reason)
                .cmp(&(right.finding.order_key(), &right.reason))
        });
        excluded.dedup();
        let mut skips: Vec<Skip> = self
            .skips
            .into_iter()
            .filter(|skip| enabled(skip.code))
            .collect();
        skips.sort();
        skips.dedup();

        let completeness = if self.unavailable.is_empty() && self.partial.is_empty() {
            Completeness::Complete
        } else {
            let unavailable = !self.unavailable.is_empty();
            let mut gaps: Vec<Gap> = self.unavailable.into_iter().chain(self.partial).collect();
            gaps.sort();
            gaps.dedup();
            if unavailable {
                Completeness::Unavailable(gaps)
            } else {
                Completeness::Partial(gaps)
            }
        };

        let mut counts = Counts {
            excluded: excluded.len(),
            skipped: skips.len(),
            ..Counts::default()
        };
        for finding in &findings {
            match finding.severity {
                Severity::Info => counts.info += 1,
                Severity::Warning => counts.warning += 1,
                Severity::Error => counts.error += 1,
            }
        }

        let mut coverage = self.coverage;
        coverage.sort();
        coverage.dedup();

        AuditReport {
            schema_version: SCHEMA_VERSION,
            scope: self.scope,
            revision: self.revision,
            roots: self.roots,
            fail_on: policy.fail_on,
            require_complete: policy.require_complete,
            rules: RULES
                .iter()
                .map(|rule| RuleState {
                    rule: rule.id,
                    severity: policy.severity(rule.id),
                })
                .collect(),
            completeness,
            counts,
            coverage,
            findings,
            excluded,
            skips,
        }
    }
}

/// An optional [`SourceRange`] as `{ "start": …, "end": … }`.
mod range {
    use adoc_core::SourceRange;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Range {
        start: usize,
        end: usize,
    }

    pub fn serialize<S: Serializer>(
        range: &Option<SourceRange>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        range
            .map(|range| Range {
                start: range.start,
                end: range.end,
            })
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<SourceRange>, D::Error> {
        Ok(Option::<Range>::deserialize(deserializer)?
            .map(|range| SourceRange::new(range.start, range.end)))
    }
}

/// A [`ResourceFamily`] by its singular name, as resource IDs write it (`page`, `partial`).
mod family {
    use adoc_antora::ResourceFamily;
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        family: &ResourceFamily,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(family)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ResourceFamily, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// A rule's severity in a report: `off`, `info`, `warning` or `error`.
mod level {
    use serde::{de, Deserialize, Deserializer, Serializer};

    use crate::Severity;

    pub fn serialize<S: Serializer>(
        severity: &Option<Severity>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(severity.map_or("off", Severity::as_str))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Severity>, D::Error> {
        let name = String::deserialize(deserializer)?;
        if name == "off" {
            return Ok(None);
        }
        Severity::from_name(&name)
            .map(Some)
            .ok_or_else(|| de::Error::custom(format!("unknown severity `{name}`")))
    }
}

#[cfg(test)]
mod tests {
    use adoc_core::DiagnosticCode;

    use super::*;

    fn page(component: &str, version: Option<&str>, path: &str) -> Coordinate {
        Coordinate {
            component: component.to_owned(),
            version: version.map(str::to_owned),
            module: "ROOT".to_owned(),
            family: ResourceFamily::Page,
            path: path.to_owned(),
        }
    }

    fn finding(code: FindingCode, path: &str, start: usize) -> Finding {
        let mut finding = Finding::new(
            code,
            Severity::Warning,
            format!("about {path}"),
            Location::new(
                0,
                format!("modules/ROOT/pages/{path}"),
                Some(SourceRange::new(start, start + 4)),
            ),
        );
        finding.coordinate = Some(page("product", Some("3.0"), path));
        finding
    }

    fn unlisted(path: &str) -> Finding {
        finding(FindingCode::Rule(RuleId::UNLISTED_PAGE), path, 0)
    }

    fn unresolved(path: &str, start: usize) -> Finding {
        let mut finding = finding(
            FindingCode::Diagnostic(DiagnosticCode::UnresolvedXrefFile),
            path,
            start,
        );
        finding.severity = Severity::Error;
        finding
    }

    fn builder() -> AuditBuilder {
        AuditBuilder::new(
            AuditScope::Workspace,
            Revision {
                index: 7,
                catalog: 3,
            },
            vec!["docs".to_owned()],
        )
    }

    fn report(findings: Vec<Finding>, policy: &Policy) -> AuditReport {
        let mut builder = builder();
        for finding in findings {
            builder.finding(finding);
        }
        builder.finish(policy)
    }

    fn sample() -> AuditReport {
        let mut policy = Policy::default();
        policy
            .add_exclusion(
                RuleId::UNLISTED_PAGE.as_str(),
                "3.0@product:ROOT:internal.adoc",
                "Staff-only page",
            )
            .unwrap();
        let mut builder = AuditBuilder::new(
            AuditScope::Component {
                name: "product".to_owned(),
                version: Some("3.0".to_owned()),
            },
            Revision {
                index: 42,
                catalog: 5,
            },
            vec!["docs".to_owned()],
        );
        let mut install = unlisted("install.adoc");
        install.evidence.push(Evidence {
            message: "Not listed in modules/ROOT/nav.adoc".to_owned(),
            location: Some(Location::new(0, "modules/ROOT/nav.adoc", None)),
        });
        install.remediation = Some("Add an entry to a navigation file".to_owned());
        builder.finding(install);
        builder.finding(unlisted("internal.adoc"));
        builder.finding(unresolved("index.adoc", 120));
        builder.skip(Skip {
            location: Location::new(
                0,
                "modules/ROOT/pages/index.adoc",
                Some(SourceRange::new(300, 330)),
            ),
            code: FindingCode::Diagnostic(DiagnosticCode::UnresolvedInclude),
            reason: "The include target uses the attribute `{snippets}`".to_owned(),
        });
        builder.coverage(NavCoverage {
            component: "product".to_owned(),
            version: Some("3.0".to_owned()),
            navigation_files: 1,
            pages: 4,
            listed: 2,
            undetermined: 0,
        });
        builder.gap(Gap {
            reason: GapReason::RemoteComponentMissing,
            detail: "The playbook names a source that is not checked out".to_owned(),
            affected: vec!["platform".to_owned()],
        });
        builder.finish(&policy)
    }

    #[test]
    fn the_report_schema_is_stable() {
        insta::assert_json_snapshot!(sample());
    }

    #[test]
    fn reports_round_trip_through_json() {
        let partial = sample();
        let text = serde_json::to_string(&partial).unwrap();
        assert_eq!(serde_json::from_str::<AuditReport>(&text).unwrap(), partial);

        let complete = report(vec![unlisted("a.adoc")], &Policy::default());
        let text = serde_json::to_string(&complete).unwrap();
        assert_eq!(
            serde_json::from_str::<AuditReport>(&text).unwrap(),
            complete
        );
    }

    #[test]
    fn the_order_does_not_depend_on_the_order_found() {
        let found = vec![
            unresolved("b.adoc", 90),
            unlisted("b.adoc"),
            unresolved("a.adoc", 10),
            unresolved("b.adoc", 20),
            unlisted("a.adoc"),
        ];
        let forwards = report(found.clone(), &Policy::default());
        let backwards = report(found.into_iter().rev().collect(), &Policy::default());
        assert_eq!(forwards, backwards);
        let order: Vec<(&str, usize)> = forwards
            .findings()
            .iter()
            .map(|finding| (finding.code.as_str(), finding.location.range.unwrap().start))
            .collect();
        assert_eq!(
            order,
            [
                ("quality.navigation.unlisted-page", 0),
                ("adoc.unresolved-xref-file", 10),
                ("quality.navigation.unlisted-page", 0),
                ("adoc.unresolved-xref-file", 20),
                ("adoc.unresolved-xref-file", 90),
            ],
            "by coordinate, then position, then rule"
        );
    }

    #[test]
    fn identical_findings_are_kept_once() {
        let report = report(
            vec![unlisted("a.adoc"), unlisted("a.adoc")],
            &Policy::default(),
        );
        assert_eq!(report.findings().len(), 1);
    }

    #[test]
    fn rules_take_the_policy_severity_and_diagnostics_keep_their_own() {
        let mut policy = Policy::default();
        policy
            .set_severity(RuleId::UNLISTED_PAGE.as_str(), "error")
            .unwrap();
        let mut info = unresolved("a.adoc", 5);
        info.severity = Severity::Info;
        let report = report(vec![unlisted("a.adoc"), info], &policy);
        let severities: Vec<(&str, Severity)> = report
            .findings()
            .iter()
            .map(|finding| (finding.code.as_str(), finding.severity))
            .collect();
        assert_eq!(
            severities,
            [
                ("quality.navigation.unlisted-page", Severity::Error),
                ("adoc.unresolved-xref-file", Severity::Info),
            ]
        );
        assert_eq!(report.counts().error, 1);
        assert_eq!(report.counts().info, 1);
    }

    #[test]
    fn a_rule_turned_off_reports_nothing() {
        let mut policy = Policy::default();
        policy
            .set_severity(RuleId::UNLISTED_PAGE.as_str(), "off")
            .unwrap();
        let mut builder = builder();
        builder.finding(unlisted("a.adoc"));
        builder.skip(Skip {
            location: Location::new(0, "a.adoc", None),
            code: FindingCode::Rule(RuleId::UNLISTED_PAGE),
            reason: "Conditional navigation entry".to_owned(),
        });
        let report = builder.finish(&policy);
        assert!(report.findings().is_empty());
        assert!(report.skips().is_empty());
        let state = report
            .rules()
            .iter()
            .find(|state| state.rule == RuleId::UNLISTED_PAGE)
            .unwrap();
        assert_eq!(state.severity, None);
    }

    #[test]
    fn excluded_findings_are_kept_with_their_reason_and_never_fail() {
        let mut policy = Policy::default();
        policy
            .add_exclusion(
                DiagnosticCode::UnresolvedXrefFile.as_str(),
                "product:ROOT:a.adoc",
                "Target page lands in the next release",
            )
            .unwrap();
        let report = report(vec![unresolved("a.adoc", 5)], &policy);
        assert!(report.findings().is_empty());
        assert_eq!(report.excluded().len(), 1);
        assert_eq!(
            report.excluded()[0].reason,
            "Target page lands in the next release"
        );
        assert_eq!(report.counts().excluded, 1);
        assert_eq!(report.verdict(), Verdict::Pass);
    }

    #[test]
    fn skips_do_not_make_a_report_partial() {
        let mut builder = builder();
        builder.skip(Skip {
            location: Location::new(0, "a.adoc", None),
            code: FindingCode::Diagnostic(DiagnosticCode::UnresolvedInclude),
            reason: "Dynamic include".to_owned(),
        });
        let report = builder.finish(&Policy::default());
        assert!(report.completeness().is_complete());
        assert_eq!(report.counts().skipped, 1);
    }

    #[test]
    fn any_unavailable_gap_makes_the_report_unavailable() {
        let mut builder = builder();
        builder.gap(Gap {
            reason: GapReason::ScopeNotIndexed,
            detail: "modules/legacy could not be read".to_owned(),
            affected: Vec::new(),
        });
        builder.unavailable(Gap {
            reason: GapReason::RemoteComponentMissing,
            detail: "Component `platform` is not checked out".to_owned(),
            affected: vec!["platform".to_owned()],
        });
        let report = builder.finish(&Policy::default());
        let Completeness::Unavailable(gaps) = report.completeness() else {
            panic!("expected unavailable, got {:?}", report.completeness());
        };
        assert_eq!(gaps.len(), 2, "every gap is kept, in order");
        assert_eq!(gaps[0].reason, GapReason::RemoteComponentMissing);
    }

    #[test]
    fn verdicts_follow_the_ci_policy() {
        fn verdict(
            completeness: &str,
            severity: Option<Severity>,
            fail_on: FailOn,
            strict: bool,
        ) -> Verdict {
            let mut policy = Policy::default();
            policy.fail_on = fail_on;
            policy.require_complete = strict;
            let mut builder = builder();
            if let Some(severity) = severity {
                let mut finding = unresolved("a.adoc", 0);
                finding.severity = severity;
                builder.finding(finding);
            }
            let gap = Gap {
                reason: GapReason::ScopeNotIndexed,
                detail: String::new(),
                affected: Vec::new(),
            };
            match completeness {
                "partial" => builder.gap(gap),
                "unavailable" => builder.unavailable(gap),
                _ => {}
            }
            builder.finish(&policy).verdict()
        }
        use Severity::{Error, Info, Warning};
        use Verdict::{Fail, Pass, Untrustworthy};
        let cases = [
            ("complete", None, FailOn::Error, false, Pass),
            ("complete", Some(Warning), FailOn::Error, false, Pass),
            ("complete", Some(Error), FailOn::Error, false, Fail),
            ("complete", Some(Warning), FailOn::Warning, false, Fail),
            ("complete", Some(Info), FailOn::Warning, false, Pass),
            ("complete", Some(Error), FailOn::None, false, Pass),
            ("partial", None, FailOn::Error, false, Pass),
            ("partial", Some(Error), FailOn::Error, false, Fail),
            ("partial", None, FailOn::Error, true, Untrustworthy),
            ("partial", Some(Error), FailOn::Error, true, Untrustworthy),
            ("unavailable", None, FailOn::None, false, Untrustworthy),
            (
                "unavailable",
                Some(Error),
                FailOn::Error,
                true,
                Untrustworthy,
            ),
        ];
        for (completeness, severity, fail_on, strict, expected) in cases {
            assert_eq!(
                verdict(completeness, severity, fail_on, strict),
                expected,
                "{completeness}, {severity:?}, fail_on {fail_on:?}, strict {strict}"
            );
        }
        assert_eq!(
            [Pass, Fail, Untrustworthy].map(Verdict::exit_code),
            [0, 1, 2]
        );
    }

    #[test]
    fn paths_use_forward_slashes() {
        let relative: std::path::PathBuf = ["modules", "ROOT", "pages", "a.adoc"].iter().collect();
        assert_eq!(portable_path(&relative), "modules/ROOT/pages/a.adoc");
        let coordinate = AntoraCoordinate {
            component: "product".to_owned(),
            version: None,
            module: "ROOT".to_owned(),
            family: ResourceFamily::Partial,
            relative_path: ["shared", "auth.adoc"].iter().collect(),
        };
        let coordinate = Coordinate::from(&coordinate);
        assert_eq!(coordinate.path, "shared/auth.adoc");
        assert_eq!(coordinate.version, None);
    }
}
