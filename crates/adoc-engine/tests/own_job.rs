//! The blocking helpers wait for their own job only (#425): a worker that has nothing to do
//! with it, held open here by a diagram that never finishes rendering, holds up none of them.

use std::{
    fs,
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex,
    },
    time::{Duration, Instant},
};

use adoc_diagrams::{DiagramCache, Diagrams};
use adoc_engine::{normalize_path, DocumentId, Engine, Settings};

/// How long a helper is given; it should answer well inside it.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How soon a helper should answer while the other worker is held.
const PROMPTLY: Duration = Duration::from_secs(5);

/// A renderer that says it has started, then holds its worker until released (the sender
/// dropped).
struct Blocked {
    started: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl adoc_diagrams::DiagramProvider for Blocked {
    fn kinds(&self) -> &[&str] {
        &["graphviz"]
    }

    fn render(
        &self,
        _: &adoc_diagrams::DiagramRequest,
    ) -> Result<String, adoc_diagrams::DiagramError> {
        let _ = self.started.lock().unwrap().send(());
        let _ = self.release.lock().unwrap().recv();
        Err(adoc_diagrams::DiagramError::Failed("released".to_owned()))
    }
}

/// A workspace with a page drawing a diagram that never finishes while `release` lives, its
/// render already started, and a plain page, `page.adoc`, opened. Kept whole, since dropping
/// `release` ends the render.
struct Held {
    _directory: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    page: DocumentId,
    _release: Sender<()>,
}

fn held() -> Held {
    let directory = tempfile::tempdir().unwrap();
    let root = normalize_path(directory.path());
    fs::write(
        root.join("blocked.adoc"),
        "= Blocked\n\n[graphviz]\n....\ndigraph { a -> b }\n....\n",
    )
    .unwrap();
    fs::write(root.join("page.adoc"), "= Page\n\nA needle in the text.\n").unwrap();
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(TIMEOUT));

    let (started_sender, started) = mpsc::channel();
    let (release, release_receiver) = mpsc::channel();
    engine.set_diagrams(Diagrams::with_providers(
        DiagramCache::new(root.join(".cache")),
        vec![Box::new(Blocked {
            started: Mutex::new(started_sender),
            release: Mutex::new(release_receiver),
        })],
    ));
    let blocked = engine.open(&root.join("blocked.adoc")).unwrap();
    engine.request_preview(blocked);
    let deadline = Instant::now() + TIMEOUT;
    while started.try_recv().is_err() {
        assert!(Instant::now() < deadline, "the diagram never started");
        engine.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    let page = engine.open(&root.join("page.adoc")).unwrap();
    Held {
        _directory: directory,
        root,
        engine,
        page,
        _release: release,
    }
}

#[test]
fn render_blocking_waits_only_for_its_own_page() {
    let mut held = held();
    let (engine, page) = (&mut held.engine, held.page);
    let asked = Instant::now();
    let preview = engine.render_blocking(page, TIMEOUT);
    let took = asked.elapsed();
    let preview = preview.expect("the page rendered");
    assert_eq!(preview.document, page);
    assert!(took < PROMPTLY, "answered after {took:?}");
}

/// A page whose preview is kept for another view is rendered afresh, not answered with the
/// preview it already had.
#[test]
fn render_blocking_renders_afresh() {
    let mut held = held();
    let root = held.root.clone();
    fs::write(root.join("part.adoc"), "Old text.\n").unwrap();
    fs::write(root.join("main.adoc"), "= Main\n\ninclude::part.adoc[]\n").unwrap();
    let engine = &mut held.engine;
    let id = engine.open(&root.join("main.adoc")).unwrap();
    engine.open(&root.join("page.adoc")).unwrap();
    engine.request_preview(id);
    let text = |engine: &mut Engine| {
        adoc_engine::to_text(&engine.render_blocking(id, TIMEOUT).expect("rendered").view)
    };
    assert!(text(engine).contains("Old text."));

    fs::write(root.join("part.adoc"), "New text.\n").unwrap();
    let shown = text(engine);
    assert!(shown.contains("New text."), "{shown}");
}

#[cfg(feature = "typst")]
#[test]
fn export_blocking_waits_only_for_its_own_job() {
    use adoc_engine::{ExportArgs, ExportTarget};

    let mut held = held();
    let (engine, page) = (&mut held.engine, held.page);
    let output = held.root.join("page.pdf");
    let asked = Instant::now();
    let summary = engine.export_blocking(
        ExportArgs {
            target: ExportTarget::Page(page),
            output: output.clone(),
            theme: None,
            standards: Vec::new(),
            fonts_dir: None,
            typst: None,
            attributes: Vec::new(),
        },
        TIMEOUT,
    );
    let took = asked.elapsed();
    summary.unwrap_or_else(|error| panic!("{error}"));
    assert!(fs::read(&output).unwrap().starts_with(b"%PDF-"));
    assert!(took < PROMPTLY, "answered after {took:?}");
}

/// `wait_until` waits for a search's last results, as the MCP server's `search` does.
#[test]
fn a_search_waits_only_for_its_own_results() {
    use adoc_engine::{commands::ids, CommandArgs, EngineEvent, SearchRequest};

    let mut held = held();
    let engine = &mut held.engine;
    let events = engine.subscribe();
    engine
        .invoke(
            ids::WORKSPACE_SEARCH,
            CommandArgs::Search(SearchRequest {
                query: "needle".to_owned(),
                regex: false,
                case_sensitive: false,
                whole_word: false,
                scope: None,
                all_text_files: false,
            }),
        )
        .unwrap();
    let (mut found, mut done) = (Vec::new(), false);
    let asked = Instant::now();
    let finished = engine.wait_until(asked + TIMEOUT, |_| {
        for event in events.try_iter() {
            if let EngineEvent::SearchResults {
                results,
                done: last,
                ..
            } = event
            {
                found.extend(results);
                done |= last;
            }
        }
        done
    });
    let took = asked.elapsed();
    assert!(finished);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].path.ends_with("page.adoc"));
    assert!(took < PROMPTLY, "answered after {took:?}");
}

/// The engine's parser, panicking on a page holding "boom".
struct PanickingParser;

impl adoc_dom::DocumentParser for PanickingParser {
    fn name(&self) -> &'static str {
        "panicking"
    }

    fn parse(&self, input: &adoc_dom::ParseInput<'_>) -> adoc_dom::Dom {
        assert!(!input.text.contains("boom"), "the parser gave up");
        adoc_dom::DocumentParser::parse(&adoc_dom::AcdcParser, input)
    }
}

/// A render whose parse crashes gives up at once, rather than waiting out its time for a
/// preview that will not come.
#[test]
fn render_blocking_gives_up_when_its_parse_crashes() {
    let mut held = held();
    let path = held.root.join("boom.adoc");
    fs::write(&path, "= Boom\n\nboom\n").unwrap();
    let engine = &mut held.engine;
    engine.set_document_parser(std::sync::Arc::new(PanickingParser));
    let id = engine.open(&path).unwrap();
    let asked = Instant::now();
    let preview = engine.render_blocking(id, TIMEOUT);
    let took = asked.elapsed();
    assert!(preview.is_none());
    assert!(took < PROMPTLY, "answered after {took:?}");
}
