//! The documentation quality audit's model: stable rule identifiers and the rule registry
//! ([`RULES`]), the report ([`AuditReport`], built with [`AuditBuilder`]), the workspace policy
//! ([`Policy`]: rule severities, exclusions, `fail_on`, `require_complete`) and what a report
//! means for a gate ([`Verdict`]).
//!
//! Everything here is pure and synchronous: no threads, no settings files, no filesystem
//! writes, no subprocesses and no network. The engine runs audits on its workers, reads the
//! `[quality]` settings and keeps results current; front ends reach this crate only through
//! `adoc-engine`.
//!
//! # Existing diagnostics
//!
//! The audit does not restate what the existing diagnostics already report. A finding for one
//! of them carries its [`adoc_core::DiagnosticCode`] ([`FindingCode::Diagnostic`]), its severity
//! and its message, unchanged but for paths beneath a workspace root, which are written relative
//! to it so that no absolute path leaves the machine in a report. No `quality.*` rule is ever
//! defined for the same fault: an unresolved navigation entry, for one, is the navigation file's
//! `adoc.unresolved-xref-file`. The policy can exclude such a finding for one resource, but
//! cannot change its severity.
//!
//! # Completeness and skips
//!
//! A report's [`Completeness`] is about coverage: whether every resource in scope was analysed
//! by every enabled rule. A construct a rule cannot assert, such as a dynamic include, is a
//! [`Skip`], counted and listed, and does not make a report partial.

mod content;
mod navigation;
mod policy;
mod report;
mod rules;
mod sarif;
mod workspace;

pub use policy::{Exclusion, FailOn, Policy, PolicyError, TargetVersion, Verdict};
pub use report::{
    portable_path, AuditBuilder, AuditReport, AuditScope, Completeness, Coordinate, Counts,
    Evidence, Excluded, Finding, Gap, GapReason, Location, NavCoverage, Revision, RuleState,
    Severity, Skip, SCHEMA_VERSION,
};
pub use rules::{Category, FindingCode, RuleId, RuleSpec, RULES};
pub use sarif::{to_sarif, Region};
pub use workspace::{
    assemble, audit, audit_file, audit_version, audited_files, AuditInput, FileAudit, VersionAudit,
};
