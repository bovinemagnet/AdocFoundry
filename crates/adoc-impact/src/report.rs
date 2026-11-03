//! The impact report: what changed, what it affects and why, and how far the analysis reached.
//! Deterministic (sorted, root-relative `/` paths, no absolute path) and schema-versioned.

use std::path::PathBuf;

use adoc_core::SourceRange;
use serde::Serialize;

/// The version of the report's JSON. Additive changes keep it.
pub const SCHEMA_VERSION: u32 = 1;

/// How a file changed between the two sides of the comparison.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
    /// Moved from `from` to the change's path.
    Renamed {
        from: PathBuf,
    },
}

/// One changed file, by its absolute path (on the head side for a rename).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Change {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// What an impact is about (PRD §5.1).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactKind {
    /// The page's rendered content may change: it is the changed page, or includes it.
    Render,
    /// A link to the changed resource or one of its anchors may no longer lead where it did.
    ReferenceIntegrity,
    /// A component version's navigation changed, or lists a changed page.
    Navigation,
    /// A publication profile selects affected content.
    Publication,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The evidence shows it.
    Confirmed,
    /// It may be so; the evidence is incomplete (an unknown selection, a context not known).
    Potential,
}

/// Which side's evidence an impact rests on.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSide {
    Base,
    Head,
    Both,
}

/// What became of a reference or entry the change touches.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactStatus {
    /// Still written, and no longer leads where it did: to review.
    Broken,
    /// The referrer no longer names the old target, or is gone: resolved by the change.
    ResolvedByChange,
    /// Still names the old place, which a `page-aliases` entry of the moved page now covers.
    ResolvedByAlias,
    /// Unresolved before the change, and resolves now.
    NowResolves,
    /// A navigation file now lists the page.
    Listed,
    /// A navigation file no longer lists the page.
    Unlisted,
}

/// A byte range: the first byte and just past the last.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl From<SourceRange> for ByteRange {
    fn from(range: SourceRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

/// Where a referrer writes its reference: a file and a byte range in it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceSpan {
    /// Root-relative.
    pub path: String,
    pub range: ByteRange,
}

/// One affected item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Impact {
    pub kind: ImpactKind,
    pub confidence: Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ImpactStatus>,
    /// The affected file, root-relative.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// `None` for the versionless version or a file outside every component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For a publication impact, the profile's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
    /// The changed file this impact comes from, root-relative.
    pub origin: String,
    pub evidence: EvidenceSide,
    /// The dependency path, from the changed file to the affected one, root-relative.
    pub via: Vec<String>,
    /// The reference or entry that links them, where there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSpan>,
    /// The anchor a reference-integrity impact concerns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

/// Why the analysis did not cover everything.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Gap {
    /// A stable name: `no-base-revision`, `base-unavailable`, `asset-consumers-not-analysed`,
    /// `outside-every-root`.
    pub reason: &'static str,
    pub detail: String,
    /// The root-relative paths it concerns.
    pub affected: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Partial { gaps: Vec<Gap> },
    Unavailable { gaps: Vec<Gap> },
}

/// A changed file as the report lists it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ReportedChange {
    pub kind: &'static str,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
}

/// How many impacts of each kind, before truncation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ImpactCounts {
    pub render: usize,
    pub reference_integrity: usize,
    pub navigation: usize,
    pub publication: usize,
}

/// Results cut short: how many are shown of how many.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Truncated {
    pub shown: usize,
    pub total: usize,
}

/// A revision a comparison read: what was asked for and the commit it named.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RevisionRef {
    pub spec: String,
    pub commit: String,
}

/// What was compared, as the caller that built the snapshots says.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReportSource {
    /// Two commits' trees; the working tree is not read.
    GitRefs {
        base: RevisionRef,
        head: RevisionRef,
    },
    /// A commit against the working tree (and, in the app, its unsaved buffers).
    WorkingTree {
        base: RevisionRef,
        unsaved_buffers: bool,
    },
    /// Paths the caller named, against the working tree, with no base revision.
    Paths,
    /// The working tree against itself with the named paths taken out: what removing, moving or
    /// renaming them would affect. Nothing is removed.
    Removal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImpactReport {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ReportSource>,
    pub completeness: Completeness,
    pub changes: Vec<ReportedChange>,
    pub impacts: Vec<Impact>,
    pub counts: ImpactCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
}

impl ImpactReport {
    /// Whether the analysis covered everything it was given.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.completeness == Completeness::Complete
    }

    /// Records what the caller could not give the analysis, such as a revision's descriptor it
    /// could not read: a complete report becomes partial.
    pub fn add_gap(&mut self, gap: Gap) {
        match &mut self.completeness {
            Completeness::Complete => {
                self.completeness = Completeness::Partial { gaps: vec![gap] };
            }
            Completeness::Partial { gaps } | Completeness::Unavailable { gaps } => {
                gaps.push(gap);
                gaps.sort();
                gaps.dedup();
            }
        }
    }
}
