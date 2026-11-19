//! Export, as PDF, DOCX or a Kindle book: one page as an article, or a component version (or
//! navigation files) in navigation order as a book. Every format shares one pipeline: the plan
//! of pages, each page parsed on a worker from its open buffer or its file, its view built and
//! its diagrams rendered, and the views arranged as the plan says ([`arrange`]). Only then does
//! the format matter: the document is typeset as PDF (the `typst` feature), written as DOCX
//! (the `docx` feature) or written as AZW3 or KFX (the `kindle` feature) on another worker.
//! The result arrives as [`EngineEvent::ExportFinished`].
//!
//! The pipeline is the `export` feature, which `typst`, `docx` and `kindle` each turn on; a
//! front end names those, never `export` itself.

use std::path::PathBuf;

use adoc_core::DiagnosticSeverity;

use crate::{DocumentId, PdfStandard};

#[cfg(all(
    feature = "export",
    not(any(
        feature = "typst",
        feature = "docx",
        feature = "kindle",
        feature = "epub",
        feature = "docbook"
    ))
))]
compile_error!(
    "the `export` feature is turned on by `typst`, `docx`, `kindle`, `epub` or `docbook`, not on \
     its own"
);

// Without either format only the request and result types remain.
#[cfg(feature = "typst")]
use std::collections::VecDeque;
#[cfg(feature = "export")]
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

#[cfg(feature = "export")]
use adoc_dom::{BlockKind, Dom};
#[cfg(feature = "typst")]
use adoc_pdf::{LoadedTheme, PdfOptions, PdfSeverity};
#[cfg(feature = "export")]
use adoc_render::{DiagramView, ViewInline, ViewKind, ViewTree};
#[cfg(feature = "export")]
use adoc_theme::ThemeSource;

#[cfg(feature = "export")]
use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent, NavItem,
};

/// What to export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportTarget {
    /// One page, as an article.
    Page(DocumentId),
    /// A component version, as a book in navigation order.
    Component {
        name: String,
        version: Option<String>,
    },
    /// A page by its path, open or not, as an article.
    File(PathBuf),
    /// Navigation files, as a book in the order given, titled after the first file's
    /// component version.
    Navigation(Vec<PathBuf>),
}

/// A PDF export request: `CommandArgs::Export`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportArgs {
    pub target: ExportTarget,
    pub output: PathBuf,
    /// A bundled theme, a theme name in the themes directory or a theme file; otherwise the
    /// page's `pdf-theme`, then `[pdf] theme`.
    pub theme: Option<String>,
    /// Standards the output must conform to, besides `[pdf] standards`.
    pub standards: Vec<PdfStandard>,
    /// Where the theme's fonts are, in place of the page's `pdf-fontsdir` and
    /// `[pdf] fonts_dir`; the bundled fonts are searched after it.
    pub fonts_dir: Option<PathBuf>,
    /// Also write the generated Typst here, for inspection.
    pub typst: Option<PathBuf>,
    /// Attributes every page is read with before its own header, after Antora's, and that a
    /// book's own attributes take.
    pub attributes: Vec<(String, String)>,
}

/// A DOCX export request: `CommandArgs::ExportDocx`. It holds nothing of PDF's: no theme,
/// standard, fonts folder or Typst.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocxExportArgs {
    pub target: ExportTarget,
    /// The `.docx` file to write. It is written to a new file beside it and renamed over it
    /// only once the whole document is written, so a failed export leaves what was there; a
    /// link there is refused, never written through.
    pub output: PathBuf,
    /// Attributes every page is read with before its own header, after Antora's, and that a
    /// book's own attributes take.
    pub attributes: Vec<(String, String)>,
    /// The paper and orientation, in place of `[docx]`'s: a profile's.
    pub paper: Option<crate::DocxPaper>,
    pub landscape: Option<bool>,
    /// The look, in place of `[docx] look`'s: the export's own or its profile's.
    pub look: Option<crate::LookChoice>,
}

/// The Kindle formats (`docs/prd/PRD-AdocFoundry_Kindle_Export.md` §4). One is never written
/// in place of the other.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum KindleFormat {
    /// KF8, in a PalmDB/MOBI file: the default, read by more Kindles.
    #[default]
    Azw3,
    /// KF10.
    Kfx,
}

impl KindleFormat {
    /// The file name extension the format is written with, and its name in settings.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Azw3 => "azw3",
            Self::Kfx => "kfx",
        }
    }

    /// The format a name or extension gives, in any letter case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "azw3" => Some(Self::Azw3),
            "kfx" => Some(Self::Kfx),
            _ => None,
        }
    }
}

/// A Kindle export request: `CommandArgs::ExportKindle`. It holds nothing of another format's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KindleExportArgs {
    pub target: ExportTarget,
    pub format: KindleFormat,
    /// The `.azw3` or `.kfx` file to write, its extension the format's. It is written to a
    /// new file beside it and renamed over it only once the whole book is written and checked,
    /// so a failed export leaves what was there; a link there is refused, never written
    /// through.
    pub output: PathBuf,
    /// Attributes every page is read with before its own header, after Antora's, and that a
    /// book's own attributes take.
    pub attributes: Vec<(String, String)>,
    /// Metadata in place of the document's and `[kindle]`'s: a profile's.
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    /// The cover, in place of `[kindle] cover` and the document's `front-cover-image`.
    pub cover: Option<PathBuf>,
}

impl KindleExportArgs {
    /// An export of `target` in `format` to `output`, with nothing else given.
    #[must_use]
    pub fn new(target: ExportTarget, format: KindleFormat, output: PathBuf) -> Self {
        Self {
            target,
            format,
            output,
            attributes: Vec::new(),
            title: None,
            creator: None,
            language: None,
            identifier: None,
            cover: None,
        }
    }
}

/// An EPUB export request: `CommandArgs::ExportEpub`. It holds nothing of another format's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubExportArgs {
    pub target: ExportTarget,
    /// The `.epub` file to write. It is written to a new file beside it and renamed over it
    /// only once the whole book is written and checked, so a failed export leaves what was
    /// there; a link there is refused, never written through.
    pub output: PathBuf,
    /// Attributes every page is read with before its own header, after Antora's, and that a
    /// book's own attributes take.
    pub attributes: Vec<(String, String)>,
    /// Metadata in place of the document's and `[epub]`'s: a profile's.
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The cover, in place of `[epub] cover` and the document's `front-cover-image`.
    pub cover: Option<PathBuf>,
    /// A stylesheet of the user's, in place of `[epub] css`, linked after the default one. It
    /// must be inside the workspace; one that imports or names anything fails the export.
    pub css: Option<PathBuf>,
    /// When the book was last modified, in seconds since the Unix epoch: its
    /// `dcterms:modified` and the date of every file in it. `None` takes `[epub] modified`,
    /// else `SOURCE_DATE_EPOCH`, else the time of the export, which is reported, since
    /// exporting again then gives other bytes.
    pub modified: Option<i64>,
}

impl EpubExportArgs {
    /// An export of `target` to `output`, with nothing else given.
    #[must_use]
    pub fn new(target: ExportTarget, output: PathBuf) -> Self {
        Self {
            target,
            output,
            attributes: Vec::new(),
            title: None,
            creator: None,
            language: None,
            identifier: None,
            publisher: None,
            cover: None,
            css: None,
            modified: None,
        }
    }
}

/// A DocBook export request: `CommandArgs::ExportDocBook`. It holds nothing of another
/// format's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocBookExportArgs {
    pub target: ExportTarget,
    /// The `.xml` file to write. Its pictures go in the folder beside it named after it
    /// (`guide.xml`, `guide_assets/`). Both are staged beside it and put in place only once the
    /// whole document is written and checked, so a failed export leaves what was there; a link
    /// at either is refused, never written through, and a folder there holding files the
    /// export did not write is never replaced.
    pub output: PathBuf,
    /// Attributes every page is read with before its own header, after Antora's, and that a
    /// book's own attributes take.
    pub attributes: Vec<(String, String)>,
}

impl DocBookExportArgs {
    /// An export of `target` to `output`, with nothing else given.
    #[must_use]
    pub fn new(target: ExportTarget, output: PathBuf) -> Self {
        Self {
            target,
            output,
            attributes: Vec::new(),
        }
    }
}

/// A finished export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSummary {
    pub output: PathBuf,
    /// The pages of the output: a PDF's own pages; for DOCX, which Word lays out when it opens
    /// the document, the AsciiDoc pages it holds.
    pub pages: usize,
    /// The AsciiDoc pages exported, in order (empty for an HTML export).
    pub sources: Vec<PathBuf>,
    pub diagnostics: Vec<ExportDiagnostic>,
    /// The look a Word export used; `None` for PDF and HTML.
    pub look: Option<crate::ExportLook>,
    /// The output carries an editing restriction its pages asked for (`:export-read-only:`).
    pub protected: bool,
}

/// A problem found while exporting, at a line of a page when known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// The file and its one-based line.
    pub location: Option<(PathBuf, u32)>,
}

impl std::fmt::Display for ExportDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some((path, line)) = &self.location {
            write!(formatter, "{}:{line}: ", path.display())?;
        }
        formatter.write_str(&self.message)
    }
}

#[cfg(not(feature = "typst"))]
impl crate::Engine {
    /// Without Typst there is no PDF export: every request fails as an unknown command.
    pub fn export_blocking(
        &mut self,
        _: ExportArgs,
        _: std::time::Duration,
    ) -> Result<ExportSummary, crate::EngineError> {
        Err(crate::EngineError::UnknownCommand(
            crate::commands::ids::EXPORT_PDF_PAGE,
        ))
    }
}

#[cfg(not(feature = "docx"))]
impl crate::Engine {
    /// Without the `docx` feature there is no DOCX export: every request fails as an unknown
    /// command.
    pub fn export_docx_blocking(
        &mut self,
        _: DocxExportArgs,
        _: std::time::Duration,
    ) -> Result<ExportSummary, crate::EngineError> {
        Err(crate::EngineError::UnknownCommand(
            crate::commands::ids::EXPORT_DOCX_PAGE,
        ))
    }
}

#[cfg(not(feature = "kindle"))]
impl crate::Engine {
    /// Without the `kindle` feature there is no Kindle export: every request fails as an
    /// unknown command.
    pub fn export_kindle_blocking(
        &mut self,
        _: KindleExportArgs,
        _: std::time::Duration,
    ) -> Result<ExportSummary, crate::EngineError> {
        Err(crate::EngineError::UnknownCommand(
            crate::commands::ids::EXPORT_KINDLE_PAGE,
        ))
    }
}

#[cfg(not(feature = "docbook"))]
impl crate::Engine {
    /// Without the `docbook` feature there is no DocBook export: every request fails as an
    /// unknown command.
    pub fn export_docbook_blocking(
        &mut self,
        _: DocBookExportArgs,
        _: std::time::Duration,
    ) -> Result<ExportSummary, crate::EngineError> {
        Err(crate::EngineError::UnknownCommand(
            crate::commands::ids::EXPORT_DOCBOOK_PAGE,
        ))
    }
}

#[cfg(not(feature = "epub"))]
impl crate::Engine {
    /// Without the `epub` feature there is no EPUB export: every request fails as an unknown
    /// command.
    pub fn export_epub_blocking(
        &mut self,
        _: EpubExportArgs,
        _: std::time::Duration,
    ) -> Result<ExportSummary, crate::EngineError> {
        Err(crate::EngineError::UnknownCommand(
            crate::commands::ids::EXPORT_EPUB_PAGE,
        ))
    }
}

/// The pages of an export and how they are arranged.
#[cfg(feature = "export")]
#[derive(Clone, Debug)]
enum Plan {
    Article(PathBuf),
    Book {
        title: String,
        attributes: BTreeMap<String, String>,
        items: Vec<PlanItem>,
    },
}

#[cfg(feature = "export")]
#[derive(Clone, Debug)]
enum PlanItem {
    Part(String),
    Heading {
        title: String,
        depth: u8,
    },
    /// A page, with its nav entry's text for when the page has no title.
    Page {
        path: PathBuf,
        depth: u8,
        nav_title: Option<String>,
        /// The start page, leading the book because the navigation does not list it: a
        /// DocBook book's preface. Other formats read it as any page.
        preface: bool,
    },
}

#[cfg(feature = "export")]
impl Plan {
    fn paths(&self) -> Vec<PathBuf> {
        match self {
            Self::Article(path) => vec![path.clone()],
            Self::Book { items, .. } => items
                .iter()
                .filter_map(|item| match item {
                    PlanItem::Page { path, .. } => Some(path.clone()),
                    _ => None,
                })
                .collect(),
        }
    }
}

/// What a job is to write, and how.
#[cfg(feature = "export")]
#[derive(Clone, Debug)]
enum Request {
    #[cfg(feature = "typst")]
    Pdf(ExportArgs),
    #[cfg(feature = "docx")]
    Docx(DocxExportArgs),
    #[cfg(feature = "kindle")]
    Kindle(KindleExportArgs),
    #[cfg(feature = "epub")]
    Epub(EpubExportArgs),
    #[cfg(feature = "docbook")]
    DocBook(DocBookExportArgs),
}

#[cfg(feature = "export")]
impl Request {
    /// Whether the views exported need formulas rendered as images: a Word document's do.
    fn draws_formulas(&self) -> bool {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(_) => false,
            #[cfg(feature = "docx")]
            Self::Docx(_) => true,
            // A Kindle book shows formulas as pictures too.
            #[cfg(feature = "kindle")]
            Self::Kindle(_) => true,
            // So does an EPUB, until it can carry MathML.
            #[cfg(feature = "epub")]
            Self::Epub(_) => true,
            // DocBook writes a formula's source, and its picture where one was drawn.
            #[cfg(feature = "docbook")]
            Self::DocBook(_) => true,
        }
    }

    /// Whether the views exported may start a program to draw a diagram (Graphviz, PlantUML,
    /// D2): every format but DocBook, which runs wholly in process (its PRD §5.4) and takes
    /// such a diagram only when the cache already has it.
    fn starts_programs(&self) -> bool {
        match self {
            #[cfg(feature = "docbook")]
            Self::DocBook(_) => false,
            #[allow(unreachable_patterns)]
            _ => true,
        }
    }
}

#[cfg(feature = "export")]
impl Request {
    fn target(&self) -> &ExportTarget {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.target,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.target,
            #[cfg(feature = "kindle")]
            Self::Kindle(args) => &args.target,
            #[cfg(feature = "epub")]
            Self::Epub(args) => &args.target,
            #[cfg(feature = "docbook")]
            Self::DocBook(args) => &args.target,
        }
    }

    fn output(&self) -> &Path {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.output,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.output,
            #[cfg(feature = "kindle")]
            Self::Kindle(args) => &args.output,
            #[cfg(feature = "epub")]
            Self::Epub(args) => &args.output,
            #[cfg(feature = "docbook")]
            Self::DocBook(args) => &args.output,
        }
    }

    /// The format the request writes, with the PDF/A standard it claims (from `[pdf]
    /// standards` and its own), if any.
    #[allow(unused_variables)]
    fn format(&self, settings: &crate::Settings) -> (crate::ExportFormat, Option<String>) {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => {
                let standards: Vec<crate::PdfStandard> = settings
                    .pdf
                    .standards
                    .iter()
                    .chain(&args.standards)
                    .copied()
                    .collect();
                match crate::protection::pdfa(&standards) {
                    Some(standard) => (crate::ExportFormat::PdfA, Some(standard)),
                    None => (crate::ExportFormat::Pdf, None),
                }
            }
            #[cfg(feature = "docx")]
            Self::Docx(_) => (crate::ExportFormat::Docx, None),
            #[cfg(feature = "kindle")]
            Self::Kindle(_) => (crate::ExportFormat::Kindle, None),
            #[cfg(feature = "epub")]
            Self::Epub(_) => (crate::ExportFormat::Epub, None),
            #[cfg(feature = "docbook")]
            Self::DocBook(_) => (crate::ExportFormat::DocBook, None),
        }
    }

    /// Refuses an output this format never writes.
    fn check_output(&self) -> Result<(), EngineError> {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(_) => Ok(()),
            #[cfg(feature = "docx")]
            Self::Docx(args) => check_docx_output(&args.output),
            #[cfg(feature = "kindle")]
            Self::Kindle(args) => check_kindle_output(args.format, &args.output),
            #[cfg(feature = "epub")]
            Self::Epub(args) => check_epub_output(&args.output),
            #[cfg(feature = "docbook")]
            Self::DocBook(args) => check_docbook_output(&args.output),
        }
    }

    fn attributes(&self) -> &[(String, String)] {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.attributes,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.attributes,
            #[cfg(feature = "kindle")]
            Self::Kindle(args) => &args.attributes,
            #[cfg(feature = "epub")]
            Self::Epub(args) => &args.attributes,
            #[cfg(feature = "docbook")]
            Self::DocBook(args) => &args.attributes,
        }
    }
}

#[cfg(feature = "export")]
enum Stage {
    Parsing,
    /// Diagrams still rendering, by checksum.
    Diagrams(BTreeSet<String>),
    /// Typesetting or writing the document.
    Writing,
}

/// Whether a job's document may still replace its output. A front end that waited for the job
/// may give up on it ([`Engine::export_blocking`]'s timeout); from then on the writer must leave
/// the output alone, and a job that already replaced it has finished, not failed. One lock, so
/// that giving up and replacing never pass each other.
#[cfg(feature = "export")]
#[derive(Clone, Default)]
struct Commit(std::sync::Arc<std::sync::Mutex<CommitState>>);

#[cfg(feature = "export")]
#[derive(Default)]
enum CommitState {
    #[default]
    Open,
    Abandoned,
    Replaced,
}

#[cfg(feature = "export")]
impl Commit {
    fn state(&self) -> std::sync::MutexGuard<'_, CommitState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Runs `replace`, which puts the finished document at the output, unless the job was
    /// abandoned.
    #[cfg_attr(not(any(feature = "typst", feature = "docx")), allow(dead_code))]
    fn replace(&self, replace: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut state = self.state();
        if matches!(*state, CommitState::Abandoned) {
            return Err("the export was abandoned before it was written".to_owned());
        }
        replace()?;
        *state = CommitState::Replaced;
        Ok(())
    }

    /// Gives up on the job unless the output was already replaced, and says whether it did.
    fn abandon(&self) -> bool {
        let mut state = self.state();
        if matches!(*state, CommitState::Replaced) {
            return false;
        }
        *state = CommitState::Abandoned;
        true
    }
}

#[cfg(feature = "export")]
struct Job {
    request: Request,
    plan: Plan,
    stage: Stage,
    commit: Commit,
    doms: Vec<(PathBuf, Dom)>,
    /// Kept for `export_blocking`, which takes it; otherwise only published.
    keep_result: bool,
    /// One of `export.pdf_pages`'s pages, reported in its summary rather than on its own.
    #[cfg_attr(not(feature = "typst"), allow(dead_code))]
    in_batch: bool,
    /// The editing restriction the output is to carry, decided before the job started.
    protection: crate::ExportProtection,
}

/// `export.pdf_pages` under way: its pages are exported one after another, so a folder of
/// pages never typesets them all at once.
#[cfg(feature = "typst")]
struct Batch {
    folder: PathBuf,
    /// Each page still to export, with the PDF it is written to.
    queue: VecDeque<(PathBuf, PathBuf)>,
    written: Vec<PathBuf>,
    failed: Vec<(PathBuf, String)>,
}

#[cfg(feature = "export")]
#[derive(Default)]
pub(crate) struct ExportState {
    jobs: BTreeMap<u64, Job>,
    finished: BTreeMap<u64, Result<ExportSummary, String>>,
    next: u64,
    #[cfg(feature = "typst")]
    batch: Option<Batch>,
}

#[cfg(feature = "export")]
pub(crate) fn register_export(registry: &mut CommandRegistry) {
    #[cfg(feature = "typst")]
    register_pdf(registry);
    #[cfg(feature = "kindle")]
    {
        register(
            registry,
            spec(
                ids::EXPORT_KINDLE_PAGE,
                "Export Page to Kindle…",
                CommandCategory::Document,
                None,
                CommandContext::ActiveDocument,
            ),
            kindle_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_KINDLE_COMPONENT,
                "Export Component to Kindle…",
                CommandCategory::Workspace,
                None,
                CommandContext::Antora,
            ),
            kindle_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_KINDLE_PROFILE,
                "Export Kindle Profile…",
                CommandCategory::Workspace,
                None,
                CommandContext::Always,
            ),
            kindle_profile_command,
        );
    }
    #[cfg(feature = "epub")]
    {
        register(
            registry,
            spec(
                ids::EXPORT_EPUB_PAGE,
                "Export Page to EPUB…",
                CommandCategory::Document,
                None,
                CommandContext::ActiveDocument,
            ),
            epub_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_EPUB_COMPONENT,
                "Export Component to EPUB…",
                CommandCategory::Workspace,
                None,
                CommandContext::Antora,
            ),
            epub_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_EPUB_PROFILE,
                "Export EPUB Profile…",
                CommandCategory::Workspace,
                None,
                CommandContext::Always,
            ),
            epub_profile_command,
        );
    }
    #[cfg(feature = "docbook")]
    {
        register(
            registry,
            spec(
                ids::EXPORT_DOCBOOK_PAGE,
                "Export Page as DocBook…",
                CommandCategory::Document,
                None,
                CommandContext::ActiveDocument,
            ),
            docbook_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_DOCBOOK_COMPONENT,
                "Export Component as DocBook…",
                CommandCategory::Workspace,
                None,
                CommandContext::Antora,
            ),
            docbook_command,
        );
    }
    #[cfg(feature = "docx")]
    {
        register(
            registry,
            spec(
                ids::EXPORT_DOCX_PAGE,
                "Export Page to Word…",
                CommandCategory::Document,
                None,
                CommandContext::ActiveDocument,
            ),
            docx_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_DOCX_COMPONENT,
                "Export Component to Word…",
                CommandCategory::Workspace,
                None,
                CommandContext::Antora,
            ),
            docx_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_DOCX_PROFILE,
                "Export DOCX Profile…",
                CommandCategory::Workspace,
                None,
                CommandContext::Always,
            ),
            docx_profile_command,
        );
        // The same exports with a look chosen from `Engine::docx_looks`, which front ends offer
        // first.
        register(
            registry,
            spec(
                ids::EXPORT_DOCX_PAGE_THEMED,
                "Export Page to Word with Theme…",
                CommandCategory::Document,
                None,
                CommandContext::ActiveDocument,
            ),
            docx_themed_command,
        );
        register(
            registry,
            spec(
                ids::EXPORT_DOCX_COMPONENT_THEMED,
                "Export Component to Word with Theme…",
                CommandCategory::Workspace,
                None,
                CommandContext::Antora,
            ),
            docx_themed_command,
        );
    }
}

#[cfg(feature = "kindle")]
fn kindle_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::ExportKindle(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_KINDLE_PAGE,
            expected: "an export target, a Kindle format and an .azw3 or .kfx output path",
        });
    };
    engine.start_export(Request::Kindle(args), false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "docbook")]
fn docbook_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::ExportDocBook(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_DOCBOOK_PAGE,
            expected: "an export target and an .xml output path",
        });
    };
    engine.start_export(Request::DocBook(args), false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "epub")]
fn epub_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::ExportEpub(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_EPUB_PAGE,
            expected: "an export target and an .epub output path",
        });
    };
    engine.start_export(Request::Epub(args), false)?;
    Ok(CommandOutcome::Done)
}

/// Exports an EPUB publication profile to the output it names.
#[cfg(feature = "epub")]
fn epub_profile_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Profile(name) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_EPUB_PROFILE,
            expected: "a profile name",
        });
    };
    let args = engine.epub_profile_export_args(&name)?;
    engine.start_export(Request::Epub(args), false)?;
    Ok(CommandOutcome::Done)
}

/// Exports a Kindle publication profile to the output it names.
#[cfg(feature = "kindle")]
fn kindle_profile_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Profile(name) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_KINDLE_PROFILE,
            expected: "a profile name",
        });
    };
    let args = engine.kindle_profile_export_args(&name)?;
    engine.start_export(Request::Kindle(args), false)?;
    Ok(CommandOutcome::Done)
}

/// A Word export with the look it names: its arguments must choose one.
#[cfg(feature = "docx")]
fn docx_themed_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    match args {
        CommandArgs::ExportDocx(args) if args.look.is_some() => {
            engine.start_export(Request::Docx(args), false)?;
            Ok(CommandOutcome::Done)
        }
        _ => Err(EngineError::InvalidArguments {
            command: ids::EXPORT_DOCX_PAGE_THEMED,
            expected: "an export target, a .docx output path and a look",
        }),
    }
}

/// Exports a Word publication profile to the output it names.
#[cfg(feature = "docx")]
fn docx_profile_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Profile(name) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_DOCX_PROFILE,
            expected: "a profile name",
        });
    };
    let args = engine.docx_profile_export_args(&name)?;
    engine.start_export(Request::Docx(args), false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "docx")]
fn docx_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::ExportDocx(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_DOCX_PAGE,
            expected: "an export target and a .docx output path",
        });
    };
    engine.start_export(Request::Docx(args), false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "typst")]
fn register_pdf(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::EXPORT_PDF_PAGE,
            "Export Page to PDF…",
            CommandCategory::Document,
            None,
            CommandContext::ActiveDocument,
        ),
        export_command,
    );
    register(
        registry,
        spec(
            ids::EXPORT_PDF_COMPONENT,
            "Export Component to PDF…",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        export_command,
    );
    register(
        registry,
        spec(
            ids::VIEW_TOGGLE_PDF_PREVIEW,
            "Toggle PDF Preview",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        toggle_pdf_preview,
    );
    register(
        registry,
        spec(
            ids::VIEW_TOGGLE_PDF_WINDOW,
            "Toggle PDF Preview Window",
            CommandCategory::View,
            None,
            CommandContext::Always,
        ),
        toggle_pdf_window,
    );
    register(
        registry,
        spec(
            ids::EXPORT_PDF_PAGES,
            "Export Pages as PDF…",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        export_pages_command,
    );
    register(
        registry,
        spec(
            ids::EXPORT_PDF_PROFILE,
            "Export PDF Profile…",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        profile_command,
    );
}

/// Switches the preview between the rendered page and its PDF pages, showing the preview when
/// it switches to the PDF; switched back, a preview it opened closes again (#420).
#[cfg(feature = "typst")]
fn toggle_pdf_preview(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    if layout.pdf && layout.visible {
        layout.hide_pdf();
    } else {
        layout.show_pdf(None);
    }
    set_pdf_layout(engine, layout);
    Ok(CommandOutcome::Done)
}

/// Shows the PDF in its own window, or, when it is shown there, turns it off, putting the
/// preview back where it was before (#420).
#[cfg(feature = "typst")]
fn toggle_pdf_window(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let mut layout = engine.preview_layout();
    if layout.pdf && layout.visible && layout.detached {
        layout.hide_pdf();
    } else {
        layout.show_pdf(Some(true));
    }
    set_pdf_layout(engine, layout);
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "typst")]
fn set_pdf_layout(engine: &mut Engine, layout: crate::PreviewLayout) {
    let pdf = layout.pdf;
    engine.set_preview_layout(layout);
    if let Some(target) = engine.preview_target().filter(|_| pdf) {
        engine.request_pdf_preview(target);
    }
}

/// Exports a publication profile to the output it names.
#[cfg(feature = "typst")]
fn profile_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Profile(name) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_PDF_PROFILE,
            expected: "a profile name",
        });
    };
    let args = engine.profile_export_args(&name)?;
    engine.start_export(Request::Pdf(args), false)?;
    Ok(CommandOutcome::Done)
}

/// Exports each page to its own PDF in a folder, one page after another.
#[cfg(feature = "typst")]
fn export_pages_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::ExportPages { pages, folder } = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_PDF_PAGES,
            expected: "the pages to export and the folder to write them in",
        });
    };
    engine.export_pages(pages, folder)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "typst")]
fn export_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Export(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_PDF_PAGE,
            expected: "an export target and output path",
        });
    };
    engine.start_export(Request::Pdf(args), false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "typst")]
impl Engine {
    /// Exports a PDF and waits for the result, for front ends without an event loop such as
    /// the CLI.
    pub fn export_blocking(
        &mut self,
        args: ExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(Request::Pdf(args), true)?;
        self.wait_for_export(job, deadline)
    }
}

#[cfg(feature = "docx")]
impl Engine {
    /// Exports a DOCX document and waits for the result, for front ends without an event loop
    /// such as the CLI.
    pub fn export_docx_blocking(
        &mut self,
        args: DocxExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(Request::Docx(args), true)?;
        self.wait_for_export(job, deadline)
    }
}

#[cfg(feature = "kindle")]
impl Engine {
    /// Exports a Kindle book and waits for the result, for front ends without an event loop
    /// such as the CLI.
    pub fn export_kindle_blocking(
        &mut self,
        args: KindleExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(Request::Kindle(args), true)?;
        self.wait_for_export(job, deadline)
    }
}

#[cfg(feature = "epub")]
impl Engine {
    /// Exports an EPUB and waits for the result, for front ends without an event loop such as
    /// the CLI.
    pub fn export_epub_blocking(
        &mut self,
        args: EpubExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(Request::Epub(args), true)?;
        self.wait_for_export(job, deadline)
    }
}

#[cfg(feature = "docbook")]
impl Engine {
    /// Exports DocBook and waits for the result, for front ends without an event loop such as
    /// the CLI.
    pub fn export_docbook_blocking(
        &mut self,
        args: DocBookExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(Request::DocBook(args), true)?;
        self.wait_for_export(job, deadline)
    }
}

/// The folder a DocBook document's pictures are written to: beside it, named after it
/// (`guide.xml`, `guide_assets`).
#[cfg(feature = "docbook")]
#[must_use]
pub fn docbook_assets_folder(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    output.with_file_name(format!("{stem}_assets"))
}

/// Refuses a DocBook output that is not an `.xml` file, or is a folder or a link, and one
/// whose assets folder is a link, is not a folder, or holds anything the export did not write:
/// that folder is replaced whole, so it must be the export's own.
#[cfg(feature = "docbook")]
fn check_docbook_output(output: &Path) -> Result<(), EngineError> {
    let xml = output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"));
    if !xml {
        return Err(EngineError::InvalidRequest(format!(
            "{} is not an .xml file",
            output.display()
        )));
    }
    check_not_link_or_folder(output)?;
    let assets = docbook_assets_folder(output);
    let refused = |why: &str| {
        Err(EngineError::InvalidRequest(format!(
            "{} {why}, so it is not replaced by the export's pictures",
            assets.display()
        )))
    };
    match std::fs::symlink_metadata(&assets) {
        Err(_) => Ok(()),
        Ok(meta) if meta.file_type().is_symlink() => refused("is a link"),
        Ok(meta) if !meta.is_dir() => refused("is not a folder"),
        Ok(_) => {
            let entries = std::fs::read_dir(&assets).map_err(|error| {
                EngineError::InvalidRequest(format!("{}: {error}", assets.display()))
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    EngineError::InvalidRequest(format!("{}: {error}", assets.display()))
                })?;
                let ours = entry.file_type().is_ok_and(|kind| kind.is_file())
                    && entry.file_name().to_str().is_some_and(is_asset_name);
                if !ours {
                    return refused(&format!(
                        "holds {}, which the export did not write",
                        entry.file_name().to_string_lossy()
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Whether `name` is one a DocBook export gives a picture: `i<n>.<extension>`.
#[cfg(feature = "docbook")]
fn is_asset_name(name: &str) -> bool {
    let Some((stem, extension)) = name.split_once('.') else {
        return false;
    };
    let number = stem.strip_prefix('i').unwrap_or_default();
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(extension, "png" | "jpg" | "gif" | "svg")
}

/// Refuses an EPUB output that is not an `.epub` file, or is a folder or a link.
#[cfg(feature = "epub")]
fn check_epub_output(output: &Path) -> Result<(), EngineError> {
    let epub = output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"));
    if !epub {
        return Err(EngineError::InvalidRequest(format!(
            "{} is not an .epub file",
            output.display()
        )));
    }
    check_not_link_or_folder(output)
}

/// Refuses a Kindle output whose extension is not the format's, so that a `.kfx` is never an
/// AZW3 or the other way round, and one that is a folder or a link.
#[cfg(feature = "kindle")]
fn check_kindle_output(format: KindleFormat, output: &Path) -> Result<(), EngineError> {
    let matches = output
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(KindleFormat::from_name)
        == Some(format);
    if !matches {
        return Err(EngineError::InvalidRequest(format!(
            "{} is not a .{} file",
            output.display(),
            format.extension()
        )));
    }
    check_not_link_or_folder(output)
}

/// Refuses an output that is a link, which would be written through, or a folder.
#[cfg(any(
    feature = "docx",
    feature = "kindle",
    feature = "epub",
    feature = "docbook"
))]
fn check_not_link_or_folder(output: &Path) -> Result<(), EngineError> {
    match std::fs::symlink_metadata(output) {
        Ok(meta) if meta.file_type().is_symlink() => Err(EngineError::InvalidRequest(format!(
            "{} is a link, which is not written through",
            output.display()
        ))),
        Ok(meta) if meta.is_dir() => Err(EngineError::InvalidRequest(format!(
            "{} is a folder",
            output.display()
        ))),
        _ => Ok(()),
    }
}

/// Refuses a DOCX output that is not a `.docx` file name, is a folder, or is a link, which
/// would be written through.
#[cfg(feature = "docx")]
fn check_docx_output(output: &Path) -> Result<(), EngineError> {
    let docx = output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("docx"));
    if !docx {
        return Err(EngineError::InvalidRequest(format!(
            "{} is not a .docx file",
            output.display()
        )));
    }
    check_not_link_or_folder(output)
}

#[cfg(feature = "export")]
impl Engine {
    /// Whether an export is under way.
    #[must_use]
    pub fn is_exporting(&self) -> bool {
        !self.export.jobs.is_empty()
    }

    /// Waits for `job` until `deadline`, typesetting once nothing it waits for is still
    /// rendering.
    fn wait_for_export(
        &mut self,
        job: u64,
        deadline: Instant,
    ) -> Result<ExportSummary, EngineError> {
        loop {
            // Waits for this job only, never for other workers.
            let waited = self.wait_until(deadline, |engine| {
                engine.export.finished.contains_key(&job)
                    || !engine.export.jobs.contains_key(&job)
                    || engine.export_stalled(job)
            });
            if let Some(result) = self.export.finished.remove(&job) {
                return result.map_err(EngineError::Refused);
            }
            if !self.export.jobs.contains_key(&job) {
                return Err(EngineError::Refused("the export stopped".to_owned()));
            }
            if !waited {
                let abandoned = self
                    .export
                    .jobs
                    .get(&job)
                    .is_none_or(|state| state.commit.abandon());
                if abandoned {
                    self.export.jobs.remove(&job);
                    return Err(EngineError::Refused(
                        "the export did not finish in time".to_owned(),
                    ));
                }
                // The document is in place: the writer's answer is on its way.
                let soon = Instant::now() + Duration::from_secs(60);
                self.wait_until(soon, |engine| engine.export.finished.contains_key(&job));
                continue;
            }
            // Nothing left to wait for: every diagram has reported, so typeset now.
            self.export_ready(job);
        }
    }

    /// Whether `job` waits for diagrams none of which is still rendering.
    fn export_stalled(&self, job: u64) -> bool {
        self.export
            .jobs
            .get(&job)
            .is_some_and(|state| match &state.stage {
                Stage::Diagrams(waiting) => waiting
                    .iter()
                    .all(|checksum| !self.diagram_rendering(checksum)),
                _ => false,
            })
    }
}

#[cfg(feature = "typst")]
impl Engine {
    /// Exports each of `pages` to `<folder>/<name>.pdf`, one after another; pages named alike
    /// are told apart as `<name>-2.pdf` and so on. Reports once, with
    /// [`EngineEvent::PagesExported`], when the last has finished. Refuses no pages, a folder
    /// that is not one, and a second request while one is under way.
    pub fn export_pages(
        &mut self,
        pages: Vec<PathBuf>,
        folder: PathBuf,
    ) -> Result<(), EngineError> {
        if pages.is_empty() {
            return Err(EngineError::InvalidRequest(
                "there are no pages to export".to_owned(),
            ));
        }
        if !folder.is_dir() {
            return Err(EngineError::InvalidRequest(format!(
                "{} is not a folder",
                folder.display()
            )));
        }
        if self.export.batch.is_some() {
            return Err(EngineError::Refused(
                "pages are being exported already".to_owned(),
            ));
        }
        let mut taken = HashSet::new();
        let queue = pages
            .into_iter()
            .map(|page| {
                let stem = page
                    .file_stem()
                    .map_or_else(|| "page".into(), |stem| stem.to_string_lossy().into_owned());
                let name = (1..)
                    .map(|n| match n {
                        1 => format!("{stem}.pdf"),
                        n => format!("{stem}-{n}.pdf"),
                    })
                    .find(|name| taken.insert(name.clone()))
                    .unwrap_or_default();
                let output = folder.join(name);
                (page, output)
            })
            .collect();
        self.export.batch = Some(Batch {
            folder,
            queue,
            written: Vec::new(),
            failed: Vec::new(),
        });
        self.export_next_page();
        Ok(())
    }

    /// Starts the batch's next page, recording any that cannot start, and reports the batch
    /// once none is left.
    fn export_next_page(&mut self) {
        loop {
            let Some(batch) = self.export.batch.as_mut() else {
                return;
            };
            let Some((page, output)) = batch.queue.pop_front() else {
                let batch = self.export.batch.take().expect("a batch");
                self.emit(EngineEvent::PagesExported {
                    folder: batch.folder,
                    written: batch.written,
                    failed: batch.failed,
                });
                return;
            };
            // The names are the pages', not chosen in a save dialog that would ask, so a link
            // planted at one is refused rather than written through. A PDF there is replaced.
            if std::fs::symlink_metadata(&output).is_ok_and(|meta| meta.file_type().is_symlink()) {
                if let Some(batch) = self.export.batch.as_mut() {
                    batch.failed.push((
                        page,
                        format!(
                            "{} is a link, which is not written through",
                            output.display()
                        ),
                    ));
                }
                continue;
            }
            let args = ExportArgs {
                target: ExportTarget::File(page.clone()),
                output,
                theme: None,
                standards: Vec::new(),
                fonts_dir: None,
                typst: None,
                attributes: Vec::new(),
            };
            match self.start_export_job(Request::Pdf(args), false, true) {
                Ok(_) => return,
                Err(error) => {
                    if let Some(batch) = self.export.batch.as_mut() {
                        batch.failed.push((page, error.to_string()));
                    }
                }
            }
        }
    }
}

#[cfg(feature = "export")]
impl Engine {
    fn start_export(&mut self, request: Request, keep_result: bool) -> Result<u64, EngineError> {
        self.start_export_job(request, keep_result, false)
    }

    fn start_export_job(
        &mut self,
        request: Request,
        keep_result: bool,
        in_batch: bool,
    ) -> Result<u64, EngineError> {
        request.check_output()?;
        let mut plan = self.export_plan(request.target())?;
        if let Plan::Book {
            attributes,
            #[cfg(feature = "typst")]
            title,
            ..
        } = &mut plan
        {
            for (name, value) in request.attributes() {
                attributes.insert(name.clone(), value.clone());
            }
            // A PDF's own title; DOCX reads no `pdf-` attribute.
            #[cfg(feature = "typst")]
            if let (Request::Pdf(_), Some(pdf_title)) = (&request, attributes.get("pdf-title")) {
                title.clone_from(pdf_title);
            }
        }
        let inputs: Vec<_> = plan
            .paths()
            .iter()
            .map(|path| {
                let mut input = self.parse_input_for_path(path).ok_or_else(|| {
                    EngineError::NotFound(format!("{} cannot be read", path.display()))
                })?;
                input
                    .attributes
                    .extend(request.attributes().iter().cloned());
                Ok(input)
            })
            .collect::<Result<_, EngineError>>()?;
        // Read-only exports: the pages' own headers, read from the very text the worker will
        // parse, decide once, before anything is parsed or written.
        let protection = self.protection_of(
            inputs.iter().map(|input: &crate::rendered::OwnedInput| {
                (input.path.clone(), input.text.clone())
            }),
            request.format(&self.settings),
        )?;
        self.export.next += 1;
        let job = self.export.next;
        self.export.jobs.insert(
            job,
            Job {
                request,
                plan,
                stage: Stage::Parsing,
                commit: Commit::default(),
                doms: Vec::new(),
                keep_result,
                in_batch,
                protection,
            },
        );
        let parser = self.document_parser();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("reading the pages crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let doms = inputs
                .into_iter()
                .map(|input| {
                    let path = input.path.clone();
                    (path, input.parse(parser.as_ref()).0)
                })
                .collect();
            let _ = sender.send(WorkerMessage::ExportParsed { job, doms });
        });
        Ok(job)
    }

    /// What an export of `target` in `format` would carry, or why it would be refused: the
    /// decision the export makes, from the pages it would select as they stand now (open
    /// buffers included), for front ends to show before exporting.
    pub fn export_protection(
        &self,
        target: &ExportTarget,
        format: crate::ExportFormat,
    ) -> Result<crate::ExportProtection, EngineError> {
        let plan = self.export_plan(target)?;
        let texts = plan
            .paths()
            .iter()
            .map(|path| {
                self.parse_input_for_path(path)
                    .map(|input| (input.path, input.text))
                    .ok_or_else(|| {
                        EngineError::NotFound(format!("{} cannot be read", path.display()))
                    })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        // A PDF claims the standards `[pdf]` names, as an export of it would.
        #[cfg(feature = "typst")]
        let standard = (format == crate::ExportFormat::Pdf)
            .then(|| crate::protection::pdfa(&self.settings.pdf.standards))
            .flatten();
        #[cfg(not(feature = "typst"))]
        let standard = None;
        self.protection_of(texts.into_iter(), (format, standard))
    }

    /// The decision for pages with these texts, written in `format` (with the PDF/A standard
    /// it claims, if any).
    fn protection_of(
        &self,
        pages: impl Iterator<Item = (PathBuf, std::sync::Arc<str>)>,
        (format, standard): (crate::ExportFormat, Option<String>),
    ) -> Result<crate::ExportProtection, EngineError> {
        let request = crate::protection::request(pages.map(|(path, text)| {
            let page = crate::read_only::page_read_only(&path, &text);
            (path, page)
        }))
        .map_err(|refusal| EngineError::ExportProtection(Box::new(refusal)))?;
        let format = match (format, &standard) {
            (crate::ExportFormat::Pdf, Some(_)) => crate::ExportFormat::PdfA,
            _ => format,
        };
        crate::protection::resolve(request, format, standard)
            .map_err(|refusal| EngineError::ExportProtection(Box::new(refusal)))
    }

    /// The pages to export and how they are arranged.
    fn export_plan(&self, target: &ExportTarget) -> Result<Plan, EngineError> {
        match target {
            ExportTarget::Page(id) => {
                let document = self
                    .document(*id)
                    .ok_or(EngineError::UnknownDocument(*id))?;
                Ok(Plan::Article(document.path.clone()))
            }
            ExportTarget::Component { name, version } => {
                self.book_plan(name, version.as_deref(), None)
            }
            ExportTarget::File(path) => Ok(Plan::Article(path.clone())),
            ExportTarget::Navigation(files) => {
                let context = files
                    .first()
                    .and_then(|file| self.workspace.antora.context_for_path(file))
                    .ok_or_else(|| {
                        EngineError::NotFound(
                            "the navigation files are not in an Antora component".to_owned(),
                        )
                    })?;
                self.book_plan(&context.component, context.version.as_deref(), Some(files))
            }
        }
    }

    /// A component version as a book: one part per nav file (or titled list) when there are
    /// several, chapters from top-level entries, sections from nested ones, each page once.
    /// `nav_files`, when given, replace the component's own navigation, in their order.
    fn book_plan(
        &self,
        name: &str,
        version: Option<&str>,
        nav_files: Option<&[PathBuf]>,
    ) -> Result<Plan, EngineError> {
        let antora = &self.workspace.antora;
        let component = antora.component(name, version).ok_or_else(|| {
            EngineError::NotFound(format!("no Antora component `{name}` in the workspace"))
        })?;
        let trees = match nav_files {
            Some(files) => self.nav_trees_of(files),
            None => self.nav_trees(name, version),
        }
        .map_err(|error| EngineError::NotFound(error.to_string()))?;
        // Groups of entries, each a candidate part: a titled list, or a whole nav file.
        let mut groups: Vec<(Option<&NavItem>, Vec<NavItem>)> = Vec::new();
        for tree in &trees {
            let titled = !tree.items.is_empty()
                && tree.items.iter().all(|item| {
                    (item.list_title || item.target.is_none()) && !item.children.is_empty()
                });
            if titled {
                for item in &tree.items {
                    groups.push((Some(item), item.children.clone()));
                }
            } else {
                groups.push((None, tree.items.clone()));
            }
        }
        let parts = groups.len() > 1 && groups.iter().any(|(title, _)| title.is_some());
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        // The start page leads as a preface when the navigation does not list it.
        let start = self.start_page(component.name.as_str(), component.version.as_deref());
        let listed = |path: &Path| {
            fn contains(engine: &Engine, items: &[NavItem], path: &Path) -> bool {
                items.iter().any(|item| {
                    engine.nav_page(item).as_deref() == Some(path)
                        || contains(engine, &item.children, path)
                })
            }
            groups.iter().any(|(title, entries)| {
                title.is_some_and(|title| self.nav_page(title).as_deref() == Some(path))
                    || contains(self, entries, path)
            })
        };
        if let Some(start) = start.filter(|start| !listed(start)) {
            seen.insert(start.clone());
            items.push(PlanItem::Page {
                path: start,
                depth: 1,
                nav_title: None,
                preface: true,
            });
        }
        for (title, entries) in &groups {
            if let Some(title) = title {
                if parts {
                    items.push(PlanItem::Part(title.title.clone()));
                }
                // A title that links to a page leads its list with that page.
                if let Some(path) = self
                    .nav_page(title)
                    .filter(|path| seen.insert(path.clone()))
                {
                    items.push(PlanItem::Page {
                        path,
                        depth: 1,
                        nav_title: Some(title.title.clone()),
                        preface: false,
                    });
                }
            }
            self.plan_entries(entries, 1, &mut items, &mut seen);
        }
        if items
            .iter()
            .all(|item| !matches!(item, PlanItem::Page { .. }))
        {
            return Err(EngineError::NotFound(format!(
                "component `{name}` lists no pages in its navigation"
            )));
        }
        let mut attributes: BTreeMap<String, String> = component
            .asciidoc_attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.trim_end_matches('@').to_owned()))
            .collect();
        if let Some(playbook) = antora.playbook() {
            for (name, value) in &playbook.asciidoc_attributes {
                attributes
                    .entry(name.clone())
                    .or_insert_with(|| value.trim_end_matches('@').to_owned());
            }
        }
        attributes.insert("doctype".to_owned(), "book".to_owned());
        attributes.entry("toc".to_owned()).or_default();
        if let Some(version) = component
            .display_version
            .clone()
            .or_else(|| component.version.clone())
        {
            attributes.entry("revnumber".to_owned()).or_insert(version);
        }
        // A PDF takes `pdf-title` in its place (`start_export_job`).
        let title = component
            .title
            .clone()
            .unwrap_or_else(|| component.name.clone());
        Ok(Plan::Book {
            title,
            attributes,
            items,
        })
    }

    fn plan_entries(
        &self,
        entries: &[NavItem],
        depth: u8,
        items: &mut Vec<PlanItem>,
        seen: &mut HashSet<PathBuf>,
    ) {
        for entry in entries {
            match self.nav_page(entry) {
                Some(path) if seen.insert(path.clone()) => {
                    items.push(PlanItem::Page {
                        path,
                        depth,
                        nav_title: Some(entry.title.clone()),
                        preface: false,
                    });
                }
                Some(_) => {}
                None => items.push(PlanItem::Heading {
                    title: entry.title.clone(),
                    depth,
                }),
            }
            self.plan_entries(&entry.children, depth.saturating_add(1), items, seen);
        }
    }

    /// The page a nav entry links to.
    fn nav_page(&self, item: &NavItem) -> Option<PathBuf> {
        let id = item
            .target
            .as_ref()
            .filter(|id| id.family == Some(crate::ResourceFamily::Page))?;
        let coordinate = crate::AntoraCoordinate {
            component: id.component.clone()?,
            version: id.version.clone(),
            module: id.module.clone()?,
            family: id.family?,
            relative_path: PathBuf::from(&id.path),
        };
        self.workspace
            .antora
            .resolve(&coordinate)
            .map(|resource| resource.source_path.clone())
    }

    /// The component version's start page: `start_page` in `antora.yml`, else `ROOT:index.adoc`.
    fn start_page(&self, name: &str, version: Option<&str>) -> Option<PathBuf> {
        let antora = &self.workspace.antora;
        let component = antora.component(name, version)?;
        let id = component.start_page.as_deref().unwrap_or("ROOT:index.adoc");
        let id = adoc_antora::parse_resource_id(id).ok()?;
        let coordinate = crate::AntoraCoordinate {
            component: name.to_owned(),
            version: component.version.clone(),
            module: id.module.unwrap_or_else(|| "ROOT".to_owned()),
            family: crate::ResourceFamily::Page,
            relative_path: PathBuf::from(&id.path),
        };
        antora
            .resolve(&coordinate)
            .map(|resource| resource.source_path.clone())
    }

    pub(crate) fn export_parsed(&mut self, job: u64, doms: Vec<(PathBuf, Dom)>) {
        let Some(state) = self.export.jobs.get_mut(&job) else {
            return;
        };
        state.doms = doms;
        self.trim_diagram_failures();
        let waiting = self.export_views(job).1;
        let Some(state) = self.export.jobs.get_mut(&job) else {
            return;
        };
        if waiting.is_empty() {
            self.export_ready(job);
        } else {
            state.stage = Stage::Diagrams(waiting);
        }
    }

    pub(crate) fn export_diagram_rendered(&mut self, checksum: &str) {
        let ready: Vec<u64> = self
            .export
            .jobs
            .iter_mut()
            .filter_map(|(job, state)| match &mut state.stage {
                Stage::Diagrams(waiting) => {
                    waiting.remove(checksum);
                    waiting.is_empty().then_some(*job)
                }
                _ => None,
            })
            .collect();
        for job in ready {
            self.export_ready(job);
        }
    }

    /// The job's views, and the diagrams they are still waiting for (which are started).
    fn export_views(&mut self, job: u64) -> (Vec<(PathBuf, ViewTree)>, BTreeSet<String>) {
        let doms: Vec<(PathBuf, Dom)> = self
            .export
            .jobs
            .get_mut(&job)
            .map(|state| std::mem::take(&mut state.doms))
            .unwrap_or_default();
        // A PDF typesets its own formulas; a Word document shows them as the images rendered
        // for the preview, waited for like diagrams.
        let (formulas, programs) = self.export.jobs.get(&job).map_or((false, true), |state| {
            (
                state.request.draws_formulas(),
                state.request.starts_programs(),
            )
        });
        let mut views = Vec::new();
        let mut waiting = BTreeSet::new();
        for (path, dom) in &doms {
            let (view, wanted) = self.view_of(path, dom, formulas, programs);
            waiting.extend(wanted);
            views.push((path.clone(), view));
        }
        if let Some(state) = self.export.jobs.get_mut(&job) {
            state.doms = doms;
        }
        (views, waiting)
    }

    /// Every diagram is in: arrange the views, then typeset or write the document on a worker.
    fn export_ready(&mut self, job: u64) {
        if !matches!(
            self.export.jobs.get(&job).map(|state| &state.stage),
            Some(Stage::Parsing | Stage::Diagrams(_))
        ) {
            return;
        }
        let (views, _) = self.export_views(job);
        let Some(state) = self.export.jobs.get_mut(&job) else {
            return;
        };
        state.stage = Stage::Writing;
        let unread = unread_pages(&state.doms);
        let request = state.request.clone();
        let commit = state.commit.clone();
        #[cfg(feature = "typst")]
        let replace = state.in_batch;
        let plan = state.plan.clone();
        let protection = state.protection.clone();
        let written = Written {
            sources: plan.paths(),
            arranged: arrange(&plan, views),
            unread,
        };
        match request {
            #[cfg(feature = "typst")]
            Request::Pdf(args) => self.write_pdf(job, args, written, replace, commit, protection),
            #[cfg(feature = "docx")]
            Request::Docx(args) => self.write_docx(job, args, written, commit, protection),
            #[cfg(feature = "kindle")]
            Request::Kindle(args) => {
                self.write_kindle(job, args, written, commit, &protection);
            }
            #[cfg(feature = "epub")]
            Request::Epub(args) => {
                self.write_epub(job, args, written, commit, &protection);
            }
            #[cfg(feature = "docbook")]
            Request::DocBook(args) => {
                self.write_docbook(job, args, written, commit, &protection);
            }
        }
    }

    pub(crate) fn export_finished(&mut self, job: u64, result: Result<ExportSummary, String>) {
        let Some(state) = self.export.jobs.remove(&job) else {
            return;
        };
        if state.keep_result {
            self.export.finished.insert(job, result.clone());
        }
        let output = state.request.output().to_path_buf();
        #[cfg(feature = "typst")]
        if state.in_batch {
            if let Some(batch) = self.export.batch.as_mut() {
                match result {
                    Ok(_) => batch.written.push(output),
                    Err(error) => {
                        let page = match state.request.target() {
                            ExportTarget::File(page) => page.clone(),
                            _ => output,
                        };
                        batch.failed.push((page, error));
                    }
                }
            }
            self.export_next_page();
            return;
        }
        self.emit(EngineEvent::ExportFinished { output, result });
    }
}

/// A job's document, arranged and ready to write in either format.
#[cfg(feature = "export")]
struct Written {
    /// The pages, in order.
    sources: Vec<PathBuf>,
    arranged: Arranged,
    /// Pages the parser could not read.
    unread: Vec<ExportDiagnostic>,
}

#[cfg(feature = "typst")]
impl Engine {
    /// Loads the theme and typesets the PDF on a worker.
    fn write_pdf(
        &mut self,
        job: u64,
        args: ExportArgs,
        written: Written,
        replace: bool,
        commit: Commit,
        protection: crate::ExportProtection,
    ) {
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_pdf(arranged);
        let first_page = sources.first().cloned();
        let attributes = match &document {
            adoc_pdf::Document::Article(page) => page.view.attributes.clone(),
            adoc_pdf::Document::Book(book) => book.attributes.clone(),
        };
        let theme = self.export_theme(&args, &attributes, first_page.as_deref());
        let mut options = self.export_options(&args, &attributes, first_page.as_deref());
        options.protection = if protection == crate::ExportProtection::Off {
            adoc_pdf::PdfProtection::None
        } else {
            adoc_pdf::PdfProtection::RestrictModification
        };
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("typesetting crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = theme
                .and_then(|theme| typeset(&document, &theme, &options, &args, replace, &commit))
                .map(|mut summary| {
                    summary.sources = sources;
                    summary.diagnostics.extend(failed);
                    summary.diagnostics.extend(unread);
                    summary
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }

    /// The theme: the export's own, else the first page's `pdf-theme`, else `[pdf] theme`;
    /// named themes are looked for in `pdf-themesdir` or `[pdf] themes_dir`.
    fn export_theme(
        &self,
        args: &ExportArgs,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> Result<LoadedTheme, String> {
        let source = self.theme_source(args.theme.as_deref(), attributes, page);
        adoc_pdf::load_theme_within(&source, &self.file_access).map_err(|error| error.to_string())
    }

    pub(crate) fn export_options(
        &self,
        args: &ExportArgs,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> PdfOptions {
        let fonts_dirs = self.fonts_dirs(args.fonts_dir.as_deref(), attributes, page);
        let mut standards = self.settings.pdf.standards.clone();
        for standard in &args.standards {
            if !standards.contains(standard) {
                standards.push(*standard);
            }
        }
        let page_urls = self
            .site_url()
            .map(|site| self.page_urls(&site))
            .unwrap_or_default();
        PdfOptions {
            standards,
            fonts_dirs,
            page_urls,
            date: None,
            access: self.file_access.clone(),
            protection: adoc_pdf::PdfProtection::None,
        }
    }
}

#[cfg(feature = "export")]
impl Engine {
    /// The directories the theme's fonts are looked for in: the export's own, else the page's
    /// `pdf-fontsdir`, else `[pdf] fonts_dir`, and the bundled fonts last.
    pub(crate) fn fonts_dirs(
        &self,
        fonts_dir: Option<&Path>,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> Vec<PathBuf> {
        let page_dir = page.and_then(Path::parent);
        let root = self.roots.first().map(PathBuf::as_path);
        let mut fonts_dirs: Vec<PathBuf> = match (fonts_dir, attributes.get("pdf-fontsdir")) {
            (Some(dir), _) => vec![dir.to_path_buf()],
            (None, Some(dirs)) => dirs
                .split([';', ','])
                .map(str::trim)
                .filter(|dir| !dir.is_empty())
                .map(|dir| {
                    if dir == "GEM_FONTS_DIR" {
                        PathBuf::from(dir)
                    } else {
                        resolve(dir, page_dir)
                    }
                })
                .collect(),
            (None, None) => self
                .settings
                .pdf
                .fonts_dir
                .as_deref()
                .map(|dir| vec![resolve(dir, root)])
                .unwrap_or_default(),
        };
        fonts_dirs.push(PathBuf::from("GEM_FONTS_DIR"));
        fonts_dirs
    }

    /// Where named themes, and Word templates beside them, are looked for: the page's
    /// `pdf-themesdir`, relative to it, else `[pdf] themes_dir`, relative to the first root.
    pub(crate) fn themes_dir(
        &self,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> Option<PathBuf> {
        let page_dir = page.and_then(Path::parent);
        let root = self.roots.first().map(PathBuf::as_path);
        attributes
            .get("pdf-themesdir")
            .map(|dir| resolve(dir, page_dir))
            .or_else(|| {
                self.settings
                    .pdf
                    .themes_dir
                    .as_deref()
                    .map(|dir| resolve(dir, root))
            })
    }

    /// Where a theme comes from: `theme`, else the page's `pdf-theme`, else `[pdf] theme`,
    /// looked for in [`Engine::themes_dir`]. PDF export and a Word export's PDF-theme look
    /// both ask this, so they never disagree about which theme applies.
    pub(crate) fn theme_source(
        &self,
        theme: Option<&str>,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> ThemeSource {
        let name = theme
            .map(str::to_owned)
            .or_else(|| attributes.get("pdf-theme").cloned())
            .unwrap_or_else(|| self.settings.pdf.theme.clone());
        ThemeSource::named(&name, self.themes_dir(attributes, page).as_deref())
    }
}

#[cfg(feature = "export")]
fn resolve(dir: &str, base: Option<&Path>) -> PathBuf {
    let path = Path::new(dir);
    match base {
        Some(base) if path.is_relative() => adoc_index::normalize_path(&base.join(path)),
        _ => path.to_path_buf(),
    }
}

/// The views arranged as the plan says, for either format: an article, or a book of parts,
/// headings and pages, a page without a title taking its navigation entry's.
#[cfg(feature = "export")]
enum Arranged {
    Article(ViewTree),
    Book {
        title: String,
        attributes: BTreeMap<String, String>,
        items: Vec<ArrangedItem>,
    },
}

#[cfg(feature = "export")]
enum ArrangedItem {
    Part(String),
    Heading {
        title: String,
        depth: u8,
    },
    Page {
        /// A short key unique in the book, for the page's labels.
        key: String,
        view: ViewTree,
        depth: u8,
        /// The start page as the book's preface ([`PlanItem::Page`]).
        preface: bool,
    },
}

#[cfg(feature = "export")]
fn arrange(plan: &Plan, views: Vec<(PathBuf, ViewTree)>) -> Arranged {
    let mut views: HashMap<PathBuf, ViewTree> = views.into_iter().collect();
    match plan {
        Plan::Article(path) => Arranged::Article(views.remove(path).unwrap_or_default()),
        Plan::Book {
            title,
            attributes,
            items,
        } => {
            let mut index = 0;
            let items = items
                .iter()
                .filter_map(|item| match item {
                    PlanItem::Part(title) => Some(ArrangedItem::Part(title.clone())),
                    PlanItem::Heading { title, depth } => Some(ArrangedItem::Heading {
                        title: title.clone(),
                        depth: *depth,
                    }),
                    PlanItem::Page {
                        path,
                        depth,
                        nav_title,
                        preface,
                    } => {
                        let mut view = views.remove(path)?;
                        if view.title.is_none() {
                            view.title = nav_title
                                .as_ref()
                                .map(|title| vec![ViewInline::Text(title.clone())]);
                        }
                        index += 1;
                        Some(ArrangedItem::Page {
                            key: format!("p{index}"),
                            view,
                            depth: *depth,
                            preface: *preface,
                        })
                    }
                })
                .collect();
            Arranged::Book {
                title: title.clone(),
                attributes: attributes.clone(),
                items,
            }
        }
    }
}

#[cfg(feature = "export")]
impl Arranged {
    fn views(&self) -> Vec<&ViewTree> {
        match self {
            Self::Article(view) => vec![view],
            Self::Book { items, .. } => items
                .iter()
                .filter_map(|item| match item {
                    ArrangedItem::Page { view, .. } => Some(view),
                    _ => None,
                })
                .collect(),
        }
    }
}

#[cfg(feature = "typst")]
fn to_pdf(arranged: Arranged) -> adoc_pdf::Document {
    use adoc_pdf::{Book, BookItem, Document, PageView};
    match arranged {
        Arranged::Article(view) => Document::Article(PageView {
            key: "p0".to_owned(),
            view,
            depth: 0,
        }),
        Arranged::Book {
            title,
            attributes,
            items,
        } => Document::Book(Book {
            title,
            subtitle: None,
            attributes,
            items: items
                .into_iter()
                .map(|item| match item {
                    ArrangedItem::Part(title) => BookItem::Part(title),
                    ArrangedItem::Heading { title, depth } => BookItem::Heading { title, depth },
                    ArrangedItem::Page {
                        key, view, depth, ..
                    } => BookItem::Page(PageView { key, view, depth }),
                })
                .collect(),
        }),
    }
}

#[cfg(feature = "docx")]
fn to_docx(arranged: Arranged) -> adoc_docx::DocxDocument {
    use adoc_docx::{BookItem, DocxBook, DocxDocument, DocxPage};
    match arranged {
        Arranged::Article(view) => DocxDocument::Article(DocxPage {
            key: "p0".to_owned(),
            view,
            depth: 0,
        }),
        Arranged::Book {
            title,
            attributes,
            items,
        } => DocxDocument::Book(DocxBook {
            title,
            subtitle: None,
            attributes,
            items: items
                .into_iter()
                .map(|item| match item {
                    ArrangedItem::Part(title) => BookItem::Part(title),
                    ArrangedItem::Heading { title, depth } => BookItem::Heading { title, depth },
                    ArrangedItem::Page {
                        key, view, depth, ..
                    } => BookItem::Page(DocxPage { key, view, depth }),
                })
                .collect(),
        }),
    }
}

#[cfg(feature = "docx")]
impl Engine {
    /// Writes the DOCX document on a worker.
    fn write_docx(
        &mut self,
        job: u64,
        args: DocxExportArgs,
        written: Written,
        commit: Commit,
        protection: crate::ExportProtection,
    ) {
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_docx(arranged);
        // The look, from the first page's attributes (a book's own) as PDF export reads them;
        // one that cannot be had fails the export.
        let attributes = match &document {
            adoc_docx::DocxDocument::Article(page) => page.view.attributes.clone(),
            adoc_docx::DocxDocument::Book(book) => book.attributes.clone(),
        };
        let resolved = self.docx_look(&args, &attributes, sources.first().map(PathBuf::as_path));
        let (options, look) = match &resolved {
            Ok(resolved) => (
                self.docx_options(
                    &args,
                    resolved.page(&attributes).as_ref(),
                    resolved.word_look(
                        &attributes,
                        &self.fonts_dirs(None, &attributes, sources.first().map(PathBuf::as_path)),
                        &self.file_access,
                    ),
                ),
                resolved.report(),
            ),
            Err(_) => (
                self.docx_options(&args, None, adoc_docx::WordLook::default()),
                crate::docx_look::ResolvedLook::None.report(),
            ),
        };
        let mut options = options;
        let asked = protection != crate::ExportProtection::Off;
        options.protection = if asked {
            adoc_docx::WordProtection::ReadOnly
        } else {
            adoc_docx::WordProtection::None
        };
        let resolved = resolved.map(drop);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("writing the document crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = resolved
                .and_then(|()| write_docx_file(&document, &options, &args.output, &commit))
                // The writer says it wrote the restriction; a request is no proof of it.
                .and_then(|report| {
                    if asked && !report.protected {
                        Err(
                            "the document was asked to be read-only but was not written so"
                                .to_owned(),
                        )
                    } else {
                        Ok(report)
                    }
                })
                .map(|report| {
                    let mut diagnostics: Vec<ExportDiagnostic> = report
                        .diagnostics
                        .into_iter()
                        .map(|diagnostic| ExportDiagnostic {
                            severity: match diagnostic.severity {
                                adoc_docx::DocxSeverity::Error => DiagnosticSeverity::Error,
                                adoc_docx::DocxSeverity::Warning => DiagnosticSeverity::Warning,
                            },
                            message: diagnostic.message,
                            location: diagnostic.source.map(|source| (source.path, source.line)),
                        })
                        .collect();
                    diagnostics.extend(failed);
                    diagnostics.extend(unread);
                    let report_protected = report.protected;
                    ExportSummary {
                        output: args.output.clone(),
                        pages: sources.len(),
                        sources,
                        diagnostics,
                        look: Some(look),
                        protected: report_protected,
                    }
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }
}

#[cfg(feature = "docx")]
impl Engine {
    /// How a document is written: `[docx]`, with the export's own paper and orientation, over
    /// the look's `page` where `[docx]` wrote nothing (the Word looks PRD, §5.1).
    fn docx_options(
        &self,
        args: &DocxExportArgs,
        page: Option<&adoc_theme::PageGeometry>,
        look: adoc_docx::WordLook,
    ) -> adoc_docx::DocxOptions {
        let settings = &self.settings.docx;
        let written = |key: &str| settings.written.contains(key);
        // Twentieths of a point.
        let twips = |points: f64| (points * 20.0).round().clamp(0.0, f64::from(u32::MAX)) as u32;
        let line = |line: &crate::DocxLine| adoc_docx::Line {
            left: line.left.clone(),
            centre: line.centre.clone(),
            right: line.right.clone(),
        };
        let slots = |slots: &crate::DocxSlots| adoc_docx::Slots {
            odd: line(&slots.odd),
            even: slots.even.as_ref().map(line),
            first: slots.first.as_ref().map(line),
        };
        let named = |paper: crate::DocxPaper| match paper {
            crate::DocxPaper::A4 => adoc_docx::Paper::A4,
            crate::DocxPaper::Letter => adoc_docx::Paper::Letter,
        };
        // The paper: the export's, else what `[docx]` wrote, else the look's (in portrait), else
        // `[docx]`'s default.
        let paper = match (args.paper, page) {
            (Some(paper), _) => named(paper),
            (None, Some(page)) if !written("paper") => {
                let (width, height) = if page.landscape {
                    (page.height, page.width)
                } else {
                    (page.width, page.height)
                };
                adoc_docx::Paper::Custom {
                    width: twips(width),
                    height: twips(height),
                }
            }
            (None, _) => named(settings.paper),
        };
        let landscape = match (args.landscape, page) {
            (Some(landscape), _) => landscape,
            (None, Some(page)) if !written("orientation") => page.landscape,
            (None, _) => settings.landscape,
        };
        let margins = settings.margins;
        let mut look = look;
        // A template's headers or footers give way to those `[docx]` wrote.
        if let Some(template) = look.template.as_mut() {
            for (header, key) in [(true, "header"), (false, "footer")] {
                if written(key) && template.has_running(header) {
                    std::sync::Arc::make_mut(template).without_running(header);
                }
            }
        }
        let template_page = look.template.as_ref().and_then(|template| template.page());
        let running = look.running.clone();
        let running = running.as_ref();
        let side = |key: &str, written_value: u32, index: usize| match page {
            Some(page) if !written(key) => twips(page.margin[index]),
            _ => written_value,
        };
        let root = self.roots.first();
        adoc_docx::DocxOptions {
            look,
            paper,
            landscape,
            margins: adoc_docx::Margins {
                top: side("margins.top", margins.top, 0),
                right: side("margins.right", margins.right, 1),
                bottom: side("margins.bottom", margins.bottom, 2),
                left: side("margins.left", margins.left, 3),
                header: match running
                    .and_then(|running| running.header_distance)
                    .or(template_page.map(|page| page.margins.header))
                {
                    Some(distance) if !written("margins.header") => distance,
                    _ => margins.header,
                },
                footer: match running
                    .and_then(|running| running.footer_distance)
                    .or(template_page.map(|page| page.margins.footer))
                {
                    Some(distance) if !written("margins.footer") => distance,
                    _ => margins.footer,
                },
            },
            running: adoc_docx::Running {
                // The look's headers and footers where `[docx]` wrote none; a theme without
                // one shows none.
                header: match running {
                    Some(running) if !written("header") => {
                        running.header.clone().unwrap_or_default()
                    }
                    _ => slots(&settings.header),
                },
                footer: match running {
                    Some(running) if !written("footer") => {
                        running.footer.clone().unwrap_or_default()
                    }
                    _ => slots(&settings.footer),
                },
                odd_even: match running {
                    Some(_) if !written("odd_even") => true,
                    _ => settings.odd_even,
                },
                logo: settings
                    .logo
                    .as_deref()
                    .zip(root)
                    .map(|(logo, root)| crate::normalize_path(&root.join(logo))),
            },
            styles: adoc_docx::StyleOverrides {
                body_font: settings.styles.body_font.clone(),
                heading_font: settings.styles.heading_font.clone(),
                code_font: settings.styles.code_font.clone(),
                body_size: settings.styles.body_size,
                heading_colour: settings.styles.heading_colour.clone(),
                link_colour: settings.styles.link_colour.clone(),
            },
            // A reference out of the document leads to its page on the site, as the PDF's do.
            page_urls: self
                .site_url()
                .map(|site| self.page_urls(&site))
                .unwrap_or_default(),
            // Images, diagrams, formulas and fonts are read as the PDF's are.
            access: self.file_access.clone(),
            fonts_named_only: !settings.embed_fonts,
            // Set by the export from its decision, never from settings.
            protection: adoc_docx::WordProtection::None,
        }
    }
}

/// Writes `document` to a new file beside `output` and renames it over `output` once the whole
/// package is written: a failure leaves whatever was at `output`, and the rename replaces the
/// name, never writing into what a link leads to. A link at `output` is refused again here, in
/// case one appeared since the export was asked for.
#[cfg(feature = "docx")]
fn write_docx_file(
    document: &adoc_docx::DocxDocument,
    options: &adoc_docx::DocxOptions,
    output: &Path,
    commit: &Commit,
) -> Result<adoc_docx::DocxReport, String> {
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", output.display());
    check_docx_output(output).map_err(|error| error.to_string())?;
    // A profile may name a folder, such as `build/`, that does not exist yet.
    let folder = match output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
            parent
        }
        None => Path::new("."),
    };
    let file = tempfile::NamedTempFile::new_in(folder).map_err(|error| failed(&error))?;
    let mut sink = std::io::BufWriter::new(file);
    let report =
        adoc_docx::write_docx(document, options, &mut sink).map_err(|error| failed(&error))?;
    let file = sink
        .into_inner()
        .map_err(|error| failed(&error.into_error()))?;
    commit.replace(|| {
        file.persist(output)
            .map(drop)
            .map_err(|error| failed(&error.error))
    })?;
    Ok(report)
}

/// Typesets `document` and writes the PDF (and the Typst, when asked). With `replace`, as for
/// `export.pdf_pages`, whose file names nobody confirmed, the PDF is written to a new file
/// beside the output and renamed over it: the rename replaces whatever name is there, a link
/// included, and never writes into what a symbolic or hard link leads to.
#[cfg(feature = "typst")]
fn typeset(
    document: &adoc_pdf::Document,
    theme: &LoadedTheme,
    options: &PdfOptions,
    args: &ExportArgs,
    replace: bool,
    commit: &Commit,
) -> Result<ExportSummary, String> {
    let typst = adoc_pdf::to_typst(document, theme, options);
    // A profile may name a folder, such as `build/`, that does not exist yet.
    for path in std::iter::once(&args.output).chain(&args.typst) {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
    }
    if let Some(path) = &args.typst {
        std::fs::write(path, &typst.source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let output = adoc_pdf::compile(&typst, theme, options).map_err(|error| error.to_string())?;
    // The bytes say they carry the restriction; a request is no proof of it.
    if options.protection != adoc_pdf::PdfProtection::None && !output.protected {
        return Err("the PDF was asked to be read-only but was not written so".to_owned());
    }
    let failed = |error: std::io::Error| format!("{}: {error}", args.output.display());
    if replace {
        let folder = args
            .output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(folder).map_err(failed)?;
        std::io::Write::write_all(&mut file, &output.bytes).map_err(failed)?;
        commit.replace(|| {
            file.persist(&args.output)
                .map(drop)
                .map_err(|error| failed(error.error))
        })?;
    } else {
        commit.replace(|| std::fs::write(&args.output, &output.bytes).map_err(failed))?;
    }
    let diagnostics: Vec<ExportDiagnostic> = output
        .diagnostics
        .iter()
        .map(|diagnostic| ExportDiagnostic {
            severity: match diagnostic.severity {
                PdfSeverity::Error => DiagnosticSeverity::Error,
                PdfSeverity::Warning => DiagnosticSeverity::Warning,
            },
            message: diagnostic.message.clone(),
            location: diagnostic
                .source
                .as_ref()
                .map(|source| (source.path.clone(), source.line)),
        })
        .collect();
    Ok(ExportSummary {
        output: args.output.clone(),
        pages: output.pages,
        sources: Vec::new(),
        diagnostics,
        look: None,
        protected: output.protected,
    })
}

/// A warning for each page the parser could not read, which is shown as its source.
#[cfg(feature = "export")]
fn unread_pages(doms: &[(PathBuf, Dom)]) -> Vec<ExportDiagnostic> {
    doms.iter()
        .filter(|(_, dom)| {
            matches!(dom.blocks.as_slice(), [block] if matches!(
                &block.kind,
                BlockKind::Unsupported { name, .. } if name == "document"
            ))
        })
        .map(|(path, dom)| {
            let reason = dom.warnings.first().map_or("", |warning| &warning.message);
            ExportDiagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "the page could not be parsed and is shown as its source: {reason}"
                ),
                location: Some((path.clone(), 1)),
            }
        })
        .collect()
}

/// A warning for each diagram that could not be rendered and is shown as its source.
#[cfg(feature = "export")]
fn failed_diagrams(arranged: &Arranged) -> Vec<ExportDiagnostic> {
    let mut out = Vec::new();
    for view in arranged.views() {
        view.each_block(&mut |block| {
            if let ViewKind::Diagram(DiagramView::Failed { kind, message, .. }) = &block.kind {
                out.push(ExportDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: format!("{kind} diagram not rendered: {message}"),
                    location: Some((block.span.path.clone(), block.span.line)),
                });
            }
        });
    }
    out
}

#[cfg(feature = "kindle")]
impl Engine {
    /// Writes the job's Kindle book on a worker: mapped, written in memory and checked by
    /// `adoc-kindle`, then put in place through the job's commit.
    fn write_kindle(
        &mut self,
        job: u64,
        args: KindleExportArgs,
        written: Written,
        commit: Commit,
        protection: &crate::ExportProtection,
    ) {
        // A Kindle book carries no editing restriction, so pages asking for one were refused
        // before anything was parsed; nothing is written that claims one.
        if *protection != crate::ExportProtection::Off {
            self.export_finished(
                job,
                Err("a Kindle book cannot carry an editing restriction".to_owned()),
            );
            return;
        }
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_book(arranged);
        // The export's own (a profile's), else `[kindle]`'s, else the document's.
        let defaults = &self.settings.kindle;
        let root = self.roots.first().cloned();
        let cover = args.cover.clone().or_else(|| {
            let cover = defaults.cover.as_deref()?;
            Some(match &root {
                Some(root) => crate::normalize_path(&root.join(cover)),
                None => PathBuf::from(cover),
            })
        });
        let options = adoc_kindle::KindleOptions {
            format: adoc_ebook::Format::Kindle,
            title: args.title.clone().or_else(|| defaults.title.clone()),
            creator: args.creator.clone().or_else(|| defaults.creator.clone()),
            language: args.language.clone().or_else(|| defaults.language.clone()),
            identifier: args
                .identifier
                .clone()
                .or_else(|| defaults.identifier.clone()),
            publisher: defaults.publisher.clone(),
            cover,
            page_urls: self
                .site_url()
                .map(|site| self.page_urls(&site))
                .unwrap_or_default(),
            access: self.file_access.clone(),
        };
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("writing the book crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let format = match args.format {
                KindleFormat::Azw3 => adoc_kindle::KindleFormat::Azw3,
                KindleFormat::Kfx => adoc_kindle::KindleFormat::Kfx,
            };
            let result = adoc_kindle::write_kindle(&document, format, &options)
                .map_err(|error| format!("{}: {error}", args.output.display()))
                .and_then(|output| {
                    let format = args.format;
                    write_book_file(&output.bytes, &args.output, &commit, |output| {
                        check_kindle_output(format, output)
                    })?;
                    Ok(output.diagnostics)
                })
                .map(|written| {
                    let mut diagnostics = book_diagnostics(written);
                    diagnostics.extend(failed);
                    diagnostics.extend(unread);
                    ExportSummary {
                        output: args.output.clone(),
                        pages: sources.len(),
                        sources,
                        diagnostics,
                        look: None,
                        protected: false,
                    }
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }
}

#[cfg(feature = "epub")]
impl Engine {
    /// Writes the job's EPUB on a worker: mapped, written in memory and checked by
    /// `adoc-epub`, then put in place through the job's commit.
    fn write_epub(
        &mut self,
        job: u64,
        args: EpubExportArgs,
        written: Written,
        commit: Commit,
        protection: &crate::ExportProtection,
    ) {
        // An EPUB carries no editing restriction, so pages asking for one were refused
        // before anything was parsed; nothing is written that claims one.
        if *protection != crate::ExportProtection::Off {
            self.export_finished(
                job,
                Err("an EPUB cannot carry an editing restriction".to_owned()),
            );
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
            });
        let source_date_epoch = std::env::var("SOURCE_DATE_EPOCH").ok();
        // The export's own (a profile's), else `[epub]`'s, else the document's.
        let defaults = self.settings.epub.clone();
        let root = self.roots.first().cloned();
        let in_root = |relative: &str| match &root {
            Some(root) => crate::normalize_path(&root.join(relative)),
            None => PathBuf::from(relative),
        };
        let given = args.modified.or(defaults.modified);
        let (modified, dated_now) = match epub_modified(given, source_date_epoch.as_deref(), now) {
            Ok(modified) => modified,
            Err(error) => {
                self.export_finished(job, Err(error));
                return;
            }
        };
        let css = args
            .css
            .clone()
            .or_else(|| defaults.css.as_deref().map(in_root));
        let stylesheet = match css.map(|css| self.epub_stylesheet(&css)).transpose() {
            Ok(stylesheet) => stylesheet,
            Err(error) => {
                self.export_finished(job, Err(error));
                return;
            }
        };
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_book(arranged);
        let options = adoc_epub::EpubOptions {
            book: adoc_epub::BookOptions {
                format: adoc_ebook::Format::Epub,
                title: args.title.clone().or(defaults.title),
                creator: args.creator.clone().or(defaults.creator),
                language: args.language.clone().or(defaults.language),
                identifier: args.identifier.clone().or(defaults.identifier),
                publisher: args.publisher.clone().or(defaults.publisher),
                cover: args
                    .cover
                    .clone()
                    .or_else(|| defaults.cover.as_deref().map(in_root)),
                page_urls: self
                    .site_url()
                    .map(|site| self.page_urls(&site))
                    .unwrap_or_default(),
                access: self.file_access.clone(),
            },
            modified,
            stylesheet,
        };
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("writing the book crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = adoc_epub::write_epub(&document, &options)
                .map_err(|error| format!("{}: {error}", args.output.display()))
                .and_then(|output| {
                    write_book_file(&output.bytes, &args.output, &commit, check_epub_output)?;
                    Ok(output.diagnostics)
                })
                .map(|written| {
                    let mut diagnostics = book_diagnostics(written);
                    diagnostics.extend(failed);
                    diagnostics.extend(unread);
                    diagnostics.extend(dated_now);
                    ExportSummary {
                        output: args.output.clone(),
                        pages: sources.len(),
                        sources,
                        diagnostics,
                        look: None,
                        protected: false,
                    }
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }
}

#[cfg(feature = "epub")]
impl Engine {
    /// A user stylesheet, read through the export's file access and checked: one outside the
    /// folders the export may read, unreadable, or that could reach outside the book fails the
    /// export, rather than being left out unseen.
    fn epub_stylesheet(&self, path: &Path) -> Result<String, String> {
        if !self.file_access.permits(path) {
            return Err(format!(
                "the stylesheet {} is outside the folders the export may read",
                path.display()
            ));
        }
        let bytes = self
            .file_access
            .read(path)
            .map_err(|error| format!("the stylesheet {}: {error}", path.display()))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("the stylesheet {} is not UTF-8 text", path.display()))?;
        adoc_epub::check_stylesheet(&text)
            .map_err(|problem| format!("{}: {problem}", path.display()))?;
        Ok(text)
    }
}

/// The time an EPUB is dated with: the export's own, else `SOURCE_DATE_EPOCH`'s, else `now`,
/// with a note that exporting again will give other bytes. A time an EPUB cannot be dated
/// with (before 1980 or after 2107) fails the export.
#[cfg(feature = "epub")]
fn epub_modified(
    given: Option<i64>,
    source_date_epoch: Option<&str>,
    now: i64,
) -> Result<(adoc_epub::Modified, Option<ExportDiagnostic>), String> {
    let datable = |seconds: i64, what: &str| {
        adoc_epub::Modified::from_unix_seconds(seconds)
            .ok_or_else(|| format!("{what} is not a time an EPUB can be dated with (1980 to 2107)"))
    };
    if let Some(seconds) = given {
        return Ok((
            datable(seconds, &format!("the modified time {seconds}"))?,
            None,
        ));
    }
    if let Some(value) = source_date_epoch.filter(|value| !value.trim().is_empty()) {
        let what = format!("SOURCE_DATE_EPOCH {value}");
        let seconds = value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{what} is not a number of seconds"))?;
        return Ok((datable(seconds, &what)?, None));
    }
    let modified = datable(now, "the time of the export")?;
    let note = ExportDiagnostic {
        severity: DiagnosticSeverity::Information,
        message: format!(
            "the book is dated with the time of the export, {modified}, so exporting it again \
             gives other bytes; set SOURCE_DATE_EPOCH to repeat it"
        ),
        location: None,
    };
    Ok((modified, Some(note)))
}

/// The arranged views as the book model Kindle and EPUB export both write.
#[cfg(any(feature = "kindle", feature = "epub"))]
fn to_book(arranged: Arranged) -> adoc_ebook::Document {
    use adoc_ebook::{BookItem, Document, Page, Plan};
    match arranged {
        Arranged::Article(view) => Document::Article(Page {
            key: "p0".to_owned(),
            view,
            depth: 0,
        }),
        Arranged::Book {
            title,
            attributes,
            items,
        } => Document::Book(Plan {
            title,
            subtitle: None,
            attributes,
            items: items
                .into_iter()
                .map(|item| match item {
                    ArrangedItem::Part(title) => BookItem::Part(title),
                    ArrangedItem::Heading { title, depth } => BookItem::Heading { title, depth },
                    ArrangedItem::Page {
                        key, view, depth, ..
                    } => BookItem::Page(Page { key, view, depth }),
                })
                .collect(),
        }),
    }
}

/// A book's diagnostics as the export's.
#[cfg(any(feature = "kindle", feature = "epub"))]
fn book_diagnostics(diagnostics: Vec<adoc_ebook::Diagnostic>) -> Vec<ExportDiagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| ExportDiagnostic {
            severity: match diagnostic.severity {
                adoc_ebook::Severity::Error => DiagnosticSeverity::Error,
                adoc_ebook::Severity::Warning => DiagnosticSeverity::Warning,
            },
            message: diagnostic.message,
            location: diagnostic.source.map(|source| (source.path, source.line)),
        })
        .collect()
}

/// Writes a checked book's bytes to a new file beside `output` and renames it over `output`
/// through the job's commit, so a failed or abandoned export leaves what was there. `check`,
/// the format's output check, refuses a link at `output` again here, in case one appeared
/// since the export was asked for.
#[cfg(any(feature = "kindle", feature = "epub"))]
fn write_book_file(
    bytes: &[u8],
    output: &Path,
    commit: &Commit,
    check: impl Fn(&Path) -> Result<(), EngineError>,
) -> Result<(), String> {
    use std::io::Write as _;
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", output.display());
    check(output).map_err(|error| error.to_string())?;
    // A profile may name a folder, such as `build/`, that does not exist yet.
    let folder = match output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
            parent
        }
        None => Path::new("."),
    };
    let mut file = tempfile::NamedTempFile::new_in(folder).map_err(|error| failed(&error))?;
    file.write_all(bytes).map_err(|error| failed(&error))?;
    file.flush().map_err(|error| failed(&error))?;
    commit.replace(|| {
        file.persist(output)
            .map(drop)
            .map_err(|error| failed(&error.error))
    })
}

#[cfg(feature = "docbook")]
impl Engine {
    /// Writes the job's DocBook on a worker: written in memory and checked by `adoc-docbook`,
    /// then the document and its assets folder put in place through the job's commit.
    fn write_docbook(
        &mut self,
        job: u64,
        args: DocBookExportArgs,
        written: Written,
        commit: Commit,
        protection: &crate::ExportProtection,
    ) {
        // DocBook carries no editing restriction, so pages asking for one were refused before
        // anything was parsed; nothing is written that claims one.
        if *protection != crate::ExportProtection::Off {
            self.export_finished(
                job,
                Err("DocBook cannot carry an editing restriction".to_owned()),
            );
            return;
        }
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_docbook(arranged);
        let options = adoc_docbook::DocBookOptions::default();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("writing the DocBook crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = adoc_docbook::write_docbook(&document, &options)
                .map_err(|error| format!("{}: {error}", args.output.display()))
                .and_then(|output| {
                    write_docbook_files(&output.xml, &[], &args.output, &commit)?;
                    Ok(output.diagnostics)
                })
                .map(|written| {
                    let mut diagnostics = docbook_diagnostics(written);
                    diagnostics.extend(failed);
                    diagnostics.extend(unread);
                    ExportSummary {
                        output: args.output.clone(),
                        pages: sources.len(),
                        sources,
                        diagnostics,
                        look: None,
                        protected: false,
                    }
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }
}

/// The arranged views as what DocBook export writes.
#[cfg(feature = "docbook")]
fn to_docbook(arranged: Arranged) -> adoc_docbook::DocBookDocument {
    use adoc_docbook::{Book, BookItem, DocBookDocument, Page};
    match arranged {
        Arranged::Article(view) => DocBookDocument::Article(Page {
            key: "p0".to_owned(),
            view,
            depth: 0,
        }),
        Arranged::Book {
            title,
            attributes,
            items,
        } => DocBookDocument::Book(Book {
            title,
            subtitle: None,
            attributes,
            items: items
                .into_iter()
                .map(|item| match item {
                    ArrangedItem::Part(title) => BookItem::Part(title),
                    ArrangedItem::Heading { title, depth } => BookItem::Heading { title, depth },
                    ArrangedItem::Page {
                        key,
                        view,
                        depth,
                        preface,
                    } => {
                        let page = Page { key, view, depth };
                        if preface {
                            BookItem::Preface(page)
                        } else {
                            BookItem::Page(page)
                        }
                    }
                })
                .collect(),
        }),
    }
}

/// DocBook's diagnostics as the export's.
#[cfg(feature = "docbook")]
fn docbook_diagnostics(diagnostics: Vec<adoc_docbook::Diagnostic>) -> Vec<ExportDiagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| ExportDiagnostic {
            severity: match diagnostic.severity {
                adoc_docbook::Severity::Error => DiagnosticSeverity::Error,
                adoc_docbook::Severity::Warning => DiagnosticSeverity::Warning,
            },
            message: diagnostic.message,
            location: diagnostic.source.map(|source| (source.path, source.line)),
        })
        .collect()
}

/// Puts a checked DocBook document and its pictures (`assets`, each a name in the assets
/// folder and its bytes) in place, so that a failed or abandoned export leaves what was there.
///
/// Both are written first to a staging folder beside `output`. Then, through the job's commit,
/// the old assets folder is moved into the staging folder, the new one (if there are pictures)
/// moved into its place, and the document renamed over `output` last. A document and a folder
/// cannot be replaced in one step: if the document's rename fails, the old folder is put back,
/// so the old document still finds its pictures. The old folder goes with the staging folder.
/// The output's checks run again inside the commit, in case a link or a stranger's file
/// appeared since the export was asked for.
#[cfg(feature = "docbook")]
fn write_docbook_files(
    xml: &str,
    assets: &[(String, Vec<u8>)],
    output: &Path,
    commit: &Commit,
) -> Result<(), String> {
    let failed =
        |path: &Path, error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    check_docbook_output(output).map_err(|error| error.to_string())?;
    // A profile may name a folder, such as `build/`, that does not exist yet.
    let folder = match output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => {
            std::fs::create_dir_all(parent).map_err(|error| failed(parent, &error))?;
            parent
        }
        None => Path::new("."),
    };
    let staging = tempfile::Builder::new()
        .prefix(".adocfoundry-docbook-")
        .tempdir_in(folder)
        .map_err(|error| failed(folder, &error))?;
    let new_assets = staging.path().join("assets");
    if !assets.is_empty() {
        std::fs::create_dir(&new_assets).map_err(|error| failed(&new_assets, &error))?;
        for (name, bytes) in assets {
            let path = new_assets.join(name);
            std::fs::write(&path, bytes).map_err(|error| failed(&path, &error))?;
        }
    }
    let new_xml = staging.path().join("document.xml");
    std::fs::write(&new_xml, xml).map_err(|error| failed(&new_xml, &error))?;
    let assets_folder = docbook_assets_folder(output);
    let old_assets = staging.path().join("old");
    commit.replace(|| {
        check_docbook_output(output).map_err(|error| error.to_string())?;
        let had_old = std::fs::symlink_metadata(&assets_folder).is_ok();
        if had_old {
            std::fs::rename(&assets_folder, &old_assets)
                .map_err(|error| failed(&assets_folder, &error))?;
        }
        let put_back = || {
            if had_old {
                let _ = std::fs::rename(&old_assets, &assets_folder);
            }
        };
        if !assets.is_empty() {
            if let Err(error) = std::fs::rename(&new_assets, &assets_folder) {
                put_back();
                return Err(failed(&assets_folder, &error));
            }
        }
        if let Err(error) = std::fs::rename(&new_xml, output) {
            if !assets.is_empty() {
                let _ = std::fs::remove_dir_all(&assets_folder);
            }
            put_back();
            return Err(failed(output, &error));
        }
        Ok(())
    })
}

#[cfg(all(test, feature = "epub"))]
mod tests {
    use super::*;

    /// 2026-11-19T10:30:00Z.
    const TIME: i64 = 1_795_084_200;

    #[test]
    fn an_epub_is_dated_from_its_export_then_source_date_epoch_then_now() {
        let (modified, note) = epub_modified(Some(TIME), Some("1"), 2).unwrap();
        assert_eq!(modified.unix_seconds(), TIME);
        assert!(note.is_none());

        let (modified, note) = epub_modified(None, Some(&format!(" {TIME} ")), 2).unwrap();
        assert_eq!(modified.unix_seconds(), TIME);
        assert!(note.is_none(), "a reproducible time is not reported");

        let (modified, note) = epub_modified(None, None, TIME).unwrap();
        assert_eq!(modified.unix_seconds(), TIME);
        let note = note.expect("the time of the export is reported");
        assert_eq!(note.severity, DiagnosticSeverity::Information);
        assert!(
            note.message.contains("2026-11-19T10:30:00Z"),
            "{}",
            note.message
        );
        // An empty variable counts as unset.
        assert!(epub_modified(None, Some(""), TIME).unwrap().1.is_some());

        for (given, variable) in [(Some(0), None), (None, Some("soon")), (None, Some("1"))] {
            let error = epub_modified(given, variable, TIME).unwrap_err();
            assert!(
                error.contains("1980 to 2107") || error.contains("not a number"),
                "{error}"
            );
        }
    }
}
