//! Documentation impact analysis (Impact PRD): which pages, navigation trees, component
//! versions and publications a change to the workspace's files may affect, and why, with the
//! evidence path of every result.
//!
//! Pure and synchronous: no threads, no Git, no files written, no network. The caller gives the
//! changes ([`Change`]) and the dependency evidence of each side of the comparison as a
//! [`DependencySnapshot`]: the workspace's index and catalog at one revision, which the engine
//! builds from Git objects, or the live workspace. Every query goes through the index's reverse
//! maps and `adoc-lang`'s resolution (`includes_of`, `references_to`, `references_to_anchor`),
//! so nothing is parsed or resolved a second way, and nothing scans every file.
//!
//! The semantics (PRD §5): an **include** is rendered content, so a change reaches every page
//! that includes the file, transitively; an **xref** is a link, so a change to the target's
//! prose reaches nothing else, while a removed or moved target, or a removed anchor, makes its
//! referrers a **reference-integrity** review; a **navigation** entry is site structure, scoped
//! to its component version; a **publication** is affected when its selection holds an affected
//! page. A changed image or attachment is not followed yet: the report says so.

mod analyse;
mod report;
mod snapshot;

pub use analyse::{analyse, Limits, Publication, PublicationSelection};
pub use report::{
    ByteRange, Change, ChangeKind, Completeness, Confidence, EvidenceSide, Gap, Impact, ImpactKind,
    ImpactReport, ImpactStatus, ReportSource, RevisionRef, SourceSpan, SCHEMA_VERSION,
};
pub use snapshot::DependencySnapshot;
