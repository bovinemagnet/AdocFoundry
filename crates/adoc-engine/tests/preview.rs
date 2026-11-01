//! Headless tests of the rendered preview service.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc::Receiver, Arc},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_diagrams::{DiagramCache, Diagrams, MermaidProvider};
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, EngineEvent, ManualClock, Settings,
    PREVIEW_SETTLE,
};
use adoc_render::{DiagramView, ViewBlock, ViewInline, ViewKind};

const WAIT: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn engine_with_clock() -> (Engine, ManualClock) {
    let clock = ManualClock::new();
    let engine = Engine::with_clock(
        Settings::load(None, None).unwrap().with_git_opt_in(),
        Arc::new(clock.clone()),
    );
    (engine, clock)
}

/// Every block of the preview, depth first.
fn blocks(engine: &Engine, id: DocumentId) -> Vec<ViewBlock> {
    fn walk(blocks: Vec<&ViewBlock>, out: &mut Vec<ViewBlock>) {
        for block in blocks {
            out.push(block.clone());
            walk(block.child_refs(), out);
        }
    }
    let preview = engine.rendered_preview(id).expect("a preview");
    let mut out = Vec::new();
    walk(preview.view.blocks.iter().collect(), &mut out);
    out
}

fn paragraphs(engine: &Engine, id: DocumentId) -> Vec<String> {
    blocks(engine, id)
        .into_iter()
        .filter_map(|block| match block.kind {
            ViewKind::Paragraph(content) => Some(ViewInline::plain_text(&content)),
            _ => None,
        })
        .collect()
}

fn ready_versions(events: &Receiver<EngineEvent>, id: DocumentId) -> Vec<u64> {
    events
        .try_iter()
        .filter_map(|event| match event {
            EngineEvent::PreviewReady { id: ready, version } if ready == id => Some(version),
            _ => None,
        })
        .collect()
}

fn type_text(engine: &mut Engine, id: DocumentId, offset: usize, text: &str) {
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(offset, offset),
                text: text.to_owned(),
            },
        )
        .unwrap();
}

#[test]
fn preview_updates_after_settle() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nFirst.\n").unwrap();
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();

    engine.request_preview(id);
    assert!(engine.has_pending());
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(ready_versions(&events, id), [0]);
    assert_eq!(paragraphs(&engine, id), ["First."]);
    let title = engine.rendered_preview(id).unwrap().view.title.clone();
    assert_eq!(
        title.as_deref().map(ViewInline::plain_text).as_deref(),
        Some("Page")
    );

    let end = "= Page\n\nFirst".len();
    type_text(&mut engine, id, end, " Second");
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert!(
        ready_versions(&events, id).is_empty(),
        "nothing before the pause"
    );
    assert_eq!(paragraphs(&engine, id), ["First."]);

    clock.advance(PREVIEW_SETTLE - Duration::from_millis(1));
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert!(ready_versions(&events, id).is_empty());

    clock.advance(Duration::from_millis(1));
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(ready_versions(&events, id), [1]);
    assert_eq!(paragraphs(&engine, id), ["First Second."]);
    assert!(!engine.has_pending());

    // Once released, nothing more is rendered for it.
    engine.release_preview(id);
    assert!(engine.rendered_preview(id).is_none());
    type_text(&mut engine, id, 0, "x");
    clock.advance(PREVIEW_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert!(ready_versions(&events, id).is_empty());
}

#[test]
fn preview_follows_included_file_changes() {
    // An Antora partial resolves through the catalog, and its blocks name the partial.
    let root = fixture("antora-single-component");
    let (mut engine, clock) = engine_with_clock();
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let page = root.join("modules/security/pages/authentication.adoc");
    let id = engine.open(&page).unwrap();
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    let note = blocks(&engine, id)
        .into_iter()
        .find(|block| matches!(block.kind, ViewKind::Admonition { .. }))
        .expect("the partial's note");
    assert!(note
        .span
        .path
        .ends_with("modules/security/partials/token-note.adoc"));

    // An edit to an open partial re-renders the page that includes it, from the buffer.
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    let part = directory.path().join("part.adoc");
    fs::write(&page, "= Page\n\ninclude::part.adoc[]\n").unwrap();
    fs::write(&part, "From the part.\n").unwrap();
    let events = engine.subscribe();
    let page_id = engine.open(&page).unwrap();
    engine.request_preview(page_id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(paragraphs(&engine, page_id), ["From the part."]);
    let _ = ready_versions(&events, page_id);

    let part_id = engine.open(&part).unwrap();
    type_text(&mut engine, part_id, "From the part".len(), ", edited");
    clock.advance(PREVIEW_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(
        ready_versions(&events, page_id),
        [0],
        "the page itself is unchanged"
    );
    assert_eq!(paragraphs(&engine, page_id), ["From the part, edited."]);
    assert_eq!(
        fs::read_to_string(&part).unwrap(),
        "From the part.\n",
        "nothing was saved"
    );
}

#[test]
fn stale_preview_results_are_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(&page, "= Page\n\nOld.\n").unwrap();
    let (mut engine, clock) = engine_with_clock();
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();

    engine.request_preview(id);
    engine.pump(); // Starts rendering version 0.
    type_text(&mut engine, id, "= Page\n\n".len(), "New ");
    // The version 0 result arrives after the edit: it is dropped, not shown.
    assert!(engine.wait_for_workers(WAIT));
    assert!(ready_versions(&events, id).is_empty());
    assert!(engine.rendered_preview(id).is_none());

    clock.advance(PREVIEW_SETTLE);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(ready_versions(&events, id), [1]);
    assert_eq!(paragraphs(&engine, id), ["New Old."]);
}

#[test]
fn diagram_completion_republishes_preview() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n\n[mermaid]\n----\ngraph TD\n  A --> B\n----\n\n[ditaa]\n....\n+--+\n....\n",
    )
    .unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let cache = DiagramCache::new(directory.path().join("cache"));
    engine.set_diagrams(Diagrams::with_providers(
        cache,
        vec![Box::new(MermaidProvider::default())],
    ));
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();

    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(
        ready_versions(&events, id),
        [0, 0],
        "published with the diagram pending, then again once it rendered"
    );
    let diagrams: Vec<DiagramView> = blocks(&engine, id)
        .into_iter()
        .filter_map(|block| match block.kind {
            ViewKind::Diagram(diagram) => Some(diagram),
            _ => None,
        })
        .collect();
    let [DiagramView::Svg(svg), DiagramView::Failed { message, .. }] = diagrams.as_slice() else {
        panic!("{diagrams:?}");
    };
    assert!(fs::read_to_string(svg).unwrap().contains("<svg"));
    assert!(svg.starts_with(directory.path().join("cache")));
    assert!(message.contains("ditaa"), "{message}");

    // The cached image is used at once next time: one publication, no second render.
    engine.release_preview(id);
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(ready_versions(&events, id), [0]);
}

#[test]
fn preview_commands_follow_and_pin() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.adoc");
    let second = directory.path().join("second.adoc");
    fs::write(&first, "= First\n").unwrap();
    fs::write(&second, "= Second\n").unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let events = engine.subscribe();
    let first = engine.open(&first).unwrap();
    assert_eq!(engine.preview_target(), None);

    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert!(engine.preview_layout().visible);
    assert_eq!(engine.preview_target(), Some(first));
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert!(engine.rendered_preview(first).is_some());

    engine.invoke(ids::PREVIEW_PIN, CommandArgs::None).unwrap();
    let second = engine.open(&second).unwrap();
    assert_eq!(engine.preview_target(), Some(first), "pinned");
    engine.invoke(ids::PREVIEW_PIN, CommandArgs::None).unwrap();
    assert_eq!(engine.preview_target(), Some(second), "following again");
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert!(engine.rendered_preview(second).is_some());
    assert!(
        engine.rendered_preview(first).is_none(),
        "the old target is released"
    );

    engine
        .invoke(ids::VIEW_DETACH_PREVIEW, CommandArgs::None)
        .unwrap();
    assert!(engine.preview_layout().detached);
    engine
        .invoke(ids::VIEW_DETACH_PREVIEW, CommandArgs::None)
        .unwrap();
    assert!(!engine.preview_layout().detached);
    engine
        .invoke(ids::VIEW_TOGGLE_SCROLL_SYNC, CommandArgs::None)
        .unwrap();
    assert!(!engine.preview_layout().scroll_sync);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::PreviewLayoutChanged));

    engine
        .invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.preview_target(), None);
    assert!(engine.rendered_preview(second).is_none());
}

#[test]
fn go_to_records_history_and_reveal_does_not() {
    use adoc_engine::NavigationPosition;

    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    let other = directory.path().join("other.adoc");
    fs::write(&page, "= Page\n").unwrap();
    fs::write(&other, "= Other\n\n== Part\n").unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let page_id = engine.open(&page).unwrap();

    engine
        .invoke(
            ids::NAVIGATE_REVEAL,
            CommandArgs::Position(NavigationPosition {
                path: other.clone(),
                offset: 10,
            }),
        )
        .unwrap();
    let other_id = engine.document_for_path(&other).unwrap();
    assert_eq!(engine.active(), Some(other_id));
    assert_eq!(engine.navigation_position().unwrap().offset, 10);
    assert!(!engine.can_go_back());

    engine.set_active(page_id);
    engine
        .invoke(
            ids::NAVIGATE_GO_TO,
            CommandArgs::Position(NavigationPosition {
                path: other.clone(),
                offset: 9,
            }),
        )
        .unwrap();
    assert_eq!(engine.active(), Some(other_id));
    assert!(engine.can_go_back());
    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.active(), Some(page_id));
}

#[test]
fn going_back_returns_to_the_caret_through_later_edits() {
    use adoc_engine::NavigationPosition;

    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    let other = directory.path().join("other.adoc");
    fs::write(&page, "= Page\n\nSome text.\n").unwrap();
    fs::write(&other, "= Other\n").unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let page_id = engine.open(&page).unwrap();
    let caret = "= Page\n\nSome".len();
    engine.caret_moved(page_id, caret);

    engine
        .invoke(
            ids::NAVIGATE_GO_TO,
            CommandArgs::Position(NavigationPosition {
                path: other.clone(),
                offset: 0,
            }),
        )
        .unwrap();
    // The page is edited above where the caret was, while it is not shown.
    type_text(&mut engine, page_id, "= Page\n".len(), "Intro.\n");
    engine
        .invoke(ids::NAVIGATE_BACK, CommandArgs::None)
        .unwrap();
    assert_eq!(engine.active(), Some(page_id));
    assert_eq!(
        engine.navigation_position().unwrap().offset,
        caret + "Intro.\n".len()
    );
}

#[test]
fn render_blocking_waits_for_diagrams() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n\n[mermaid]\n----\ngraph LR\n  X --> Y\n----\n",
    )
    .unwrap();
    let (mut engine, _clock) = engine_with_clock();
    engine.set_diagrams(Diagrams::with_providers(
        DiagramCache::new(directory.path().join("cache")),
        vec![Box::new(MermaidProvider::default())],
    ));
    let id = engine.open(&page).unwrap();
    let preview = engine.render_blocking(id, WAIT).unwrap();
    assert!(matches!(
        &preview.view.blocks[0].kind,
        ViewKind::Diagram(DiagramView::Svg(_))
    ));
    assert!(engine.rendered_preview(id).is_none(), "released afterwards");
}

#[test]
fn stem_renders_to_svg_and_is_cached() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n:stem:\n\n[stem]\n++++\nsum_(i=1)^n i\n++++\n\nInline stem:[x^2] here.\n",
    )
    .unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let cache = DiagramCache::new(directory.path().join("cache"));
    engine.set_diagrams(
        Diagrams::with_providers(cache, Vec::new())
            .with_provider(Box::new(adoc_engine::StemProvider)),
    );
    let events = engine.subscribe();
    let id = engine.open(&page).unwrap();
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    // Pending first, then once for each formula that finishes: the two may finish together or
    // apart, depending on the workers' timing.
    let versions = ready_versions(&events, id);
    assert!(
        (2..=3).contains(&versions.len()) && versions.iter().all(|version| *version == 0),
        "pending, then rendered: {versions:?}"
    );

    let blocks = blocks(&engine, id);
    let rendered = blocks
        .iter()
        .find_map(|block| match &block.kind {
            ViewKind::Stem { rendered, .. } => Some(rendered.clone()),
            _ => None,
        })
        .expect("the formula block");
    let DiagramView::Svg(svg) = rendered else {
        panic!("{rendered:?}");
    };
    assert!(svg.starts_with(directory.path().join("cache")));
    assert!(fs::read_to_string(&svg).unwrap().starts_with("<svg"));
    let inline = blocks.iter().find_map(|block| match &block.kind {
        ViewKind::Paragraph(content) => content.iter().find_map(|inline| match inline {
            ViewInline::Stem { rendered, .. } => Some(rendered.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert!(matches!(inline, Some(Some(_))), "{inline:?}");

    // Cached: shown at once next time, with no second render.
    engine.release_preview(id);
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(ready_versions(&events, id), [0]);
}

#[test]
fn inline_stem_carries_its_metrics() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n:stem:\n\nThe sum stem:[sum_(i=1)^n i] here.\n",
    )
    .unwrap();
    let inline_formula = |engine: &Engine, id| {
        blocks(engine, id)
            .iter()
            .find_map(|block| match &block.kind {
                ViewKind::Paragraph(content) => content.iter().find_map(|inline| match inline {
                    ViewInline::Stem { rendered, .. } => rendered.clone(),
                    _ => None,
                }),
                _ => None,
            })
    };
    let rendered = || {
        let (mut engine, _clock) = engine_with_clock();
        engine.set_diagrams(
            Diagrams::with_providers(
                DiagramCache::new(directory.path().join("cache")),
                Vec::new(),
            )
            .with_provider(Box::new(adoc_engine::StemProvider)),
        );
        let id = engine.open(&page).unwrap();
        engine.request_preview(id);
        engine.pump();
        assert!(engine.wait_for_workers(WAIT));
        inline_formula(&engine, id).expect("a rendered inline formula")
    };

    let image = rendered();
    assert!(image.width > 0.0 && image.baseline > 0.0 && image.baseline < image.height);
    let svg = fs::read_to_string(&image.path).unwrap();
    let height = format!("height=\"{}pt\"", image.height);
    assert!(
        svg.contains(&height),
        "{height} in {}",
        &svg[..svg.len().min(300)]
    );
    // From the cache, a second engine finds the same measures without typesetting again.
    assert_eq!(rendered(), image);
}

#[test]
fn asciidoctor_is_the_configured_one_when_set() {
    let directory = tempfile::tempdir().unwrap();
    let tool = directory.path().join("asciidoctor");
    std::fs::write(&tool, "").unwrap();
    let mut settings = Settings::default();
    settings.tools.asciidoctor = tool.display().to_string();
    assert_eq!(Engine::new(settings).asciidoctor_executable(), Some(tool));

    let mut settings = Settings::default();
    settings.tools.asciidoctor = directory.path().join("missing").display().to_string();
    assert_eq!(Engine::new(settings).asciidoctor_executable(), None);
}

#[test]
fn confined_access_reads_no_include_outside_the_roots() {
    let directory = tempfile::tempdir().unwrap();
    let outside = adoc_engine::normalize_path(directory.path());
    let root = outside.join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(outside.join("outside.adoc"), "OUTSIDE-MARKER\n").unwrap();
    std::fs::write(root.join("inside.adoc"), "Inside text.\n").unwrap();
    let page = root.join("page.adoc");
    std::fs::write(
        &page,
        "= Page\n\ninclude::inside.adoc[]\n\ninclude::../outside.adoc[]\n",
    )
    .unwrap();
    let rendered = |access: adoc_engine::FileAccess| {
        let mut engine = Engine::new(Settings::default());
        engine.set_file_access(access);
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(WAIT));
        let id = engine.open(&page).unwrap();
        adoc_engine::to_text(&engine.render_blocking(id, WAIT).unwrap().view)
    };

    // The desktop app, the CLI and the language server read what their user can.
    let any = rendered(adoc_engine::FileAccess::Any);
    assert!(
        any.contains("Inside text.") && any.contains("OUTSIDE-MARKER"),
        "{any}"
    );

    let confined = rendered(adoc_engine::FileAccess::Within(vec![root.clone()]));
    assert!(confined.contains("Inside text."), "{confined}");
    assert!(!confined.contains("OUTSIDE-MARKER"), "{confined}");

    let mut engine = Engine::new(Settings::default());
    engine.set_file_access(adoc_engine::FileAccess::Within(vec![root.clone()]));
    assert!(engine.open(&outside.join("outside.adoc")).is_err());
}

#[test]
fn a_configured_diagram_tool_is_used_and_follows_the_settings() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    fs::write(
        &page,
        "= Page\n\n[graphviz]\n....\ndigraph { a -> b }\n....\n",
    )
    .unwrap();
    let user = directory.path().join("adocfoundry.toml");
    let configure = |dot: &std::path::Path| {
        fs::write(
            &user,
            format!("[tools]\ndot = {:?}\n", dot.display().to_string()),
        )
        .unwrap();
    };
    // Paths that do not exist, so no process is ever started.
    let first = directory.path().join("first").join("dot");
    let second = directory.path().join("second").join("dot");
    configure(&first);
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap().with_git_opt_in());
    let id = engine.open(&page).unwrap();
    let message = |engine: &mut Engine| {
        engine.request_preview(id);
        engine.pump();
        assert!(engine.wait_for_workers(WAIT));
        engine.pump();
        let message = blocks(engine, id)
            .into_iter()
            .find_map(|block| match block.kind {
                ViewKind::Diagram(DiagramView::Failed { message, .. }) => Some(message),
                _ => None,
            });
        engine.release_preview(id);
        message.expect("the diagram fails with a reason")
    };

    let reason = message(&mut engine);
    assert!(reason.contains(&first.display().to_string()), "{reason}");

    configure(&second);
    engine.reload_settings().unwrap();
    let reason = message(&mut engine);
    assert!(reason.contains(&second.display().to_string()), "{reason}");
}

/// A renderer that records how many renders run at once.
struct Concurrency {
    running: Arc<std::sync::atomic::AtomicUsize>,
    most: Arc<std::sync::atomic::AtomicUsize>,
    renders: Arc<std::sync::atomic::AtomicUsize>,
}

impl adoc_diagrams::DiagramProvider for Concurrency {
    fn kinds(&self) -> &[&str] {
        &["graphviz"]
    }

    fn render(
        &self,
        request: &adoc_diagrams::DiagramRequest,
    ) -> Result<String, adoc_diagrams::DiagramError> {
        use std::sync::atomic::Ordering;
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        self.running.fetch_sub(1, Ordering::SeqCst);
        self.renders.fetch_add(1, Ordering::SeqCst);
        Ok(format!("<svg>{}</svg>", request.source))
    }
}

#[test]
fn many_diagrams_render_through_a_bounded_pool() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    let source: String = (0..50)
        .map(|number| format!("[graphviz]\n....\ndigraph {{ a{number} }}\n....\n\n"))
        .collect();
    fs::write(&page, format!("= Page\n\n{source}")).unwrap();
    let (mut engine, _clock) = engine_with_clock();
    let most = Arc::new(AtomicUsize::new(0));
    let renders = Arc::new(AtomicUsize::new(0));
    engine.set_diagrams(Diagrams::with_providers(
        DiagramCache::new(directory.path().join("cache")),
        vec![Box::new(Concurrency {
            running: Arc::new(AtomicUsize::new(0)),
            most: most.clone(),
            renders: renders.clone(),
        })],
    ));
    let id = engine.open(&page).unwrap();
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    engine.pump();

    assert_eq!(renders.load(Ordering::SeqCst), 50);
    let most = most.load(Ordering::SeqCst);
    assert!(
        (1..=adoc_engine::DIAGRAM_WORKERS).contains(&most),
        "{most} at once"
    );
    let svgs = blocks(&engine, id)
        .into_iter()
        .filter(|block| matches!(block.kind, ViewKind::Diagram(DiagramView::Svg(_))))
        .count();
    assert_eq!(svgs, 50);
}

/// A renderer with a bug: every render panics.
struct Panics;

impl adoc_diagrams::DiagramProvider for Panics {
    fn kinds(&self) -> &[&str] {
        &["mermaid"]
    }

    fn render(
        &self,
        _: &adoc_diagrams::DiagramRequest,
    ) -> Result<String, adoc_diagrams::DiagramError> {
        panic!("a bug in the renderer")
    }
}

fn engine_with_panicking_diagrams(directory: &Path) -> (Engine, DocumentId) {
    let page = directory.join("page.adoc");
    fs::write(
        &page,
        "= Page\n\n[mermaid]\n----\ngraph TD\n  A --> B\n----\n",
    )
    .unwrap();
    let (mut engine, _clock) = engine_with_clock();
    engine.set_diagrams(Diagrams::with_providers(
        DiagramCache::new(directory.join("cache")),
        vec![Box::new(Panics)],
    ));
    let id = engine.open(&page).unwrap();
    (engine, id)
}

#[cfg(feature = "typst")]
fn export_page(engine: &mut Engine, id: DocumentId, output: PathBuf) {
    use adoc_engine::{ExportArgs, ExportTarget};

    let summary = engine.export_blocking(
        ExportArgs {
            target: ExportTarget::Page(id),
            output: output.clone(),
            theme: None,
            standards: Vec::new(),
            fonts_dir: None,
            typst: None,
            attributes: Vec::new(),
        },
        WAIT,
    );
    summary.unwrap_or_else(|error| panic!("{error}"));
    assert!(fs::read(&output).unwrap().starts_with(b"%PDF-"));
}

#[test]
fn a_panicking_diagram_provider_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = engine_with_panicking_diagrams(directory.path());
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT), "the worker reported back");
    engine.pump();
    let message = blocks(&engine, id)
        .into_iter()
        .find_map(|block| match block.kind {
            ViewKind::Diagram(DiagramView::Failed { message, .. }) => Some(message),
            _ => None,
        })
        .expect("the diagram failed");
    assert!(message.contains("a bug in the renderer"), "{message}");
    assert!(!engine.has_pending());

    #[cfg(feature = "typst")]
    export_page(&mut engine, id, directory.path().join("page.pdf"));
}

#[cfg(feature = "typst")]
#[test]
fn an_export_whose_diagram_renderer_panics_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = engine_with_panicking_diagrams(directory.path());
    export_page(&mut engine, id, directory.path().join("page.pdf"));
    assert!(!engine.has_pending());
}
