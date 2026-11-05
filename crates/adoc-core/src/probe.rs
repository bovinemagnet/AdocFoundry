//! Counters of costly operations, so tests can assert how often each runs: a keystroke that
//! parses the page twice, or hashes it at all, fails a test however fast the machine is.
//!
//! Without the `probe` feature, which only dev-dependencies turn on, [`hit`] does nothing and
//! [`count`] is always 0. Counts are per thread, so tests running side by side do not see each
//! other's; work done on a worker thread is counted on that thread.

/// An operation worth counting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Probe {
    /// `adoc_parser::parse` over a whole document.
    Parse,
    /// A hash of a whole document's text, to tell whether it is unsaved.
    HashText,
    /// The editor working out what changed between its buffer and the engine's.
    Diff,
    /// The editor replacing text in its buffer to match the engine's.
    Reconcile,
    /// A dictionary searching for replacements for a misspelt word.
    Suggest,
    /// A walk over every node of a visual model.
    Walk,
    /// A typeset formula's gap measured in a font.
    MeasureFormula,
    /// A list of a view block's children, `ViewBlock::child_refs`.
    ChildRefs,
    /// Inline content laid out as styled text for the preview.
    InlineLayout,
    /// A file's Quick Open item made from the index.
    QuickOpenItem,
    /// Every open document diagnosed again, `Engine::refresh_open_diagnostics`.
    DiagnoseOpen,
    /// A node of the link graph made for a snapshot.
    GraphNode,
    /// One file's part of the quality audit, `adoc_audit::audit_file`.
    AuditFile,
    /// One component version's part of the quality audit, `adoc_audit::audit_version`.
    AuditVersion,
    /// A catalog resource read while listing resources, `Engine::resources_after`.
    ListResource,
    /// A folder read by the index's walk of the roots.
    IndexWalk,
    /// A watch placed on a folder of the workspace.
    Watch,
    /// A document typeset for the PDF preview, `adoc_pdf::preview_pages`.
    PreviewTypeset,
    /// A theme's font file read from disk and parsed.
    FontRead,
    /// A PDF theme loaded from its files, `adoc_pdf::KeptTheme::load`.
    ThemeLoad,
}

const PROBES: usize = 20;

#[cfg(feature = "probe")]
thread_local! {
    static COUNTS: [std::cell::Cell<u64>; PROBES] = Default::default();
}

/// Counts one `probe`.
#[inline]
pub fn hit(probe: Probe) {
    #[cfg(feature = "probe")]
    COUNTS.with(|counts| {
        let count = &counts[probe as usize];
        count.set(count.get() + 1);
    });
    #[cfg(not(feature = "probe"))]
    let _ = probe;
}

/// How many times `probe` was counted on this thread.
#[must_use]
pub fn count(probe: Probe) -> u64 {
    #[cfg(feature = "probe")]
    return COUNTS.with(|counts| counts[probe as usize].get());
    #[cfg(not(feature = "probe"))]
    {
        let _ = probe;
        0
    }
}

/// Sets every count on this thread back to 0.
pub fn reset() {
    #[cfg(feature = "probe")]
    COUNTS.with(|counts| {
        for count in counts {
            count.set(0);
        }
    });
}

// Every probe has a counter.
const _: () = assert!(Probe::ThemeLoad as usize == PROBES - 1);
