//! The rendered preview: a view tree for each document a front end shows, rebuilt on a worker
//! once edits to it, or to any file it includes, pause for [`PREVIEW_SETTLE`].

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use adoc_antora::{AntoraResolver, ResourceFamily};
use adoc_core::{Location, SourceRange};
use adoc_diagrams::{DiagramCache, DiagramRequest, Diagrams};
use adoc_dom::{AcdcParser, DocumentParser, Dom, IncludeResolver, ParseInput};
use adoc_index::normalize_path;
use adoc_lang::WorkspaceState;
use adoc_render::{build_view, DiagramView, ResolvedLink, ViewResolver, ViewTree};

use crate::{workspace::WorkerMessage, DocumentId, Engine, EngineEvent};

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
}

impl Default for PreviewLayout {
    fn default() -> Self {
        Self {
            visible: false,
            detached: false,
            scroll_sync: true,
            pinned: None,
        }
    }
}

/// A parsed document kept so a finished diagram can rebuild the view without re-parsing.
struct Parsed {
    version: u64,
    path: PathBuf,
    dom: Arc<Dom>,
    /// Every file the page was assembled from, itself included.
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
    failed: BTreeMap<String, String>,
}

impl PreviewState {
    pub(crate) fn has_scheduled(&self) -> bool {
        !self.scheduled.is_empty()
    }
}

/// The worker's input: everything it reads, copied, so the engine stays free to change.
struct Snapshot {
    parser: Arc<dyn DocumentParser>,
    id: DocumentId,
    generation: u64,
    version: u64,
    path: PathBuf,
    text: String,
    attributes: Vec<(String, String)>,
    includes: SnapshotIncludes,
}

/// A parse's input, owned so it can move to a worker.
pub(crate) struct OwnedInput {
    pub(crate) version: u64,
    pub(crate) path: PathBuf,
    pub(crate) text: String,
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
    buffers: HashMap<PathBuf, String>,
    read: Mutex<BTreeSet<PathBuf>>,
}

impl IncludeResolver for SnapshotIncludes {
    fn read(&self, requested: &Path) -> Option<(PathBuf, String)> {
        let requested = normalize_path(requested);
        let path = self.map.get(&requested).cloned().unwrap_or(requested);
        let text = match self.buffers.get(&path) {
            Some(text) => text.clone(),
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

    /// Renders `id` and waits for the result, diagrams included, for front ends without an
    /// event loop such as the CLI. `None` when it does not finish within `timeout`.
    pub fn render_blocking(
        &mut self,
        id: DocumentId,
        timeout: Duration,
    ) -> Option<Arc<RenderedPreview>> {
        self.request_preview(id);
        // Rendered now, not after the settle time.
        let now = self.clock.now();
        self.rendered.scheduled.insert(id, now);
        self.start_due_previews();
        let finished = self.wait_for_workers(timeout);
        let preview = self.rendered_preview(id);
        self.release_preview(id);
        preview.filter(|_| finished)
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
        layout
            .pinned
            .filter(|id| self.documents.contains_key(id))
            .or(self.active)
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
            if let Some(snapshot) = self.preview_snapshot(*id) {
                self.spawn_preview(snapshot);
            }
        }
        due.len()
    }

    fn preview_snapshot(&mut self, id: DocumentId) -> Option<Snapshot> {
        let input = self.parse_input(id)?;
        self.rendered.next_generation += 1;
        let generation = self.rendered.next_generation;
        self.rendered.latest.insert(id, generation);
        Some(Snapshot {
            parser: self.document_parser(),
            id,
            generation,
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
            document.text.to_string(),
            document.version,
            &document.parsed,
        ))
    }

    /// The parse input of the page at `path`: its open buffer when it is open, else the file.
    pub(crate) fn parse_input_for_path(&self, path: &Path) -> Option<OwnedInput> {
        if let Some(id) = self.document_for_path(path) {
            return self.parse_input(id);
        }
        let text = fs::read_to_string(path).ok()?;
        let parsed = adoc_parser::parse(&crate::document::uri_for(path), &text).document;
        Some(self.owned_input(path, text, 0, &parsed))
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
        text: String,
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
            .map(|open| (open.path.clone(), open.text.to_string()))
            .collect();
        OwnedInput {
            version,
            path: path.to_path_buf(),
            text,
            attributes,
            includes: SnapshotIncludes {
                map,
                buffers,
                read: Mutex::default(),
            },
        }
    }

    fn spawn_preview(&mut self, snapshot: Snapshot) {
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let Snapshot {
                parser,
                id,
                generation,
                version,
                path,
                text,
                attributes,
                includes,
            } = snapshot;
            let input = OwnedInput {
                version,
                path,
                text,
                attributes,
                includes,
            };
            let (dom, sources) = input.parse(parser.as_ref());
            let _ = sender.send(WorkerMessage::PreviewParsed {
                id,
                generation,
                version,
                dom: Box::new(dom),
                sources,
            });
        });
    }

    pub(crate) fn preview_parsed(
        &mut self,
        id: DocumentId,
        generation: u64,
        version: u64,
        dom: Dom,
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
                dom: Arc::new(dom),
                sources,
                waiting: BTreeSet::new(),
            },
        );
        self.publish_preview(id);
    }

    /// The view of `dom`, with the diagrams it still needs, which are started.
    pub(crate) fn view_of(&mut self, path: &Path, dom: &Dom) -> (ViewTree, Vec<DiagramRequest>) {
        let diagrams = self.diagrams();
        let resolver = EngineResolver {
            workspace: &self.workspace,
            diagrams: &diagrams,
            failed: &self.rendered.failed,
            wanted: RefCell::default(),
        };
        let view = build_view(path, dom, &resolver);
        let wanted = resolver.wanted.into_inner();
        for request in &wanted {
            self.spawn_diagram(&diagrams, request.clone());
        }
        (view, wanted)
    }

    /// Builds the view from the cached parse, starts any diagram it is missing, and publishes.
    fn publish_preview(&mut self, id: DocumentId) {
        let Some(parsed) = self.rendered.parsed.get(&id) else {
            return;
        };
        let (path, dom) = (parsed.path.clone(), parsed.dom.clone());
        let (view, wanted) = self.view_of(&path, &dom);
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
            parsed.waiting = wanted.iter().map(DiagramRequest::checksum).collect();
        }
        let version = preview.version;
        self.rendered.published.insert(id, preview);
        self.emit(EngineEvent::PreviewReady { id, version });
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
                Arc::new(Diagrams::new(cache))
            })
            .clone()
    }

    /// Chooses the parser previews are built with.
    pub fn set_document_parser(&mut self, parser: Arc<dyn DocumentParser>) {
        self.rendered.parser = Some(parser);
        self.preview_workspace_changed();
    }

    /// Replaces the diagram renderer, for tests and front ends with their own providers.
    pub fn set_diagrams(&mut self, diagrams: Diagrams) {
        self.rendered.diagrams = Some(Arc::new(diagrams));
    }

    fn spawn_diagram(&mut self, diagrams: &Arc<Diagrams>, request: DiagramRequest) {
        let checksum = request.checksum();
        if !self.rendered.rendering.insert(checksum.clone()) {
            return;
        }
        let diagrams = diagrams.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let result = diagrams.render(&request).map_err(|error| error.to_string());
            let _ = sender.send(WorkerMessage::DiagramRendered { checksum, result });
        });
    }

    pub(crate) fn diagram_rendered(&mut self, checksum: &str, result: Result<PathBuf, String>) {
        self.rendered.rendering.remove(checksum);
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
        self.export_diagram_rendered(checksum);
    }
}

/// Resolves the view's references, images and diagrams against the workspace.
struct EngineResolver<'e> {
    workspace: &'e WorkspaceState,
    diagrams: &'e Diagrams,
    failed: &'e BTreeMap<String, String>,
    /// Diagrams to render, gathered while building because the view is built immutably.
    wanted: RefCell<Vec<DiagramRequest>>,
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
                path: location.path.clone(),
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
        };
        if let Some(path) = self.diagrams.cached(&request) {
            return DiagramView::Svg(path);
        }
        let failed = |message: String| DiagramView::Failed {
            kind: kind.to_owned(),
            message,
            source: source.to_owned(),
        };
        if let Some(message) = self.failed.get(&request.checksum()) {
            return failed(message.clone());
        }
        if !self.diagrams.supports(kind) {
            return failed(format!("no renderer for {kind} diagrams"));
        }
        self.wanted.borrow_mut().push(request);
        DiagramView::Pending {
            kind: kind.to_owned(),
            source: source.to_owned(),
        }
    }
}
