//! The rendered preview: a view tree for each document a front end shows, rebuilt on a worker
//! once edits to it, or to any file it includes, pause for [`PREVIEW_SETTLE`].

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use adoc_antora::{AntoraResolver, ResourceFamily};
use adoc_core::{FileAccess, Location, SourceRange};
use adoc_diagrams::{DiagramCache, DiagramRequest, Diagrams, ToolPaths};
use adoc_dom::{AcdcParser, DocumentParser, Dom, IncludeResolver, ParseInput};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;
use adoc_render::{build_view, DiagramView, ResolvedLink, ViewResolver, ViewTree};
use ropey::Rope;

use crate::{
    parses::Lookup,
    worker::{self, Crashed},
    workspace::WorkerMessage,
    DocumentId, Engine, EngineEvent,
};

/// Edits re-render the preview once they pause this long.
pub const PREVIEW_SETTLE: Duration = Duration::from_millis(100);

/// A document's preview at one version.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedPreview {
    pub document: DocumentId,
    pub version: u64,
    pub path: PathBuf,
    pub view: ViewTree,
}

/// How the preview is shown. Changed by the `view.*` and `preview.*` commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewLayout {
    pub visible: bool,
    /// In its own window rather than docked beside the editor.
    pub detached: bool,
    /// The caret and the preview's scroll position follow each other.
    pub scroll_sync: bool,
    /// Shown instead of the active document.
    pub pinned: Option<DocumentId>,
    /// Shows the target's pages as the PDF export typesets them, instead of the rendered page.
    pub pdf: bool,
    /// Where the preview was before a PDF command showed the PDF, which turning the PDF off
    /// puts back: a preview the PDF opened closes again.
    pub before_pdf: Option<PreviewPlace>,
}

/// Whether the preview is shown, and where.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewPlace {
    pub visible: bool,
    pub detached: bool,
}

impl Default for PreviewLayout {
    fn default() -> Self {
        Self {
            visible: false,
            detached: false,
            scroll_sync: true,
            pinned: None,
            pdf: false,
            before_pdf: None,
        }
    }
}

impl PreviewLayout {
    /// Shows the PDF, in its own window when `detached` is `Some(true)`, docked when
    /// `Some(false)`, and where the preview is when `None`, remembering where it was.
    pub(crate) fn show_pdf(&mut self, detached: Option<bool>) {
        if !(self.pdf && self.visible) {
            self.before_pdf = Some(PreviewPlace {
                visible: self.visible,
                detached: self.detached,
            });
        }
        self.pdf = true;
        self.visible = true;
        if let Some(detached) = detached {
            self.detached = detached;
        }
    }

    /// Turns the PDF off, putting the preview back where it was before the PDF was shown.
    pub(crate) fn hide_pdf(&mut self) {
        self.pdf = false;
        if let Some(place) = self.before_pdf.take() {
            self.visible = place.visible;
            self.detached = place.detached;
        }
    }
}

/// A parsed document kept so a finished diagram can rebuild the view without re-parsing.
struct Parsed {
    version: u64,
    path: PathBuf,
    dom: Arc<Dom>,
    /// Every file the page was assembled from, itself included, and every file its diagrams
    /// read: a change to any of them previews it again.
    sources: BTreeSet<PathBuf>,
    /// Diagrams still rendering, by checksum.
    waiting: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct PreviewState {
    pub(crate) layout: PreviewLayout,
    /// The target the engine itself requested, see `sync_preview_target`.
    shown: Option<DocumentId>,
    requests: BTreeMap<DocumentId, usize>,
    scheduled: BTreeMap<DocumentId, Instant>,
    next_generation: u64,
    /// The newest job started for each document.
    latest: BTreeMap<DocumentId, u64>,
    /// Jobs older than this were started before the document last changed.
    stale_before: BTreeMap<DocumentId, u64>,
    parsed: BTreeMap<DocumentId, Parsed>,
    published: BTreeMap<DocumentId, Arc<RenderedPreview>>,
    diagrams: Option<Arc<Diagrams>>,
    /// The parser previews are built with; acdc unless set otherwise.
    parser: Option<Arc<dyn DocumentParser>>,
    rendering: BTreeSet<String>,
    /// Diagrams waiting for one of the [`DIAGRAM_WORKERS`].
    queued: std::collections::VecDeque<(Arc<Diagrams>, String, DiagramRequest)>,
    /// Diagram renders running on workers.
    running: usize,
    /// Diagrams that could not be rendered, so they are not tried again on every rebuild.
    failed: FailedDiagrams,
}

/// The most diagrams rendered at once, however many a page holds.
pub const DIAGRAM_WORKERS: usize = 4;

/// The most diagram failures remembered from one parse of a page to the next: the oldest are
/// forgotten first, when a page is parsed afresh, and tried again when next shown. Between
/// parses the failures of the pages being shown are added, never forgotten, so a view rebuilt
/// as its other diagrams finish never asks for one again.
pub const MAX_DIAGRAM_FAILURES: usize = 256;

/// Why each diagram that failed did, by checksum, at most [`MAX_DIAGRAM_FAILURES`] of them.
#[derive(Default)]
pub(crate) struct FailedDiagrams {
    messages: HashMap<String, String>,
    /// Checksums, oldest first.
    order: std::collections::VecDeque<String>,
}

impl FailedDiagrams {
    fn get(&self, checksum: &str) -> Option<&String> {
        self.messages.get(checksum)
    }

    fn insert(&mut self, checksum: String, message: String) {
        if self.messages.insert(checksum.clone(), message).is_none() {
            self.order.push_back(checksum);
        }
    }

    /// Forgets the oldest failures beyond [`MAX_DIAGRAM_FAILURES`].
    fn trim(&mut self) {
        while self.order.len() > MAX_DIAGRAM_FAILURES {
            if let Some(oldest) = self.order.pop_front() {
                self.messages.remove(&oldest);
            }
        }
    }

    fn clear(&mut self) {
        self.messages.clear();
        self.order.clear();
    }

    fn len(&self) -> usize {
        self.messages.len()
    }
}

impl PreviewState {
    /// When the next scheduled preview is to start.
    pub(crate) fn next_scheduled(&self) -> Option<std::time::Instant> {
        self.scheduled.values().min().copied()
    }
}

/// The worker's input: everything it reads, copied, so the engine stays free to change.
struct Snapshot {
    parser: Arc<dyn DocumentParser>,
    id: DocumentId,
    generation: u64,
    /// The ticket its parse is shared with ([`Engine::share_next_parse`]).
    ticket: u64,
    version: u64,
    path: PathBuf,
    text: Arc<str>,
    attributes: Vec<(String, String)>,
    includes: SnapshotIncludes,
}

/// A parse's input, owned so it can move to a worker.
pub(crate) struct OwnedInput {
    pub(crate) version: u64,
    pub(crate) path: PathBuf,
    pub(crate) text: Arc<str>,
    pub(crate) attributes: Vec<(String, String)>,
    pub(crate) includes: SnapshotIncludes,
}

impl OwnedInput {
    /// Parses the input, returning the DOM and every file the page was assembled from.
    pub(crate) fn parse(self, parser: &dyn DocumentParser) -> (Dom, BTreeSet<PathBuf>) {
        let OwnedInput {
            path,
            text,
            attributes,
            includes,
            ..
        } = self;
        let includes = Arc::new(includes);
        let dom = parser.parse(&ParseInput {
            path: &path,
            text: &text,
            attributes,
            includes: includes.clone(),
        });
        let mut sources = includes
            .read
            .lock()
            .map(|read| read.clone())
            .unwrap_or_default();
        sources.extend(includes.map.values().cloned());
        sources.insert(path);
        (dom, sources)
    }
}

/// Includes as the workspace resolves them: Antora ids through the catalog, open documents
/// from their buffers, anything else from disk.
pub(crate) struct SnapshotIncludes {
    map: BTreeMap<PathBuf, PathBuf>,
    /// Open buffers, shared rather than copied: a rope's clone is cheap.
    buffers: HashMap<PathBuf, Rope>,
    access: FileAccess,
    read: Mutex<BTreeSet<PathBuf>>,
}

impl IncludeResolver for SnapshotIncludes {
    fn read(&self, requested: &Path) -> Option<(PathBuf, String)> {
        let requested = normalize_path(requested);
        let path = self.map.get(&requested).cloned().unwrap_or(requested);
        if !self.access.permits(&path) {
            return None;
        }
        let text = match self.buffers.get(&path) {
            Some(text) => text.to_string(),
            None => fs::read_to_string(&path).ok()?,
        };
        if let Ok(mut read) = self.read.lock() {
            read.insert(path.clone());
        }
        Some((path, text))
    }
}

impl Engine {
    /// The latest preview of `id`, once one has been rendered.
    #[must_use]
    pub fn rendered_preview(&self, id: DocumentId) -> Option<Arc<RenderedPreview>> {
        self.rendered.published.get(&id).cloned()
    }

    /// Keeps a preview of `id` up to date until it is released. Requests are counted, so two
    /// views of one document release it independently.
    pub fn request_preview(&mut self, id: DocumentId) {
        if !self.documents.contains_key(&id) {
            return;
        }
        let count = self.rendered.requests.entry(id).or_default();
        *count += 1;
        if *count == 1 {
            let now = self.clock.now();
            self.rendered.scheduled.insert(id, now);
        }
    }

    /// The `asciidoctor` a front end renders with when it renders through Asciidoctor:
    /// `[tools] asciidoctor` when that is a file, else the first `asciidoctor` on `PATH`.
    #[must_use]
    pub fn asciidoctor_executable(&self) -> Option<PathBuf> {
        let configured = &self.settings.tools.asciidoctor;
        if !configured.is_empty() {
            let path = PathBuf::from(configured);
            return path.is_file().then_some(path);
        }
        let names: &[&str] = if cfg!(windows) {
            &["asciidoctor.bat", "asciidoctor.cmd", "asciidoctor.exe"]
        } else {
            &["asciidoctor"]
        };
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths).find_map(|directory| {
            names
                .iter()
                .map(|name| directory.join(name))
                .find(|candidate| candidate.is_file())
        })
    }

    /// Renders `id` and waits for the result, diagrams included, for front ends without an
    /// event loop such as the CLI. `None` when it does not finish within `timeout`.
    pub fn render_blocking(
        &mut self,
        id: DocumentId,
        timeout: Duration,
    ) -> Option<Arc<RenderedPreview>> {
        self.render_now(id);
        let finished = self.wait_for_workers(timeout);
        let preview = self.rendered_preview(id);
        self.release_preview(id);
        preview.filter(|_| finished)
    }

    /// Requests a preview of `id` and starts rendering it now, not after the settle time.
    pub(crate) fn render_now(&mut self, id: DocumentId) {
        self.request_preview(id);
        let now = self.clock.now();
        self.rendered.scheduled.insert(id, now);
        self.start_due_previews();
    }

    /// The preview of `id` once it shows the document's current text and no diagram it draws
    /// is still rendering.
    pub(crate) fn finished_preview(&self, id: DocumentId) -> Option<Arc<RenderedPreview>> {
        let version = self.documents.get(&id)?.version;
        let parsed = self.rendered.parsed.get(&id)?;
        let preview = self.rendered.published.get(&id)?;
        (preview.version == version
            && parsed.waiting.is_empty()
            && !self.rendered.scheduled.contains_key(&id))
        .then(|| preview.clone())
    }

    pub fn release_preview(&mut self, id: DocumentId) {
        let Some(count) = self.rendered.requests.get_mut(&id) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            self.forget_preview(id);
        }
    }

    pub(crate) fn forget_preview(&mut self, id: DocumentId) {
        let state = &mut self.rendered;
        if state.shown == Some(id) {
            state.shown = None;
        }
        state.requests.remove(&id);
        state.scheduled.remove(&id);
        state.latest.remove(&id);
        state.stale_before.remove(&id);
        state.parsed.remove(&id);
        state.published.remove(&id);
        #[cfg(feature = "typst")]
        self.forget_pdf_preview(id);
    }

    #[must_use]
    pub fn preview_layout(&self) -> PreviewLayout {
        self.rendered.layout
    }

    pub(crate) fn set_preview_layout(&mut self, layout: PreviewLayout) {
        if self.rendered.layout != layout {
            self.rendered.layout = layout;
            self.emit(EngineEvent::PreviewLayoutChanged);
        }
        self.sync_preview_target();
    }

    /// Keeps the preview target requested, and releases the previous one, as the active
    /// document, the pin or visibility changes.
    pub(crate) fn sync_preview_target(&mut self) {
        let target = self.preview_target();
        if target == self.rendered.shown {
            return;
        }
        if let Some(previous) = self.rendered.shown.take() {
            self.release_preview(previous);
        }
        if let Some(target) = target {
            self.request_preview(target);
        }
        self.rendered.shown = target;
    }

    /// The document the preview shows: the pinned one, or else the active one.
    #[must_use]
    pub fn preview_target(&self) -> Option<DocumentId> {
        let layout = self.rendered.layout;
        if !layout.visible {
            return None;
        }
        // Only an AsciiDoc page has a rendering.
        let page = |id: &DocumentId| {
            self.documents
                .get(id)
                .is_some_and(|document| document.asciidoc)
        };
        layout.pinned.filter(page).or(self.active.filter(page))
    }

    /// Schedules a re-render of every previewed document assembled from `path`.
    pub(crate) fn preview_source_changed(&mut self, path: &Path) {
        let affected: Vec<DocumentId> = self
            .rendered
            .requests
            .keys()
            .copied()
            .filter(|id| {
                self.documents
                    .get(id)
                    .is_some_and(|document| document.path == path)
                    || self
                        .rendered
                        .parsed
                        .get(id)
                        .is_some_and(|parsed| parsed.sources.contains(path))
            })
            .collect();
        for id in affected {
            self.schedule_preview(id);
        }
    }

    /// A file that is not a page changed on disk: the previews and visual views with a diagram
    /// that reads it, such as a PlantUML `!include`, are shown again, keyed afresh by it.
    pub(crate) fn diagram_input_changed(&mut self, path: &Path) {
        self.preview_source_changed(path);
        if self.visual_reads(path) {
            self.visual_source_changed(path);
        }
    }

    /// Schedules a re-render of every previewed document, after a catalog or index change.
    pub(crate) fn preview_workspace_changed(&mut self) {
        let ids: Vec<DocumentId> = self.rendered.requests.keys().copied().collect();
        for id in ids {
            self.schedule_preview(id);
        }
    }

    fn schedule_preview(&mut self, id: DocumentId) {
        let deadline = self.clock.now() + PREVIEW_SETTLE;
        let state = &mut self.rendered;
        state.scheduled.insert(id, deadline);
        state.stale_before.insert(id, state.next_generation + 1);
    }

    /// Starts a worker for every preview whose settle time has passed. Returns the number
    /// started.
    pub(crate) fn start_due_previews(&mut self) -> usize {
        let now = self.clock.now();
        let due: Vec<DocumentId> = self
            .rendered
            .scheduled
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.rendered.scheduled.remove(id);
            match self.shared_parse(*id) {
                // Parsed already for the visual model or another consumer.
                Lookup::Ready(parse) => {
                    self.rendered.next_generation += 1;
                    let generation = self.rendered.next_generation;
                    self.rendered.latest.insert(*id, generation);
                    self.preview_parsed(
                        *id,
                        generation,
                        parse.version,
                        parse.dom.clone(),
                        parse.sources.clone(),
                    );
                }
                // Being parsed for another consumer: the preview waits for that parse.
                Lookup::Running => {
                    self.rendered.scheduled.insert(*id, now);
                }
                // Its parse crashed: the preview keeps the view it had until the page changes.
                Lookup::Failed => {}
                // Its input reads the page's line parse, which is on its way from a worker.
                Lookup::Missing if !self.line_parse_ready(*id) => {
                    self.rendered.scheduled.insert(*id, now);
                }
                Lookup::Missing => {
                    if let Some(snapshot) = self.preview_snapshot(*id) {
                        self.spawn_preview(snapshot);
                    }
                }
            }
        }
        due.len()
    }

    fn preview_snapshot(&mut self, id: DocumentId) -> Option<Snapshot> {
        let input = self.parse_input(id)?;
        self.rendered.next_generation += 1;
        let generation = self.rendered.next_generation;
        self.rendered.latest.insert(id, generation);
        let ticket = self.share_next_parse(id, input.version);
        Some(Snapshot {
            parser: self.document_parser(),
            id,
            generation,
            ticket,
            version: input.version,
            path: input.path,
            text: input.text,
            attributes: input.attributes,
            includes: input.includes,
        })
    }

    /// The parser documents are read with: acdc unless set otherwise.
    pub(crate) fn document_parser(&self) -> Arc<dyn DocumentParser> {
        self.rendered
            .parser
            .clone()
            .unwrap_or_else(|| Arc::new(AcdcParser))
    }

    /// Everything a parse of `id` reads, copied: its text, the Antora attributes, and the
    /// workspace's includes with open buffers.
    pub(crate) fn parse_input(&self, id: DocumentId) -> Option<OwnedInput> {
        let document = self.documents.get(&id)?;
        Some(self.owned_input(
            &document.path,
            document.text.to_string().into(),
            document.version,
            document.parsed(),
        ))
    }

    /// The parse input of document `id` as it would be if it held `text`.
    pub(crate) fn parse_input_of_text(&self, id: DocumentId, text: &str) -> Option<OwnedInput> {
        let document = self.documents.get(&id)?;
        let parsed = adoc_parser::parse(&crate::document::uri_for(&document.path), text).document;
        Some(self.owned_input(&document.path, text.into(), document.version, &parsed))
    }

    /// The parse input of the page at `path`: its open buffer when it is open, else the file.
    #[cfg(feature = "typst")]
    pub(crate) fn parse_input_for_path(&self, path: &Path) -> Option<OwnedInput> {
        if let Some(id) = self.document_for_path(path) {
            return self.parse_input(id);
        }
        let text = self.file_access.read_to_string(path).ok()?;
        let parsed = adoc_parser::parse(&crate::document::uri_for(path), &text).document;
        Some(self.owned_input(path, text.into(), 0, &parsed))
    }

    /// The attributes a page is parsed with before its own header: the playbook's, its
    /// component version's and Antora's page attributes.
    pub(crate) fn antora_attributes_for(&self, path: &Path) -> Vec<(String, String)> {
        let antora = &self.workspace.antora;
        let mut attributes: Vec<(String, String)> = antora
            .playbook()
            .map(|playbook| {
                playbook
                    .asciidoc_attributes
                    .iter()
                    // A trailing `@` marks a soft-set value; the value is what precedes it.
                    .map(|(name, value)| (name.clone(), value.trim_end_matches('@').to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        attributes.extend(
            adoc_lang::attributes::component_attributes(antora, path)
                .map(|(name, value)| (name.to_owned(), value.to_owned())),
        );
        attributes.extend(adoc_lang::attributes::antora_attributes(antora, path));
        attributes
    }

    fn owned_input(
        &self,
        path: &Path,
        text: Arc<str>,
        version: u64,
        parsed: &adoc_core::Document,
    ) -> OwnedInput {
        let attributes = self.antora_attributes_for(path);
        let map = adoc_lang::includes::include_map(
            &self.workspace.index,
            &self.workspace.antora,
            path,
            parsed,
        );
        let buffers = self
            .documents
            .values()
            .filter(|open| open.path != path)
            .map(|open| (open.path.clone(), open.text.clone()))
            .collect();
        OwnedInput {
            version,
            path: path.to_path_buf(),
            text,
            attributes,
            includes: SnapshotIncludes {
                map,
                buffers,
                access: self.file_access.clone(),
                read: Mutex::default(),
            },
        }
    }

    fn spawn_preview(&mut self, snapshot: Snapshot) {
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let (id, version, ticket) = (snapshot.id, snapshot.version, snapshot.ticket);
        let crashed = move |error| WorkerMessage::Crashed {
            job: Crashed::Preview {
                id,
                version,
                ticket,
            },
            error,
        };
        worker::spawn(sender, crashed, move |sender| {
            let Snapshot {
                parser,
                id,
                generation,
                ticket,
                version,
                path,
                text,
                attributes,
                includes,
            } = snapshot;
            let input = OwnedInput {
                version,
                path,
                text: text.clone(),
                attributes,
                includes,
            };
            let (dom, sources) = input.parse(parser.as_ref());
            let _ = sender.send(WorkerMessage::PreviewParsed {
                id,
                generation,
                ticket,
                version,
                text,
                dom: Arc::new(dom),
                sources,
            });
        });
    }

    pub(crate) fn preview_parsed(
        &mut self,
        id: DocumentId,
        generation: u64,
        version: u64,
        dom: Arc<Dom>,
        sources: BTreeSet<PathBuf>,
    ) {
        let state = &self.rendered;
        let superseded = state.latest.get(&id) != Some(&generation)
            || state
                .stale_before
                .get(&id)
                .is_some_and(|before| generation < *before);
        let Some(document) = self.documents.get(&id) else {
            return;
        };
        if superseded || !state.requests.contains_key(&id) {
            return;
        }
        let path = document.path.clone();
        self.rendered.parsed.insert(
            id,
            Parsed {
                version,
                path,
                dom,
                sources,
                waiting: BTreeSet::new(),
            },
        );
        self.trim_diagram_failures();
        self.publish_preview(id);
    }

    /// The view of `dom`, with the checksums of the diagrams it still needs, which are started. Formulas are
    /// rendered as images only for views that draw them (`formulas`); a PDF typesets its own.
    pub(crate) fn view_of(
        &mut self,
        path: &Path,
        dom: &Dom,
        formulas: bool,
    ) -> (ViewTree, BTreeSet<String>) {
        let (view, waiting, _) = self.view_and_inputs(path, dom, formulas);
        (view, waiting)
    }

    /// [`Engine::view_of`], with the files the view's diagrams read, whose changes are to
    /// show it again ([`adoc_diagrams::Diagrams::inputs`]).
    pub(crate) fn view_and_inputs(
        &mut self,
        path: &Path,
        dom: &Dom,
        formulas: bool,
    ) -> (ViewTree, BTreeSet<String>, BTreeSet<PathBuf>) {
        let diagrams = self.diagrams();
        let resolver = EngineResolver {
            workspace: &self.workspace,
            diagrams: &diagrams,
            failed: &self.rendered.failed,
            formulas,
            wanted: RefCell::default(),
            inputs: RefCell::default(),
            directory: path.parent().map(Path::to_path_buf),
        };
        let view = build_view(path, dom, &resolver);
        let inputs = resolver.inputs.into_inner();
        let mut waiting = BTreeSet::new();
        for (checksum, request) in resolver.wanted.into_inner() {
            waiting.insert(checksum.clone());
            self.spawn_diagram(&diagrams, checksum, request);
        }
        (view, waiting, inputs)
    }

    /// Builds the view from the cached parse, starts any diagram it is missing, and publishes.
    fn publish_preview(&mut self, id: DocumentId) {
        let Some(parsed) = self.rendered.parsed.get(&id) else {
            return;
        };
        let (path, dom) = (parsed.path.clone(), parsed.dom.clone());
        let (view, waiting, inputs) = self.view_and_inputs(&path, &dom, true);
        let Some(parsed) = self.rendered.parsed.get(&id) else {
            return;
        };
        let preview = Arc::new(RenderedPreview {
            document: id,
            version: parsed.version,
            path: parsed.path.clone(),
            view,
        });
        if let Some(parsed) = self.rendered.parsed.get_mut(&id) {
            parsed.waiting = waiting;
            parsed.sources.extend(inputs);
        }
        let version = preview.version;
        self.rendered.published.insert(id, preview);
        self.emit(EngineEvent::PreviewReady { id, version });
        #[cfg(feature = "typst")]
        self.request_pdf_preview(id);
    }

    /// The diagram renderer, cached under the first workspace root.
    fn diagrams(&mut self) -> Arc<Diagrams> {
        self.rendered
            .diagrams
            .get_or_insert_with(|| {
                let cache = match self.roots.first() {
                    Some(root) => DiagramCache::for_workspace(root),
                    None => DiagramCache::new(std::env::temp_dir().join("adocfoundry-diagrams")),
                };
                // External tools read files of their own (PlantUML's `!include`, Graphviz's
                // `image`), so a confined engine renders Mermaid only.
                let diagrams = match self.file_access {
                    FileAccess::Any => Diagrams::with_tools(cache, &diagram_tools(&self.settings)),
                    FileAccess::Within(_) => Diagrams::in_process(cache),
                };
                // Without Typst, formulas stay as styled source.
                #[cfg(feature = "typst")]
                let diagrams = diagrams.with_provider(Box::new(crate::StemProvider));
                Arc::new(diagrams)
            })
            .clone()
    }

    /// Chooses the parser previews are built with.
    pub fn set_document_parser(&mut self, parser: Arc<dyn DocumentParser>) {
        self.rendered.parser = Some(parser);
        self.parses_changed(None);
        self.preview_workspace_changed();
    }

    /// Confines every file the engine reads on the front end's behalf (opening, includes, and
    /// the images, themes and fonts of an export) to `access`. External diagram tools, which
    /// read files the engine cannot see, are not run unless access is [`FileAccess::Any`]. The
    /// MCP server confines the engine to its roots; the default reads anything.
    pub fn set_file_access(&mut self, access: FileAccess) {
        self.file_access = access;
        self.rendered.diagrams = None;
        self.preview_workspace_changed();
    }

    #[must_use]
    pub fn file_access(&self) -> &FileAccess {
        &self.file_access
    }

    /// Builds the diagram renderer afresh when the settings now name other diagram tools, and
    /// previews again, retrying the diagrams that failed.
    pub(crate) fn diagram_tools_changed(&mut self, before: &crate::settings::ToolSettings) {
        let now = &self.settings.tools;
        let tools = |tools: &crate::settings::ToolSettings| {
            (
                tools.dot.clone(),
                tools.plantuml.clone(),
                tools.d2.clone(),
                tools.diagram_timeout,
            )
        };
        if tools(before) != tools(now) {
            self.rendered.diagrams = None;
            self.retry_diagrams();
        }
    }

    /// Forgets every diagram that failed, has the renderer look for its tools again, so one
    /// installed since is used, and shows the previews and visual views again, rendering those
    /// diagrams afresh: `preview.retry_diagrams`.
    pub fn retry_diagrams(&mut self) {
        self.rendered.failed.clear();
        if let Some(diagrams) = &self.rendered.diagrams {
            diagrams.look_for_tools_again();
        }
        self.preview_workspace_changed();
        self.refresh_visual_views();
    }

    /// Forgets the oldest diagram failures beyond [`MAX_DIAGRAM_FAILURES`]. Called only where
    /// a page has been parsed afresh, before its view is built, never as a view is rebuilt for
    /// a diagram that finished, which would ask again for a failure just forgotten, and again.
    pub(crate) fn trim_diagram_failures(&mut self) {
        self.rendered.failed.trim();
    }

    /// How many diagram failures are remembered: at most [`MAX_DIAGRAM_FAILURES`] and those
    /// of the pages shown since they were last parsed.
    #[must_use]
    pub fn diagram_failures(&self) -> usize {
        self.rendered.failed.len()
    }

    /// The workspace's diagram cache: under the first root, where the renderer keeps it.
    pub(crate) fn diagram_cache(&self) -> Option<DiagramCache> {
        self.roots
            .first()
            .map(|root| DiagramCache::for_workspace(root))
    }

    /// Removes, on a worker, the images in the workspace's diagram cache not read for
    /// `[history] diagram_cache_days`, keeping every one the renderer has shown (#431).
    pub(crate) fn prune_diagram_cache(&mut self) {
        let Some(cache) = self.diagram_cache().filter(|cache| cache.dir().is_dir()) else {
            return;
        };
        let keep = self
            .rendered
            .diagrams
            .as_ref()
            .map(|diagrams| diagrams.in_use())
            .unwrap_or_default();
        let unused_for =
            Duration::from_secs(u64::from(self.settings.history.diagram_cache_days) * 86_400);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = |_| WorkerMessage::DiagramCachePruned;
        worker::spawn(sender, crashed, move |sender| {
            let _ = cache.prune(unused_for, &keep);
            let _ = sender.send(WorkerMessage::DiagramCachePruned);
        });
    }

    /// Replaces the diagram renderer, for tests and front ends with their own providers.
    pub fn set_diagrams(&mut self, diagrams: Diagrams) {
        self.rendered.diagrams = Some(Arc::new(diagrams));
    }

    /// Renders `request` on a worker, or queues it while all [`DIAGRAM_WORKERS`] are busy.
    fn spawn_diagram(
        &mut self,
        diagrams: &Arc<Diagrams>,
        checksum: String,
        request: DiagramRequest,
    ) {
        if !self.rendered.rendering.insert(checksum.clone()) {
            return;
        }
        self.rendered
            .queued
            .push_back((diagrams.clone(), checksum, request));
        self.start_queued_diagrams();
    }

    fn start_queued_diagrams(&mut self) {
        while self.rendered.running < DIAGRAM_WORKERS {
            let Some((diagrams, checksum, request)) = self.rendered.queued.pop_front() else {
                return;
            };
            let sender = self.worker_sender.clone();
            self.rendered.running += 1;
            self.workers_running += 1;
            let failed = checksum.clone();
            let crashed = move |error| WorkerMessage::DiagramRendered {
                checksum: failed,
                result: Err(format!("the diagram renderer crashed: {error}")),
            };
            worker::spawn(sender, crashed, move |sender| {
                let result = diagrams
                    .render_at(&checksum, &request)
                    .map_err(|error| error.to_string());
                let _ = sender.send(WorkerMessage::DiagramRendered { checksum, result });
            });
        }
    }

    pub(crate) fn diagram_rendered(&mut self, checksum: &str, result: Result<PathBuf, String>) {
        self.rendered.rendering.remove(checksum);
        self.rendered.running = self.rendered.running.saturating_sub(1);
        self.start_queued_diagrams();
        if let Err(message) = result {
            self.rendered.failed.insert(checksum.to_owned(), message);
        }
        let waiting: Vec<DocumentId> = self
            .rendered
            .parsed
            .iter()
            .filter(|(_, parsed)| parsed.waiting.contains(checksum))
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            self.publish_preview(id);
        }
        self.visual_diagram_rendered(checksum);
        #[cfg(feature = "typst")]
        self.export_diagram_rendered(checksum);
    }
}

/// Resolves the view's references, images and diagrams against the workspace.
struct EngineResolver<'e> {
    workspace: &'e WorkspaceState,
    diagrams: &'e Diagrams,
    failed: &'e FailedDiagrams,
    /// Whether formulas are rendered as images for this view.
    formulas: bool,
    /// Diagrams to render, gathered while building because the view is built immutably.
    wanted: RefCell<Vec<(String, DiagramRequest)>>,
    /// The files the view's diagrams read.
    inputs: RefCell<BTreeSet<PathBuf>>,
    /// The folder of the page the view is of, where its diagrams' tools run.
    directory: Option<PathBuf>,
}

impl ViewResolver for EngineResolver<'_> {
    fn xref(&self, from: &Path, target: &str) -> ResolvedLink {
        let (file, anchor) = target
            .split_once('#')
            .map_or((target, None), |(file, anchor)| (file, Some(anchor)));
        let index = &self.workspace.index;
        let Some(path) =
            adoc_lang::definition::reference_target_path(index, &self.workspace.antora, from, file)
        else {
            return ResolvedLink {
                unresolved: true,
                ..ResolvedLink::default()
            };
        };
        let document = index.file(&path).map(|file| &file.document);
        let anchor = anchor.filter(|anchor| !anchor.is_empty());
        let location = anchor
            .and_then(|anchor| index.resolve_anchor(&path, anchor))
            .map(|location| Location {
                path: location.path.to_path_buf(),
                range: location.range,
            })
            .unwrap_or_else(|| Location {
                path: path.clone(),
                range: document
                    .and_then(|document| document.title.as_ref())
                    .map_or(SourceRange::new(0, 0), |title| title.range),
            });
        let title = match anchor {
            Some(anchor) => document.and_then(|document| {
                document
                    .sections
                    .iter()
                    .find(|section| section.id.as_deref() == Some(anchor))
                    .map(|section| section.title.clone())
            }),
            None => document
                .and_then(|document| document.title.as_ref())
                .map(|title| title.text.clone()),
        };
        ResolvedLink {
            target: Some(location),
            fragment: anchor.map(str::to_owned),
            title,
            unresolved: false,
        }
    }

    fn image(&self, from: &Path, target: &str, imagesdir: Option<&str>) -> Option<PathBuf> {
        let antora = &self.workspace.antora;
        if let Some(context) = antora.context_for_path(from) {
            let mut id = adoc_lang::images::antora_image_id(target).ok()??;
            id.family.get_or_insert(ResourceFamily::Image);
            return AntoraResolver::resolve(antora, &id, &context)
                .ok()
                .map(|resource| resource.source_path.clone());
        }
        let base = from.parent().unwrap_or_else(|| Path::new(""));
        let path = normalize_path(&match imagesdir {
            Some(dir) => base.join(dir).join(target),
            None => base.join(target),
        });
        (path.is_file() || self.workspace.index.file(&path).is_some()).then_some(path)
    }

    fn diagram(
        &self,
        kind: &str,
        source: &str,
        attributes: &BTreeMap<String, String>,
    ) -> DiagramView {
        let request = DiagramRequest {
            kind: kind.to_owned(),
            source: source.to_owned(),
            attributes: attributes.clone(),
            directory: self.directory.clone(),
        };
        self.inputs
            .borrow_mut()
            .extend(self.diagrams.inputs(&request));
        let checksum = self.diagrams.checksum(&request);
        if let Some(path) = self.diagrams.cached_at(&checksum) {
            return DiagramView::Svg(path);
        }
        if !self.formulas && kind.starts_with("stem-") {
            return DiagramView::Pending {
                kind: kind.to_owned(),
                source: source.to_owned(),
            };
        }
        let failed = |message: String| DiagramView::Failed {
            kind: kind.to_owned(),
            message,
            source: source.to_owned(),
        };
        if let Some(message) = self.failed.get(&checksum) {
            return failed(message.clone());
        }
        if !self.diagrams.supports(kind) {
            return failed(format!("no renderer for {kind} diagrams"));
        }
        self.wanted.borrow_mut().push((checksum, request));
        DiagramView::Pending {
            kind: kind.to_owned(),
            source: source.to_owned(),
        }
    }
}

/// The diagram tools `settings` names, each looked up on the PATH when its setting is empty.
fn diagram_tools(settings: &crate::Settings) -> ToolPaths {
    let defaults = ToolPaths::default();
    let or = |configured: &str, default: PathBuf| {
        if configured.is_empty() {
            default
        } else {
            PathBuf::from(configured)
        }
    };
    ToolPaths {
        dot: or(&settings.tools.dot, defaults.dot),
        plantuml: or(&settings.tools.plantuml, defaults.plantuml),
        d2: or(&settings.tools.d2, defaults.d2),
        timeout: Duration::from_secs(settings.tools.diagram_timeout),
    }
}
