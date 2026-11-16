//! Export, as PDF or DOCX: one page as an article, or a component version (or navigation
//! files) in navigation order as a book. Both formats share one pipeline: the plan of pages,
//! each page parsed on a worker from its open buffer or its file, its view built and its
//! diagrams rendered, and the views arranged as the plan says ([`arrange`]). Only then does the
//! format matter: the document is typeset as PDF (the `typst` feature) or written as DOCX (the
//! `docx` feature) on another worker. The result arrives as [`EngineEvent::ExportFinished`].
//!
//! The pipeline is the `export` feature, which `typst` and `docx` each turn on; a front end
//! names those, never `export` itself.

use std::path::PathBuf;

use adoc_core::DiagnosticSeverity;

use crate::{DocumentId, PdfStandard};

#[cfg(all(feature = "export", not(any(feature = "typst", feature = "docx"))))]
compile_error!("the `export` feature is turned on by `typst` or `docx`, not on its own");

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
use adoc_pdf::{LoadedTheme, PdfOptions, PdfSeverity, ThemeSource};
#[cfg(feature = "export")]
use adoc_render::{DiagramView, ViewInline, ViewKind, ViewTree};

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
}

#[cfg(feature = "export")]
impl Request {
    fn target(&self) -> &ExportTarget {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.target,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.target,
        }
    }

    fn output(&self) -> &Path {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.output,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.output,
        }
    }

    /// Refuses an output this format never writes.
    fn check_output(&self) -> Result<(), EngineError> {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(_) => Ok(()),
            #[cfg(feature = "docx")]
            Self::Docx(args) => check_docx_output(&args.output),
        }
    }

    fn attributes(&self) -> &[(String, String)] {
        match self {
            #[cfg(feature = "typst")]
            Self::Pdf(args) => &args.attributes,
            #[cfg(feature = "docx")]
            Self::Docx(args) => &args.attributes,
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

#[cfg(feature = "export")]
struct Job {
    request: Request,
    plan: Plan,
    stage: Stage,
    doms: Vec<(PathBuf, Dom)>,
    /// Kept for `export_blocking`, which takes it; otherwise only published.
    keep_result: bool,
    /// One of `export.pdf_pages`'s pages, reported in its summary rather than on its own.
    #[cfg_attr(not(feature = "typst"), allow(dead_code))]
    in_batch: bool,
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
                self.export.jobs.remove(&job);
                return Err(EngineError::Refused(
                    "the export did not finish in time".to_owned(),
                ));
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
        self.export.next += 1;
        let job = self.export.next;
        self.export.jobs.insert(
            job,
            Job {
                request,
                plan,
                stage: Stage::Parsing,
                doms: Vec::new(),
                keep_result,
                in_batch,
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
        let mut views = Vec::new();
        let mut waiting = BTreeSet::new();
        for (path, dom) in &doms {
            let (view, wanted) = self.view_of(path, dom, false);
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
        #[cfg(feature = "typst")]
        let replace = state.in_batch;
        let plan = state.plan.clone();
        let written = Written {
            sources: plan.paths(),
            arranged: arrange(&plan, views),
            unread,
        };
        match request {
            #[cfg(feature = "typst")]
            Request::Pdf(args) => self.write_pdf(job, args, written, replace),
            #[cfg(feature = "docx")]
            Request::Docx(args) => self.write_docx(job, args, written),
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
    fn write_pdf(&mut self, job: u64, args: ExportArgs, written: Written, replace: bool) {
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
        let options = self.export_options(&args, &attributes, first_page.as_deref());
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("typesetting crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = theme
                .and_then(|theme| typeset(&document, &theme, &options, &args, replace))
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

    /// Where the theme [`Engine::export_theme`] loads comes from.
    pub(crate) fn theme_source(
        &self,
        theme: Option<&str>,
        attributes: &BTreeMap<String, String>,
        page: Option<&Path>,
    ) -> ThemeSource {
        let page_dir = page.and_then(Path::parent);
        let root = self.roots.first().map(PathBuf::as_path);
        let themes_dir = attributes
            .get("pdf-themesdir")
            .map(|dir| resolve(dir, page_dir))
            .or_else(|| {
                self.settings
                    .pdf
                    .themes_dir
                    .as_deref()
                    .map(|dir| resolve(dir, root))
            });
        let name = theme
            .map(str::to_owned)
            .or_else(|| attributes.get("pdf-theme").cloned())
            .unwrap_or_else(|| self.settings.pdf.theme.clone());
        ThemeSource::named(&name, themes_dir.as_deref())
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
        }
    }

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
}

#[cfg(feature = "typst")]
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
                    ArrangedItem::Page { key, view, depth } => {
                        BookItem::Page(PageView { key, view, depth })
                    }
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
                    ArrangedItem::Page { key, view, depth } => {
                        BookItem::Page(DocxPage { key, view, depth })
                    }
                })
                .collect(),
        }),
    }
}

#[cfg(feature = "docx")]
impl Engine {
    /// Writes the DOCX document on a worker.
    fn write_docx(&mut self, job: u64, args: DocxExportArgs, written: Written) {
        let Written {
            sources,
            arranged,
            unread,
        } = written;
        let failed = failed_diagrams(&arranged);
        let document = to_docx(arranged);
        let options = self.docx_options(&args);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ExportFinished {
            job,
            result: Err(format!("writing the document crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let result = write_docx_file(&document, &options, &args.output).map(|report| {
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
                ExportSummary {
                    output: args.output.clone(),
                    pages: sources.len(),
                    sources,
                    diagnostics,
                }
            });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }
}

#[cfg(feature = "docx")]
impl Engine {
    /// How a document is written: `[docx]`, with the export's own paper and orientation.
    fn docx_options(&self, args: &DocxExportArgs) -> adoc_docx::DocxOptions {
        let settings = &self.settings.docx;
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
        let margins = settings.margins;
        let root = self.roots.first();
        adoc_docx::DocxOptions {
            paper: match args.paper.unwrap_or(settings.paper) {
                crate::DocxPaper::A4 => adoc_docx::Paper::A4,
                crate::DocxPaper::Letter => adoc_docx::Paper::Letter,
            },
            landscape: args.landscape.unwrap_or(settings.landscape),
            margins: adoc_docx::Margins {
                top: margins.top,
                right: margins.right,
                bottom: margins.bottom,
                left: margins.left,
                header: margins.header,
                footer: margins.footer,
            },
            running: adoc_docx::Running {
                header: slots(&settings.header),
                footer: slots(&settings.footer),
                odd_even: settings.odd_even,
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
            // Images, diagrams and formulas are read as the PDF's are.
            access: self.file_access.clone(),
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
    file.persist(output).map_err(|error| failed(&error.error))?;
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
    let failed = |error: std::io::Error| format!("{}: {error}", args.output.display());
    if replace {
        let folder = args
            .output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(folder).map_err(failed)?;
        std::io::Write::write_all(&mut file, &output.bytes).map_err(failed)?;
        file.persist(&args.output)
            .map_err(|error| failed(error.error))?;
    } else {
        std::fs::write(&args.output, &output.bytes).map_err(failed)?;
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
