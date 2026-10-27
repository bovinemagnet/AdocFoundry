//! PDF export: one page as an article, or a component version in navigation order as a book.
//! Every page is parsed on a worker from its open buffer or its file, its view is built and
//! its diagrams rendered, and once none is pending the document is typeset and written on
//! another worker. The result arrives as [`EngineEvent::ExportFinished`].

use std::path::PathBuf;

use adoc_core::DiagnosticSeverity;

use crate::{DocumentId, PdfStandard};

// The export itself needs Typst; without it only the request and result types remain.
#[cfg(feature = "typst")]
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "typst")]
use adoc_dom::{BlockKind, Dom};
#[cfg(feature = "typst")]
use adoc_pdf::{
    Book, BookItem, Document, LoadedTheme, PageView, PdfOptions, PdfSeverity, ThemeSource,
};
#[cfg(feature = "typst")]
use adoc_render::{DiagramView, ViewBlock, ViewInline, ViewKind, ViewTree};

#[cfg(feature = "typst")]
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
}

/// An export request: `CommandArgs::Export`.
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
}

/// A finished export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSummary {
    pub output: PathBuf,
    pub pages: usize,
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
    /// Without Typst there is no export: every request fails as an unknown command.
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

/// The pages of an export and how they are arranged.
#[cfg(feature = "typst")]
#[derive(Clone, Debug)]
enum Plan {
    Article(PathBuf),
    Book {
        title: String,
        attributes: BTreeMap<String, String>,
        items: Vec<PlanItem>,
    },
}

#[cfg(feature = "typst")]
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

#[cfg(feature = "typst")]
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

#[cfg(feature = "typst")]
enum Stage {
    Parsing,
    /// Diagrams still rendering, by checksum.
    Diagrams(BTreeSet<String>),
    Typesetting,
}

#[cfg(feature = "typst")]
struct Job {
    args: ExportArgs,
    plan: Plan,
    stage: Stage,
    doms: Vec<(PathBuf, Dom)>,
    /// Kept for `export_blocking`, which takes it; otherwise only published.
    keep_result: bool,
}

#[cfg(feature = "typst")]
#[derive(Default)]
pub(crate) struct ExportState {
    jobs: BTreeMap<u64, Job>,
    finished: BTreeMap<u64, Result<ExportSummary, String>>,
    next: u64,
}

#[cfg(feature = "typst")]
pub(crate) fn register_export(registry: &mut CommandRegistry) {
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
}

#[cfg(feature = "typst")]
fn export_command(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Export(args) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_PDF_PAGE,
            expected: "an export target and output path",
        });
    };
    engine.start_export(args, false)?;
    Ok(CommandOutcome::Done)
}

#[cfg(feature = "typst")]
impl Engine {
    /// Whether an export is under way.
    #[must_use]
    pub fn is_exporting(&self) -> bool {
        !self.export.jobs.is_empty()
    }

    /// Exports and waits for the result, for front ends without an event loop such as the CLI.
    pub fn export_blocking(
        &mut self,
        args: ExportArgs,
        timeout: Duration,
    ) -> Result<ExportSummary, EngineError> {
        let deadline = Instant::now() + timeout;
        let job = self.start_export(args, true)?;
        loop {
            if let Some(result) = self.export.finished.remove(&job) {
                return result.map_err(EngineError::Refused);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || !self.wait_for_workers(remaining) {
                self.export.jobs.remove(&job);
                return Err(EngineError::Refused(
                    "the export did not finish in time".to_owned(),
                ));
            }
            if !self.export.jobs.contains_key(&job) && !self.export.finished.contains_key(&job) {
                return Err(EngineError::Refused("the export stopped".to_owned()));
            }
            if self.workers_running == 0 && self.export.jobs.contains_key(&job) {
                // Nothing left to wait for: every diagram has reported, so typeset now.
                self.export_ready(job);
            }
        }
    }

    fn start_export(&mut self, args: ExportArgs, keep_result: bool) -> Result<u64, EngineError> {
        let plan = self.export_plan(&args.target)?;
        let inputs: Vec<_> = plan
            .paths()
            .iter()
            .map(|path| {
                self.parse_input_for_path(path).ok_or_else(|| {
                    EngineError::NotFound(format!("{} cannot be read", path.display()))
                })
            })
            .collect::<Result<_, _>>()?;
        self.export.next += 1;
        let job = self.export.next;
        self.export.jobs.insert(
            job,
            Job {
                args,
                plan,
                stage: Stage::Parsing,
                doms: Vec::new(),
                keep_result,
            },
        );
        let parser = self.document_parser();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
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
            ExportTarget::Component { name, version } => self.book_plan(name, version.as_deref()),
        }
    }

    /// A component version as a book: one part per nav file (or titled list) when there are
    /// several, chapters from top-level entries, sections from nested ones, each page once.
    fn book_plan(&self, name: &str, version: Option<&str>) -> Result<Plan, EngineError> {
        let antora = &self.workspace.antora;
        let component = antora.component(name, version).ok_or_else(|| {
            EngineError::NotFound(format!("no Antora component `{name}` in the workspace"))
        })?;
        let trees = self
            .nav_trees(name, version)
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
        let title = attributes
            .get("pdf-title")
            .cloned()
            .or_else(|| component.title.clone())
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
        let id = item.target.as_ref()?;
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
            waiting.extend(wanted.iter().map(adoc_diagrams::DiagramRequest::checksum));
            views.push((path.clone(), view));
        }
        if let Some(state) = self.export.jobs.get_mut(&job) {
            state.doms = doms;
        }
        (views, waiting)
    }

    /// Every diagram is in: typeset and write the document on a worker.
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
        state.stage = Stage::Typesetting;
        let unread = unread_pages(&state.doms);
        let args = state.args.clone();
        let plan = state.plan.clone();
        let document = assemble(&plan, views);
        let first_page = plan.paths().into_iter().next();
        let attributes = match &document {
            Document::Article(page) => page.view.attributes.clone(),
            Document::Book(book) => book.attributes.clone(),
        };
        let theme = self.export_theme(&args, &attributes, first_page.as_deref());
        let options = self.export_options(&args, &attributes, first_page.as_deref());
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let result = theme
                .and_then(|theme| typeset(&document, &theme, &options, &args))
                .map(|mut summary| {
                    summary.diagnostics.extend(unread);
                    summary
                });
            let _ = sender.send(WorkerMessage::ExportFinished { job, result });
        });
    }

    pub(crate) fn export_finished(&mut self, job: u64, result: Result<ExportSummary, String>) {
        let Some(state) = self.export.jobs.remove(&job) else {
            return;
        };
        if state.keep_result {
            self.export.finished.insert(job, result.clone());
        }
        self.emit(EngineEvent::ExportFinished {
            output: state.args.output,
            result,
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

    fn export_options(
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
        // The settings' site, else the playbook's.
        let site_url = self.settings.pdf.site_url.clone().or_else(|| {
            self.workspace
                .antora
                .playbook()
                .and_then(|playbook| playbook.site_url.clone())
        });
        let page_urls = site_url
            .as_deref()
            .map(|site| self.page_urls(site))
            .unwrap_or_default();
        PdfOptions {
            standards,
            fonts_dirs,
            site_url,
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

    /// The published address of every page in the catalogue, as Antora lays out a site:
    /// `<site>/<component>/<version>/<module>/<page>.html`, without `ROOT` or `~`.
    fn page_urls(&self, site: &str) -> HashMap<PathBuf, String> {
        let site = site.trim_end_matches('/');
        self.workspace
            .antora
            .resources()
            .filter(|resource| resource.coordinate.family == crate::ResourceFamily::Page)
            .map(|resource| {
                let coordinate = &resource.coordinate;
                let mut url = format!("{site}/{}", coordinate.component);
                if let Some(version) = coordinate
                    .version
                    .as_deref()
                    .filter(|version| *version != "~")
                {
                    url.push('/');
                    url.push_str(version);
                }
                if coordinate.module != "ROOT" {
                    url.push('/');
                    url.push_str(&coordinate.module);
                }
                let page = coordinate
                    .relative_path
                    .with_extension("html")
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                url.push('/');
                url.push_str(&page);
                (resource.source_path.clone(), url)
            })
            .collect()
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

/// The views arranged as the plan says.
#[cfg(feature = "typst")]
fn assemble(plan: &Plan, views: Vec<(PathBuf, ViewTree)>) -> Document {
    let mut views: HashMap<PathBuf, ViewTree> = views.into_iter().collect();
    match plan {
        Plan::Article(path) => Document::Article(PageView {
            key: "p0".to_owned(),
            view: views.remove(path).unwrap_or_default(),
            depth: 0,
        }),
        Plan::Book {
            title,
            attributes,
            items,
        } => {
            let mut index = 0;
            let items = items
                .iter()
                .filter_map(|item| match item {
                    PlanItem::Part(title) => Some(BookItem::Part(title.clone())),
                    PlanItem::Heading { title, depth } => Some(BookItem::Heading {
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
                        Some(BookItem::Page(PageView {
                            key: format!("p{index}"),
                            view,
                            depth: *depth,
                        }))
                    }
                })
                .collect();
            Document::Book(Book {
                title: title.clone(),
                subtitle: None,
                attributes: attributes.clone(),
                items,
            })
        }
    }
}

/// Typesets `document` and writes the PDF (and the Typst, when asked).
#[cfg(feature = "typst")]
fn typeset(
    document: &Document,
    theme: &LoadedTheme,
    options: &PdfOptions,
    args: &ExportArgs,
) -> Result<ExportSummary, String> {
    let typst = adoc_pdf::to_typst(document, theme, options);
    if let Some(path) = &args.typst {
        std::fs::write(path, &typst.source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let output = adoc_pdf::compile(&typst, theme, options).map_err(|error| error.to_string())?;
    std::fs::write(&args.output, &output.bytes)
        .map_err(|error| format!("{}: {error}", args.output.display()))?;
    let mut diagnostics: Vec<ExportDiagnostic> = output
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
    diagnostics.extend(failed_diagrams(document));
    Ok(ExportSummary {
        output: args.output.clone(),
        pages: output.pages,
        diagnostics,
    })
}

/// A warning for each page the parser could not read, which is shown as its source.
#[cfg(feature = "typst")]
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
#[cfg(feature = "typst")]
fn failed_diagrams(document: &Document) -> Vec<ExportDiagnostic> {
    fn walk(blocks: &[ViewBlock], out: &mut Vec<ExportDiagnostic>) {
        for block in blocks {
            match &block.kind {
                ViewKind::Diagram(DiagramView::Failed { kind, message, .. }) => {
                    out.push(ExportDiagnostic {
                        severity: DiagnosticSeverity::Warning,
                        message: format!("{kind} diagram not rendered: {message}"),
                        location: Some((block.span.path.clone(), block.span.line)),
                    });
                }
                ViewKind::Section { blocks, .. }
                | ViewKind::Admonition { blocks, .. }
                | ViewKind::Quote { blocks, .. }
                | ViewKind::Sidebar(blocks)
                | ViewKind::Example(blocks)
                | ViewKind::Open(blocks) => walk(blocks, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    match document {
        Document::Article(page) => walk(&page.view.blocks, &mut out),
        Document::Book(book) => {
            for item in &book.items {
                if let BookItem::Page(page) = item {
                    walk(&page.view.blocks, &mut out);
                }
            }
        }
    }
    out
}
