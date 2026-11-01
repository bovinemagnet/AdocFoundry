//! Headless UI tests on GPUI's test platform: no display is needed.

use std::path::{Path, PathBuf};

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, Engine, Settings};
use gpui::{px, size, AppContext as _, Entity, TestAppContext, VisualTestContext, WindowHandle};
use gpui_component::Root;

use crate::{app::AppState, editor::SourceEditorView, workspace_view::WorkspaceView};

fn fixture(name: &str) -> PathBuf {
    adoc_index::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        // Dialogs and sheets then open at rest, rather than moving on the wall clock while a
        // test clicks them.
        cx.set_reduce_motion(true);
        crate::editor::language::register();
    });
}

fn app_with(engine: Engine, cx: &mut TestAppContext) -> Entity<AppState> {
    cx.update(|cx| crate::editor::bindings::bind(&engine, cx));
    cx.new(|cx| AppState::new(engine, cx))
}

fn widget_text(editor: &Entity<SourceEditorView>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| editor.read(cx).widget_text(cx))
}

fn document_text(app: &Entity<AppState>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .text
            .to_string()
    })
}

fn editor_window(
    app: &Entity<AppState>,
    document: adoc_engine::DocumentId,
    cx: &mut TestAppContext,
) -> (Entity<SourceEditorView>, WindowHandle<Root>) {
    let mut editor = None;
    let window = cx.open_window(size(px(800.), px(600.)), |window, cx| {
        let view = cx.new(|cx| SourceEditorView::new(app.clone(), document, false, window, cx));
        editor = Some(view.clone());
        Root::new(view, window, cx)
    });
    (editor.unwrap(), window)
}

#[gpui::test]
fn typing_reaches_the_engine_and_undo_goes_through_it(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    std::fs::write(&path, "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::default());
    let document = engine.open(&path).unwrap();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);

    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(7, 7), window, cx);
        });
    });
    // Separate keystrokes, as a user types them.
    cx.simulate_keystrokes("h e l l o");
    cx.run_until_parked();
    assert_eq!(document_text(&app, &mut cx), "= Page\nhello");
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.undo_stack(document).unwrap().len()),
        1,
        "single characters typed in a run coalesce into one undo entry"
    );
    assert!(cx.update(|_, cx| app.read(cx).engine.document(document).unwrap().dirty));

    // Undo is the engine's: the typing run coalesces into one entry.
    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(document_text(&app, &mut cx), "= Page\n");
    assert_eq!(widget_text(&editor, &mut cx), "= Page\n");
}

/// A typed character is found by one diff of the widget's and the engine's text, and the
/// engine's echo of it is not applied back: nothing per keystroke is the whole page twice.
#[gpui::test]
fn typing_diffs_once_and_never_reconciles(cx: &mut TestAppContext) {
    use adoc_core::probe::{self, Probe};
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    std::fs::write(&path, "= Page\n").unwrap();
    let mut engine = Engine::new(Settings::default());
    let document = engine.open(&path).unwrap();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(7, 7), window, cx)
        });
    });
    cx.run_until_parked();
    probe::reset();
    for character in "typing some prose here"
        .split("")
        .filter(|c| !c.is_empty())
        .take(20)
    {
        let key = if character == " " { "space" } else { character };
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
    }
    assert_eq!(document_text(&app, &mut cx), "= Page\ntyping some prose he");
    assert_eq!(probe::count(Probe::Diff), 20);
    assert_eq!(probe::count(Probe::Reconcile), 0);

    // An edit made elsewhere is still applied to the widget.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id: document,
                    range: SourceRange::new(0, 0),
                    text: "// note\n".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    cx.run_until_parked();
    assert_eq!(
        widget_text(&editor, &mut cx),
        "// note\n= Page\ntyping some prose he"
    );
    assert_eq!(probe::count(Probe::Reconcile), 1);
}

#[gpui::test]
fn engine_edits_reach_the_widget(cx: &mut TestAppContext) {
    init(cx);
    let mut engine = Engine::new(Settings::default());
    let document = engine.open_untitled();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id: document,
                    range: SourceRange::new(0, 0),
                    text: "== From the engine\n".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    cx.run_until_parked();
    assert_eq!(widget_text(&editor, &mut cx), "== From the engine\n");
}

#[gpui::test]
fn following_a_link_opens_a_tab_and_records_history(cx: &mut TestAppContext) {
    init(cx);
    let root = fixture("antora-multi-component");
    let page = root.join("alpha/modules/ROOT/pages/index.adoc");
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(60)));
    let app = app_with(engine, cx);

    let mut workspace = None;
    let window = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
        let view =
            cx.new(|cx| WorkspaceView::new(app.clone(), None, vec![page.clone()], window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 1);

    let (document, offset) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let id = engine.active().unwrap();
        let text = &engine.document(id).unwrap().parsed().text;
        (id, text.find("xref:beta::index.adoc").unwrap() + 6)
    });
    cx.update(|window, cx| {
        crate::workspace_view::follow_link(app.clone(), document, offset, window, cx);
    });
    cx.run_until_parked();

    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 2);
    let active_path = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone()
    });
    assert!(active_path.ends_with("beta/modules/ROOT/pages/index.adoc"));
    assert!(cx.update(|_, cx| app.read(cx).engine.can_go_back()));
}

#[gpui::test]
fn a_second_window_mirrors_the_active_document(cx: &mut TestAppContext) {
    init(cx);
    let mut engine = Engine::new(Settings::default());
    let document = engine.open_untitled();
    let app = app_with(engine, cx);
    let (_editor, main_window) = editor_window(&app, document, cx);

    let mut mirror = None;
    let mirror_window = cx.open_window(size(px(600.), px(400.)), |window, cx| {
        let view = cx.new(|cx| crate::mirror_view::MirrorView::new(app.clone(), window, cx));
        mirror = Some(view.clone());
        Root::new(view, window, cx)
    });
    let mirror = mirror.unwrap();
    let mut main = VisualTestContext::from_window(main_window.into(), cx);
    main.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id: document,
                    range: SourceRange::new(0, 0),
                    text: "Shared text\n".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    main.run_until_parked();

    let mut second = VisualTestContext::from_window(mirror_window.into(), cx);
    let mirrored = second.update(|_, cx| {
        mirror
            .read(cx)
            .editor()
            .map(|editor| editor.read(cx).widget_text(cx))
    });
    assert_eq!(mirrored.as_deref(), Some("Shared text\n"));
}

#[gpui::test]
fn the_search_panel_shows_workspace_results(cx: &mut TestAppContext) {
    init(cx);
    let root = fixture("antora-multi-component");
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![root]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);

    let mut search = None;
    let window = cx.open_window(size(px(800.), px(400.)), |window, cx| {
        let view = cx.new(|cx| {
            crate::search_view::SearchView::new(
                app.clone(),
                gpui::WeakEntity::new_invalid(),
                window,
                cx,
            )
        });
        search = Some(view.clone());
        Root::new(view, window, cx)
    });
    let search = search.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, cx| {
        search.update(cx, |search, cx| search.run_query("xref:", window, cx));
    });
    // Results arrive from a worker thread through the engine's event pump.
    let mut count = None;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        cx.update(|_, cx| app.update(cx, |app, cx| app.drain(cx)));
        cx.run_until_parked();
        count = cx.update(|_, cx| search.read(cx).result_count());
        if count.is_some() {
            break;
        }
    }
    assert_eq!(count, Some(9), "every xref: in the fixture is found");
}

/// IME composition reaches the editor through GPUI's input handler: marked (provisional) text is
/// replaced as the user composes, then committed. The engine must end up with exactly the
/// committed text, with nothing of the provisional stages left behind.
#[gpui::test]
fn ime_composition_commits_the_composed_text(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;

    init(cx);
    let mut engine = Engine::new(Settings::default());
    let document = engine.open_untitled();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let state = cx.update(|_, cx| editor.read(cx).state_for_test().clone());

    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.replace_text_in_range(None, "= Title\n", window, cx);
            for stage in ["に", "にほ", "にほん", "日本"] {
                let marked = state.marked_text_range(window, cx);
                state.replace_and_mark_text_in_range(marked, stage, None, window, cx);
            }
        });
    });
    cx.run_until_parked();
    assert_eq!(
        cx.update(|window, cx| state.update(cx, |state, cx| state.marked_text_range(window, cx))),
        Some(8..10),
        "the composition is still provisional"
    );

    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            let marked = state.marked_text_range(window, cx);
            state.replace_text_in_range(marked, "日本", window, cx);
        });
    });
    cx.run_until_parked();

    assert_eq!(document_text(&app, &mut cx), "= Title\n日本");
    assert_eq!(widget_text(&editor, &mut cx), "= Title\n日本");
    assert!(cx
        .update(|window, cx| state.update(cx, |state, cx| state.marked_text_range(window, cx)))
        .is_none());
    assert!(cx.update(|_, cx| app.read(cx).engine.document(document).unwrap().dirty));
}

/// Drains the engine until `done` holds: preview rendering runs on worker threads. The
/// deadline is generous because a book's PDF export, alone under a second, can take several
/// times as long while the other tests run alongside it.
fn drain_until(
    app: &Entity<AppState>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        cx.update(|_, cx| app.update(cx, |app, cx| app.drain(cx)));
        cx.run_until_parked();
        if done(cx) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn workspace_window(
    app: &Entity<AppState>,
    files: Vec<PathBuf>,
    cx: &mut TestAppContext,
) -> (Entity<WorkspaceView>, VisualTestContext) {
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| WorkspaceView::new(app.clone(), None, files.clone(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    (workspace.unwrap(), cx)
}

fn shown_paragraphs(workspace: &Entity<WorkspaceView>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().read(cx);
        let Some(shown) = preview.shown() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut stack: Vec<&adoc_render::ViewBlock> = shown.view.blocks.iter().rev().collect();
        while let Some(block) = stack.pop() {
            if let adoc_render::ViewKind::Paragraph(content) = &block.kind {
                out.push(adoc_render::ViewInline::plain_text(content));
            }
            stack.extend(block.child_refs().into_iter().rev());
        }
        out
    })
}

#[gpui::test]
fn preview_renders_the_active_document(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.adoc");
    let second = directory.path().join("second.adoc");
    std::fs::write(
        &first,
        "= First\n\nThe first page.\n\n[source,rust]\n----\nfn main() {}\n----\n",
    )
    .unwrap();
    std::fs::write(&second, "= Second\n\nThe second page.\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![first.clone()], cx);

    cx.simulate_keystrokes("secondary-k v");
    assert!(cx.update(|_, cx| app.read(cx).engine.preview_layout().visible));
    assert!(drain_until(&app, &mut cx, |cx| {
        !shown_paragraphs(&workspace, cx).is_empty()
    }));
    assert_eq!(shown_paragraphs(&workspace, &mut cx), ["The first page."]);

    // The preview follows the active document.
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &second,
                SourceRange::default(),
                adoc_engine::OpenDisposition::Preview,
                window,
                cx,
            );
        });
    });
    assert!(drain_until(&app, &mut cx, |cx| {
        shown_paragraphs(&workspace, cx) == ["The second page."]
    }));

    // Typing re-renders it once the edits settle.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(14, 20),
                    text: "edited".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    assert!(drain_until(&app, &mut cx, |cx| {
        shown_paragraphs(&workspace, cx) == ["The edited page."]
    }));

    // Hidden again, it renders nothing.
    cx.simulate_keystrokes("secondary-k v");
    cx.run_until_parked();
    assert!(cx.update(|_, cx| workspace.read(cx).preview().read(cx).shown().is_none()));
}

#[gpui::test]
fn preview_draws_block_stem_as_an_image(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Maths\n\n[stem]\n++++\nsqrt(4) = 2\n++++\n").unwrap();
    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    let app = app_with(engine, cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);

    cx.simulate_keystrokes("secondary-k v");
    let rendered = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let preview = workspace.read(cx).preview().read(cx);
            preview.shown().is_some_and(|shown| {
                shown.view.blocks.iter().any(|block| {
                    matches!(
                        &block.kind,
                        adoc_render::ViewKind::Stem {
                            rendered: adoc_render::DiagramView::Svg(_),
                            ..
                        }
                    )
                })
            })
        })
    };
    assert!(drain_until(&app, &mut cx, rendered), "the formula renders");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("stem-image").is_some());
}

#[gpui::test]
fn preview_draws_inline_stem_in_the_line(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(
        &page,
        "= Maths\n:stem:\n\nThe root stem:[sqrt(4) = 2] here.\n\nThe root sqrt4=2 here.\n",
    )
    .unwrap();
    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![directory.path().to_path_buf()])
        .unwrap();
    let app = app_with(engine, cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);

    cx.simulate_keystrokes("secondary-k v");
    let rendered = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let preview = workspace.read(cx).preview().read(cx);
            preview.shown().is_some_and(|shown| {
                shown.view.blocks.iter().any(|block| match &block.kind {
                    adoc_render::ViewKind::Paragraph(content) => content.iter().any(|inline| {
                        matches!(
                            inline,
                            adoc_render::ViewInline::Stem {
                                rendered: Some(_),
                                ..
                            }
                        )
                    }),
                    _ => false,
                })
            })
        })
    };
    assert!(drain_until(&app, &mut cx, rendered), "the formula renders");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let image = cx
        .debug_bounds("stem-inline")
        .expect("the formula is drawn in the line");
    let (with_formula, with_text) = cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().read(cx);
        (preview.extent(&[0]).unwrap(), preview.extent(&[1]).unwrap())
    });
    let height = |extent: crate::preview::sync::Extent| extent.bottom - extent.top;
    assert!(
        (height(with_formula) - height(with_text)).abs() <= px(2.),
        "{with_formula:?} against {with_text:?}"
    );
    assert!(image.size.width > px(0.) && image.size.height > px(0.));
    // The image lies across the paragraph's line, not beside or below it.
    let paragraph_top = cx.update(|_, cx| {
        workspace
            .read(cx)
            .preview()
            .read(cx)
            .block_position(&[0])
            .unwrap()
    });
    let top = image.origin.y - paragraph_top.y;
    assert!(
        top > px(-8.) && top < height(with_formula),
        "image at {top:?} in a paragraph {:?} tall",
        height(with_formula)
    );

    // Its gap is measured once, not again each frame.
    use adoc_core::probe::{self, Probe};
    probe::reset();
    for _ in 0..10 {
        cx.update(|window, cx| {
            let preview = workspace.read(cx).preview().clone();
            preview.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
    }
    assert_eq!(probe::count(Probe::MeasureFormula), 0);
}

/// An app whose settings live in `directory` and whose releases feed names `tag`, counting
/// the times it is read.
fn app_with_feed(
    directory: &std::path::Path,
    tag: &'static str,
    reads: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    cx: &mut TestAppContext,
) -> Entity<AppState> {
    let file = directory.join("settings.toml");
    if !file.exists() {
        std::fs::write(&file, "").unwrap();
    }
    let settings = Settings::load(Some(&file), None).unwrap();
    let app = app_with(Engine::new(settings), cx);
    let reads = reads.clone();
    cx.update(|cx| {
        app.update(cx, |app, _| {
            app.fetch = std::sync::Arc::new(move |url| {
                assert_eq!(url, adoc_engine::updates::FEED);
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(format!(
                    r#"{{"tag_name": "{tag}", "html_url": "https://example.org/{tag}"}}"#
                ))
            });
        });
    });
    app
}

#[gpui::test]
fn update_notice_is_shown_once(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let notices = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| gpui_component::WindowExt::notifications(window, cx).len())
    };

    let app = app_with_feed(directory.path(), "v99.0.0", &reads, cx);
    let (_, mut cx_first) = workspace_window(&app, Vec::new(), cx);
    cx_first.run_until_parked();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(notices(&mut cx_first), 1, "the newer release is announced");

    // Started again: no check is due for a day, so nothing is read or shown.
    let again = app_with_feed(directory.path(), "v99.0.0", &reads, cx);
    // A page is open, so the window has focus for the action.
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n").unwrap();
    let (_, mut cx_second) = workspace_window(&again, vec![page], cx);
    cx_second.run_until_parked();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(notices(&mut cx_second), 0);

    // Asked for, the check runs and shows the release again.
    cx_second.dispatch_action(crate::editor::bindings::CheckForUpdates);
    cx_second.run_until_parked();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(notices(&mut cx_second), 1);
}

#[gpui::test]
fn asking_when_up_to_date_says_so(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("settings.toml"),
        "[updates]\ncheck = false\n",
    )
    .unwrap();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = app_with_feed(directory.path(), "v0.0.1", &reads, cx);
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n").unwrap();
    let (_, mut cx) = workspace_window(&app, vec![page], cx);
    cx.run_until_parked();
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "switched off"
    );
    cx.dispatch_action(crate::editor::bindings::CheckForUpdates);
    cx.run_until_parked();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        cx.update(|window, cx| gpui_component::WindowExt::notifications(window, cx).len()),
        1
    );
}

fn detached_paragraphs(
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    cx.update(|_, cx| {
        let Some(preview) = workspace.read(cx).detached_preview() else {
            return Vec::new();
        };
        let Some(shown) = preview.read(cx).shown() else {
            return Vec::new();
        };
        shown
            .view
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                adoc_render::ViewKind::Paragraph(content) => {
                    Some(adoc_render::ViewInline::plain_text(content))
                }
                _ => None,
            })
            .collect()
    })
}

#[gpui::test]
fn detached_preview_follows_edits(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nBefore.\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);

    let invoke = |command, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.invoke(command, CommandArgs::None, cx).unwrap()
            });
        });
        cx.run_until_parked();
    };
    invoke(ids::VIEW_DETACH_PREVIEW, &mut cx);
    let layout = cx.update(|_, cx| app.read(cx).engine.preview_layout());
    assert!(layout.visible && layout.detached);
    assert!(cx.update(|_, cx| workspace.read(cx).detached_preview().is_some()));
    assert!(drain_until(&app, &mut cx, |cx| {
        detached_paragraphs(&workspace, cx) == ["Before."]
    }));

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(8, 14),
                    text: "After".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    assert!(drain_until(&app, &mut cx, |cx| {
        detached_paragraphs(&workspace, cx) == ["After."]
    }));

    // Docking again closes the window and shows the preview beside the editor.
    invoke(ids::VIEW_DETACH_PREVIEW, &mut cx);
    assert!(cx.update(|_, cx| workspace.read(cx).detached_preview().is_none()));
    assert!(!cx.update(|_, cx| app.read(cx).engine.preview_layout().detached));
    assert!(drain_until(&app, &mut cx, |cx| {
        shown_paragraphs(&workspace, cx) == ["After."]
    }));
}

/// Opens `files` in a workspace window with the docked preview showing and rendered.
fn previewing(
    files: Vec<PathBuf>,
    cx: &mut TestAppContext,
) -> (Entity<AppState>, Entity<WorkspaceView>, VisualTestContext) {
    init(cx);
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, files, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None, cx)
                .unwrap();
        });
    });
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| workspace.read(cx).preview().read(cx).extent(&[0]).is_some())
    }));
    (app, workspace, cx)
}

/// The trail and span of the innermost preview block at `needle` in `path`.
fn block_for(
    workspace: &Entity<WorkspaceView>,
    path: &std::path::Path,
    text: &str,
    needle: &str,
    cx: &mut VisualTestContext,
) -> (Vec<usize>, adoc_render::SourceSpan) {
    let offset = text.find(needle).unwrap();
    cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().read(cx);
        let view = &preview.shown().unwrap().view;
        let trail = adoc_render::block_at(view, path, offset).unwrap();
        let span = adoc_render::block_by_trail(view, &trail)
            .unwrap()
            .span
            .clone();
        (trail, span)
    })
}

fn caret(workspace: &Entity<WorkspaceView>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| {
        workspace
            .read(cx)
            .active_editor_for_test()
            .unwrap()
            .read(cx)
            .cursor(cx)
    })
}

fn click_block(workspace: &Entity<WorkspaceView>, trail: &[usize], cx: &mut VisualTestContext) {
    let position = cx
        .update(|_, cx| workspace.read(cx).preview().read(cx).block_position(trail))
        .expect("the block is laid out");
    cx.simulate_click(position, gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn clicking_a_block_moves_the_caret(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    let text = "= Page\n\nFirst paragraph.\n\n* An item\n* Another item\n\nLast paragraph.\n";
    std::fs::write(&page, text).unwrap();
    let (_app, workspace, mut cx) = previewing(vec![page.clone()], cx);
    assert_eq!(caret(&workspace, &mut cx), 0);

    let (trail, span) = block_for(&workspace, &page, text, "Last paragraph", &mut cx);
    click_block(&workspace, &trail, &mut cx);
    assert_eq!(caret(&workspace, &mut cx), span.range.start);
    assert_eq!(span.range.start, text.find("Last paragraph").unwrap());

    let (trail, span) = block_for(&workspace, &page, text, "First paragraph", &mut cx);
    click_block(&workspace, &trail, &mut cx);
    assert_eq!(caret(&workspace, &mut cx), span.range.start);
}

#[gpui::test]
fn clicking_partial_content_opens_the_partial(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    let partial = adoc_index::normalize_path(&directory.path().join("partials/note.adoc"));
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    let page_text = "= Page\n\nOn the page.\n\ninclude::partials/note.adoc[]\n";
    let partial_text = "// A partial.\n\nFrom the partial.\n";
    std::fs::write(&page, page_text).unwrap();
    std::fs::write(&partial, partial_text).unwrap();
    let (app, workspace, mut cx) = previewing(vec![page.clone()], cx);
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 1);

    let (trail, span) = block_for(
        &workspace,
        &partial,
        partial_text,
        "From the partial",
        &mut cx,
    );
    assert_eq!(span.path, partial);
    click_block(&workspace, &trail, &mut cx);

    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 2);
    let active = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone()
    });
    assert_eq!(active, partial);
    assert_eq!(
        caret(&workspace, &mut cx),
        partial_text.find("From the partial").unwrap()
    );
}

#[gpui::test]
fn clicking_a_link_in_the_preview_navigates(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    let other = adoc_index::normalize_path(&directory.path().join("other.adoc"));
    let text = "= Page\n\nxref:other.adoc[Other page]\n";
    std::fs::write(&page, text).unwrap();
    std::fs::write(&other, "= Other\n\nThe other page.\n").unwrap();
    let (app, workspace, mut cx) = previewing(vec![page.clone()], cx);

    // The paragraph is only the link, so its first character is link text.
    let (trail, _) = block_for(&workspace, &page, text, "xref:other", &mut cx);
    click_block(&workspace, &trail, &mut cx);

    // The block under the link must not take the click back to the page's own source.
    let active = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone()
    });
    assert_eq!(active, other);
}

/// A page of `count` one-line paragraphs, previewed.
fn previewing_paragraphs(
    count: usize,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    PathBuf,
    String,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("long.adoc"));
    let mut text = String::from("= Long\n");
    for number in 1..=count {
        text.push_str(&format!("\nParagraph {number}.\n"));
    }
    std::fs::write(&page, &text).unwrap();
    let (app, workspace, cx) = previewing(vec![page.clone()], cx);
    (directory, page, text, app, workspace, cx)
}

/// Draws a frame of the preview, rendering it again.
fn draw_preview(workspace: &Entity<WorkspaceView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let preview = workspace.read(cx).preview().clone();
        preview.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear(cx);
    });
}

#[gpui::test]
fn preview_lays_out_only_visible_blocks(cx: &mut TestAppContext) {
    let (_directory, _, _, _, workspace, mut cx) = previewing_paragraphs(2_000, cx);
    draw_preview(&workspace, &mut cx);
    let laid_out = cx.update(|_, cx| workspace.read(cx).preview().read(cx).laid_out());
    assert!(laid_out > 0, "the first blocks are laid out");
    assert!(laid_out <= 90, "{laid_out} blocks laid out");
}

#[gpui::test]
fn scroll_sync_reaches_a_block_not_laid_out(cx: &mut TestAppContext) {
    let (_directory, page, text, _, workspace, mut cx) = previewing_paragraphs(2_000, cx);
    let (trail, _) = block_for(&workspace, &page, &text, "Paragraph 1500.", &mut cx);
    let offset = text.find("Paragraph 1500.").unwrap();
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(offset, offset), window, cx);
        });
    });
    for _ in 0..4 {
        cx.run_until_parked();
        draw_preview(&workspace, &mut cx);
    }
    let (extent, top) = cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().read(cx);
        (preview.extent(&trail), preview.scroll_top())
    });
    let extent = extent.expect("the block is laid out");
    assert!(
        (extent.top - top - px(8.)).abs() <= px(2.),
        "{extent:?} at {top:?}"
    );
}

#[gpui::test]
fn preview_lays_out_inline_text_once_per_block(cx: &mut TestAppContext) {
    use adoc_core::probe::{self, Probe};
    let (_directory, _, _, _, workspace, mut cx) = previewing_paragraphs(300, cx);
    probe::reset();
    for _ in 0..10 {
        draw_preview(&workspace, &mut cx);
    }
    let laid_out = probe::count(Probe::InlineLayout);
    assert!(laid_out <= 300, "inline text laid out {laid_out} times");
}

#[gpui::test]
fn typing_in_one_paragraph_does_not_rerender_the_preview(cx: &mut TestAppContext) {
    use crate::renders::{self, View};
    let (_directory, _, text, _, workspace, mut cx) = previewing_paragraphs(80, cx);
    let offset = text.find("Paragraph 40.").unwrap() + "Paragraph".len();
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(offset, offset), window, cx);
        });
    });
    cx.run_until_parked();
    renders::reset();
    for _ in 0..20 {
        cx.simulate_keystrokes("x");
        cx.run_until_parked();
    }
    let rendered = renders::count(View::Preview);
    assert!(rendered <= 2, "the preview rendered {rendered} times");
}

#[gpui::test]
fn reveal_source_is_logarithmic(cx: &mut TestAppContext) {
    use adoc_core::probe::{self, Probe};
    let (_directory, page, text, _, workspace, mut cx) = previewing_paragraphs(500, cx);
    let offsets: Vec<usize> = (1..=200)
        .map(|number| text.find(&format!("Paragraph {}.", number * 2)).unwrap())
        .collect();
    probe::reset();
    cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().clone();
        for offset in offsets {
            preview.update(cx, |preview, cx| preview.reveal_source(&page, offset, cx));
        }
    });
    let walked = probe::count(Probe::ChildRefs);
    // 200 × log₂ 500.
    assert!(walked <= 1_800, "{walked} lists of children");
}

#[gpui::test]
fn moving_the_caret_to_the_title_scrolls_the_preview_to_the_top(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("long.adoc"));
    let mut text = String::from("= Long\n:description: A long page.\n");
    for number in 1..=80 {
        text.push_str(&format!("\nParagraph {number}.\n"));
    }
    std::fs::write(&page, &text).unwrap();
    let (_app, workspace, mut cx) = previewing(vec![page.clone()], cx);
    let preview = cx.update(|_, cx| workspace.read(cx).preview().clone());
    let reveal = |offset: usize, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
            editor.update(cx, |editor, cx| {
                editor.reveal(SourceRange::new(offset, offset), window, cx);
            });
        });
        cx.run_until_parked();
    };

    reveal(text.find("Paragraph 60.").unwrap(), &mut cx);
    assert!(cx.update(|_, cx| preview.read(cx).scroll_top()) > gpui::px(0.));

    // The title and header are not blocks, so no block contains the caret there.
    reveal(text.find("Long").unwrap(), &mut cx);
    assert_eq!(
        cx.update(|_, cx| preview.read(cx).scroll_top()),
        gpui::px(0.)
    );
}

/// Answers the export's save dialog with `output` and returns how the export finished.
fn finish_export(
    app: &Entity<AppState>,
    events: &std::sync::mpsc::Receiver<adoc_engine::EngineEvent>,
    output: &Path,
    cx: &mut VisualTestContext,
) -> Result<adoc_engine::ExportSummary, String> {
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let chosen = output.to_path_buf();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    cx.run_until_parked();
    let finished_event = |events: &std::sync::mpsc::Receiver<adoc_engine::EngineEvent>| {
        events.try_iter().find_map(|event| match event {
            adoc_engine::EngineEvent::ExportFinished { result, .. } => Some(result),
            _ => None,
        })
    };
    // The export started: it is running (the status bar shows "Exporting PDF…" while it is),
    // or a small page has already finished.
    let exporting = cx.update(|_, cx| app.read(cx).engine.is_exporting());
    let mut finished = if exporting {
        None
    } else {
        finished_event(events)
    };
    assert!(exporting || finished.is_some(), "no export started");
    assert!(drain_until(app, cx, |_| {
        finished = finished.take().or_else(|| {
            events.try_iter().find_map(|event| match event {
                adoc_engine::EngineEvent::ExportFinished { result, .. } => Some(result),
                _ => None,
            })
        });
        finished.is_some()
    }));
    finished.unwrap()
}

#[gpui::test]
fn export_command_writes_a_pdf(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n\nExported from the app.\n").unwrap();
    let output = directory.path().join("exported.pdf");
    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);

    // The command asks where to write the PDF; the test answers the save dialog.
    cx.dispatch_action(crate::editor::bindings::ExportPagePdf);
    let result = finish_export(&app, &events, &output, &mut cx);
    assert!(std::fs::read(&output).unwrap().starts_with(b"%PDF-"));
    assert_eq!(
        crate::workspace_view::export_message(&output, &result),
        "Exported exported.pdf (1 pages)"
    );
}

#[gpui::test]
fn export_page_to_html_writes_the_page(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n\nExported from the app.\n").unwrap();
    let output = directory.path().join("exported.html");
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);

    // The command asks where to write the page, suggesting its name; the test answers.
    cx.dispatch_action(crate::editor::bindings::ExportPageHtml);
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let chosen = output.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    let mut finished = None;
    assert!(drain_until(&app, &mut cx, |_| {
        finished = finished.take().or_else(|| {
            events.try_iter().find_map(|event| match event {
                adoc_engine::EngineEvent::ExportFinished { result, .. } => Some(result),
                _ => None,
            })
        });
        finished.is_some()
    }));
    let result = finished.unwrap();
    let html = std::fs::read_to_string(&output).unwrap();
    assert!(html.contains("Exported from the app."), "{html}");
    assert_eq!(
        crate::workspace_view::export_message(&output, &result),
        "Exported exported.html"
    );
}

#[gpui::test]
fn export_component_exports_the_active_page_s_component_with_unsaved_edits(
    cx: &mut TestAppContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(&directory.path().join("book"));
    copy_dir(&fixture("pdf/book"), &root);
    let page = root.join("guide/modules/ROOT/pages/use.adoc");
    let mut settings = Settings::default();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![page.clone()],
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        // The page's component is known once the workspace is indexed.
        cx.update(|_, cx| {
            app.read(cx).engine.active().is_some() && !app.read(cx).engine.is_indexing()
        })
    }));

    cx.dispatch_action(crate::editor::bindings::ExportComponentPdf);
    let saved = finish_export(&app, &events, &directory.path().join("saved.pdf"), &mut cx)
        .unwrap_or_else(|error| panic!("{error}"));
    // The page belongs to `guide`, which exports as a book, not as the page alone.
    assert!(saved.pages >= 8, "{} pages", saved.pages);

    // Unsaved paragraphs lengthen the book, and the file on disk is untouched.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            let id = app.engine.active().unwrap();
            let end = app.engine.document(id).unwrap().text.to_string().len();
            let text =
                "\nAn unsaved paragraph that is long enough to fill a line or two of the page.\n"
                    .repeat(150);
            app.engine
                .invoke(
                    ids::EDIT_INSERT_TEXT,
                    CommandArgs::Text {
                        id,
                        range: SourceRange::new(end, end),
                        text,
                    },
                )
                .unwrap();
        });
    });
    cx.dispatch_action(crate::editor::bindings::ExportComponentPdf);
    let edited = finish_export(&app, &events, &directory.path().join("edited.pdf"), &mut cx)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(edited.pages > saved.pages, "{} pages", edited.pages);
    assert!(!std::fs::read_to_string(&page).unwrap().contains("unsaved"));
}

#[gpui::test]
fn export_errors_name_the_file_and_line(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let dir = adoc_index::normalize_path(directory.path());
    std::fs::write(dir.join("broken-theme.yml"), "base: [unclosed\nfoo: bar\n").unwrap();
    let themed = dir.join("themed.adoc");
    std::fs::write(
        &themed,
        "= Themed\n:pdf-theme: broken\n:pdf-themesdir: .\n\nText.\n",
    )
    .unwrap();
    std::fs::write(dir.join("bad.png"), "not a png").unwrap();
    let imaged = dir.join("imaged.adoc");
    std::fs::write(&imaged, "= Imaged\n\nText.\n\nimage::bad.png[Bad]\n").unwrap();
    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![themed.clone(), imaged.clone()], cx);

    // A broken theme fails the export.
    let open = |page: &Path, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            app.update(cx, |app, _| {
                let id = app.engine.open(page).unwrap();
                app.engine.set_active(id);
            });
        });
    };
    open(&themed, &mut cx);
    cx.dispatch_action(crate::editor::bindings::ExportPagePdf);
    let output = themed.with_extension("pdf");
    let result = finish_export(&app, &events, &output, &mut cx);
    let message = crate::workspace_view::export_message(&output, &result);
    assert!(message.starts_with("PDF export failed: "), "{message}");
    let expected = format!("{}: error: line 2", dir.join("broken-theme.yml").display());
    assert!(message.contains(&expected), "{message}");
    assert!(!output.exists());

    // An image that cannot be read is shown as its alt text, with a warning naming its line.
    open(&imaged, &mut cx);
    cx.dispatch_action(crate::editor::bindings::ExportPagePdf);
    let output = imaged.with_extension("pdf");
    let result = finish_export(&app, &events, &output, &mut cx);
    let message = crate::workspace_view::export_message(&output, &result);
    assert!(message.starts_with("Exported imaged.pdf"), "{message}");
    let expected = format!("{}:5: image could not be read: bad.png", imaged.display());
    assert!(message.contains(&expected), "{message}");
    assert!(output.exists());
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[gpui::test]
fn scroll_sync_works_both_ways(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("long.adoc"));
    let mut text = String::from("= Long\n");
    for number in 1..=80 {
        text.push_str(&format!("\nParagraph {number}.\n"));
    }
    std::fs::write(&page, &text).unwrap();
    let (_app, workspace, mut cx) = previewing(vec![page.clone()], cx);
    let preview = cx.update(|_, cx| workspace.read(cx).preview().clone());
    assert_eq!(
        cx.update(|_, cx| preview.read(cx).scroll_top()),
        gpui::px(0.)
    );

    // Caret to preview: the preview scrolls to the caret's block.
    let (trail, _) = block_for(&workspace, &page, &text, "Paragraph 60.", &mut cx);
    let offset = text.find("Paragraph 60.").unwrap();
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(offset, offset), window, cx);
        });
    });
    cx.run_until_parked();
    let top = cx.update(|_, cx| preview.read(cx).scroll_top());
    let extent = cx.update(|_, cx| preview.read(cx).extent(&trail)).unwrap();
    assert!(top > gpui::px(0.), "the preview scrolled");
    assert!((extent.top - top) <= gpui::px(10.), "{extent:?} at {top:?}");

    // Preview to source: the editor scrolls to the block at the preview's top, caret unmoved.
    // Only blocks in view are laid out: one a little below the caret's.
    let (trail, _) = block_for(&workspace, &page, &text, "Paragraph 64.", &mut cx);
    let extent = cx.update(|_, cx| preview.read(cx).extent(&trail)).unwrap();
    cx.update(|_, cx| {
        preview.update(cx, |preview, cx| preview.scroll_for_test(extent.top, cx));
    });
    cx.run_until_parked();
    let line = text[..text.find("Paragraph 64.").unwrap()]
        .matches('\n')
        .count();
    let top_line = cx.update(|_, cx| {
        workspace
            .read(cx)
            .active_editor_for_test()
            .unwrap()
            .read(cx)
            .top_line(cx)
    });
    assert_eq!(top_line, Some(line));
    assert_eq!(caret(&workspace, &mut cx), offset, "the caret stays put");
}

// --- git ---------------------------------------------------------------------------------------

/// A workspace window on `root` with `page` open, after the engine has indexed it and found its
/// repository.
fn git_workspace(root: &Path, page: &Path, cx: &mut TestAppContext) -> VisualTestContext {
    init(cx);
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (_, cx) = workspace_window(&app, vec![page.to_path_buf()], cx);
    cx.run_until_parked();
    cx
}

#[gpui::test]
fn status_bar_shows_the_branch(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write("page.adoc", "= Page\n");
    test.commit_all("first");
    let mut cx = git_workspace(dir.path(), &dir.path().join("page.adoc"), cx);
    assert!(
        cx.debug_bounds("status-branch-main").is_some(),
        "the branch is in the status bar"
    );
}

#[gpui::test]
fn status_bar_shows_the_version_and_branch(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write("docs/antora.yml", "name: product\nversion: '2.0'\n");
    test.write("docs/modules/ROOT/pages/index.adoc", "= Home\n");
    test.commit_all("first");
    let page = dir.path().join("docs/modules/ROOT/pages/index.adoc");
    let mut cx = git_workspace(dir.path(), &page, cx);
    assert!(cx.debug_bounds("status-version-product@2.0").is_some());
    assert!(cx.debug_bounds("status-branch-main").is_some());
}

#[gpui::test]
fn status_bar_shows_no_branch_outside_git(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    if dir
        .path()
        .ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
    {
        return;
    }
    std::fs::write(dir.path().join("page.adoc"), "= Page\n").unwrap();
    let mut cx = git_workspace(dir.path(), &dir.path().join("page.adoc"), cx);
    assert!(cx.debug_bounds("status-branch-main").is_none());
}

#[gpui::test]
fn status_bar_shows_the_word_count_and_the_selection_s(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nOne two three.\n").unwrap();
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    settle_engine(&app, &mut cx);
    assert!(
        cx.debug_bounds("status-words-4").is_some(),
        "the page's words are in the status bar"
    );

    let start = "= Page\n\nOne ".len();
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(
                SourceRange::new(start, start + "two three".len()),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("status-words-2-of-4").is_some(),
        "the selection's words while there is one"
    );
}

#[gpui::test]
fn the_window_losing_focus_saves_with_on_focus_change(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.adoc");
    std::fs::write(&page, "= Page\n").unwrap();
    let mut settings = Settings::default().with_git_opt_in();
    settings.editor.auto_save = adoc_engine::AutoSave::OnFocusChange;
    let app = app_with(Engine::new(settings), cx);
    let (_, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.run_until_parked();
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                adoc_engine::commands::ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(7, 7),
                    text: "\nEdited.\n".to_owned(),
                },
                cx,
            )
            .unwrap();
        });
    });
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "= Page\n");

    // The test platform opens windows inactive; activate it, then blur it.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert!(cx.update(|window, _| window.is_window_active()));
    cx.deactivate_window();
    // The page is written off the UI thread.
    settle_engine(&app, &mut cx);
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        "= Page\n\nEdited.\n",
        "saved when the window lost focus"
    );
}

/// Drains the engine until its workers and timers are done, as the app's pump would.
fn settle_engine(app: &Entity<AppState>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let pending = cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.drain(cx);
                app.engine.has_pending()
            })
        });
        if !pending {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.run_until_parked();
}

/// A repository whose `page.adoc` has ten numbered lines, open in a workspace window.
fn ten_line_repository(
    dir: &Path,
    cx: &mut TestAppContext,
) -> (Entity<AppState>, Entity<WorkspaceView>, VisualTestContext) {
    init(cx);
    let test = adoc_git::testing::TestRepo::init(dir);
    let text: String = (1..=10).map(|number| format!("line {number}\n")).collect();
    test.write("page.adoc", &text);
    test.commit_all("first");
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![dir.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (workspace, mut cx) = workspace_window(&app, vec![dir.join("page.adoc")], cx);
    settle_engine(&app, &mut cx);
    (app, workspace, cx)
}

fn replace_in(app: &Entity<AppState>, find: &str, with: &str, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            let text = app.engine.document(id).unwrap().text.to_string();
            let start = text.find(find).unwrap();
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                adoc_engine::CommandArgs::Text {
                    id,
                    range: SourceRange::new(start, start + find.len()),
                    text: with.to_owned(),
                },
                cx,
            )
            .unwrap();
        })
    });
}

#[gpui::test]
fn gutter_marks_follow_hunks(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (app, workspace, mut cx) = ten_line_repository(dir.path(), cx);
    let editor = cx.update(|_, cx| workspace.read(cx).active_editor_for_test().unwrap().clone());
    assert!(cx
        .update(|_, cx| editor.read(cx).marks_for_test())
        .is_empty());

    replace_in(&app, "line 2\n", "line two\n", &mut cx);
    replace_in(&app, "line 5\n", "line 5\nnew line\n", &mut cx);
    replace_in(&app, "line 9\n", "", &mut cx);
    settle_engine(&app, &mut cx);

    let marks = cx.update(|_, cx| editor.read(cx).marks_for_test());
    let kinds: Vec<_> = marks.iter().map(|mark| mark.kind).collect();
    assert_eq!(
        kinds,
        [
            adoc_engine::HunkKind::Modified,
            adoc_engine::HunkKind::Added,
            adoc_engine::HunkKind::Deleted
        ]
    );
    // Each mark sits on its lines: the modified line is the second, one line high.
    let line_height = marks[0].bounds.size.height;
    assert!(line_height > gpui::px(0.));
    assert!(marks[1].bounds.top() > marks[0].bounds.bottom());
    assert_eq!(marks[1].bounds.size.height, line_height, "one added line");
    assert!(marks[2].bounds.top() > marks[1].bounds.bottom());

    // Undoing the edits clears the marks.
    for _ in 0..3 {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let id = app.engine.active().unwrap();
                app.invoke(ids::EDIT_UNDO, adoc_engine::CommandArgs::Document(id), cx)
                    .unwrap();
            })
        });
    }
    settle_engine(&app, &mut cx);
    assert!(cx
        .update(|_, cx| editor.read(cx).marks_for_test())
        .is_empty());
}

#[gpui::test]
fn clicking_a_mark_shows_head_text_and_reverts(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (app, workspace, mut cx) = ten_line_repository(dir.path(), cx);
    let editor = cx.update(|_, cx| workspace.read(cx).active_editor_for_test().unwrap().clone());
    replace_in(&app, "line 4\n", "line four\n", &mut cx);
    settle_engine(&app, &mut cx);
    let mark = cx.update(|_, cx| editor.read(cx).marks_for_test())[0].clone();

    cx.simulate_click(mark.bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).open_change_for_test()),
        Some("line 4\n".to_owned())
    );
    let revert = cx
        .debug_bounds("change-card")
        .expect("the change card is open");
    assert!(revert.size.width > gpui::px(0.));
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.revert_open_change(window, cx));
    });
    settle_engine(&app, &mut cx);
    let text = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .text
            .to_string()
    });
    assert!(text.contains("line 4\n") && !text.contains("four"));
    assert!(cx
        .update(|_, cx| editor.read(cx).marks_for_test())
        .is_empty());
}

#[gpui::test]
fn visual_blocks_marked_when_range_intersects_hunk(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write(
        "page.adoc",
        "= Page\n\nFirst paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n",
    );
    test.commit_all("first");
    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (_, mut cx) = workspace_window(&app, vec![dir.path().join("page.adoc")], cx);
    cx.simulate_keystrokes("secondary-k w");
    settle_engine(&app, &mut cx);
    assert!(
        cx.debug_bounds("visual-change-2").is_none(),
        "nothing changed yet"
    );

    replace_in(
        &app,
        "Second paragraph.",
        "Second paragraph, edited.",
        &mut cx,
    );
    settle_engine(&app, &mut cx);
    // Blocks: the title (0), the first (1), second (2) and third (3) paragraphs.
    assert!(
        cx.debug_bounds("visual-change-2").is_some(),
        "the edited paragraph is marked"
    );
    assert!(cx.debug_bounds("visual-change-1").is_none());
    assert!(cx.debug_bounds("visual-change-3").is_none());
}

#[gpui::test]
fn changes_panel_lists_changed_adoc_files_only(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write("modules/ROOT/pages/page.adoc", "= Page\n\nText.\n");
    test.write("modules/ROOT/pages/same.adoc", "= Same\n");
    test.write("README.md", "readme\n");
    test.write("notes.txt", "notes\n");
    test.commit_all("first");
    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let page = dir.path().join("modules/ROOT/pages/page.adoc");
    let (workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    settle_engine(&app, &mut cx);
    assert!(
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.changes_for_test(cx)))
            .is_empty(),
        "a repository with no changes has an empty panel"
    );

    test.write("modules/ROOT/pages/new.adoc", "= New\n");
    test.write("modules/ROOT/images/diagram.svg", "<svg/>\n");
    test.write("README.md", "readme, changed\n");
    test.write("notes.txt", "notes, changed\n");
    replace_in(&app, "Text.", "Text, edited.", &mut cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            app.invoke(
                ids::DOCUMENT_SAVE,
                adoc_engine::CommandArgs::Document(id),
                cx,
            )
            .unwrap();
            app.invoke(ids::GIT_REFRESH, adoc_engine::CommandArgs::None, cx)
                .unwrap();
        })
    });
    settle_engine(&app, &mut cx);

    let labels =
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.changes_for_test(cx)));
    assert_eq!(
        labels,
        [
            "?? modules/ROOT/images/diagram.svg",
            "?? modules/ROOT/pages/new.adoc",
            " M modules/ROOT/pages/page.adoc (1)",
        ]
    );
}

#[gpui::test]
fn blame_column_toggles_and_hover_shows_the_commit(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.write("page.adoc", "= Page\n\nText.\n");
    let first = test.commit_all("Write the page");
    // Settings are read from their file again when the workspace opens.
    let config = tempfile::tempdir().unwrap();
    let user = config.path().join("adocfoundry.toml");
    std::fs::write(&user, "[git]\nblame_on_hover = true\n").unwrap();
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (workspace, mut cx) = workspace_window(&app, vec![dir.path().join("page.adoc")], cx);
    settle_engine(&app, &mut cx);
    let editor = cx.update(|_, cx| workspace.read(cx).active_editor_for_test().unwrap().clone());

    assert!(cx.debug_bounds("blame-column").is_none());
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.toggle_blame(window, cx)));
    settle_engine(&app, &mut cx);
    assert!(cx.update(|_, cx| editor.read(cx).shows_blame()));
    assert!(cx.debug_bounds("blame-column").is_some());
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let blame = cx
        .update(|_, cx| app.read(cx).engine.blame_for(id))
        .unwrap();
    assert_eq!(blame[0].commit.as_deref(), Some(&first[..7]));

    // Resting on the gutter beside the third line shows its commit.
    let gutter = cx.update(|_, cx| {
        let state = editor.read(cx).state_for_test().read(cx);
        let line = state.range_to_bounds(&(8..8)).unwrap();
        gpui::point(line.left() - px(6.), line.center().y)
    });
    cx.simulate_mouse_move(gutter, None, gpui::Modifiers::none());
    settle_engine(&app, &mut cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    let tip = cx
        .update(|_, cx| editor.read(cx).hover_tip_for_test())
        .unwrap();
    assert!(
        tip.starts_with(&first[..7]) && tip.ends_with("Write the page"),
        "{tip}"
    );
    assert!(cx.debug_bounds("blame-tip").is_some());

    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.toggle_blame(window, cx)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("blame-column").is_none());
}

#[gpui::test]
fn save_and_commit_commits_the_page_and_counts_it_unpushed(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let test = adoc_git::testing::TestRepo::init(dir.path());
    test.set_identity("Ada Writer", "ada@example.com");
    test.write("page.adoc", "= The Page\n\nText.\n");
    test.commit_all("first");
    test.add_remote("origin", "git@github.com:org/docs.git");
    test.track_remote("origin");
    let mut engine = Engine::new(Settings::default());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (workspace, mut cx) = workspace_window(&app, vec![dir.path().join("page.adoc")], cx);
    settle_engine(&app, &mut cx);
    assert!(cx.debug_bounds("status-branch-main").is_some());

    replace_in(&app, "Text.", "Text, edited.", &mut cx);
    let (message, paths) = cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            match app
                .invoke(
                    ids::GIT_SAVE_AND_COMMIT,
                    adoc_engine::CommandArgs::Document(id),
                    cx,
                )
                .unwrap()
            {
                adoc_engine::CommandOutcome::CommitDraft { message, paths } => (message, paths),
                other => panic!("expected a draft, got {other:?}"),
            }
        })
    });
    assert_eq!(message, "docs: update The Page");
    settle_engine(&app, &mut cx);
    let labels =
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.changes_for_test(cx)));
    assert_eq!(
        labels,
        ["M  page.adoc (1)"],
        "staged; still one change from HEAD"
    );

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.commit(message, &paths, false, window, cx)
        });
    });
    settle_engine(&app, &mut cx);
    let labels =
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.changes_for_test(cx)));
    assert!(labels.is_empty(), "committed: {labels:?}");
    let ahead = cx.update(|_, cx| app.read(cx).engine.git().repositories()[0].ahead);
    assert_eq!(ahead, Some(1), "one commit not pushed");
}

// --- visual editor ---------------------------------------------------------------------------

/// A workspace showing `text` as its only page in the visual editor.
fn visual_workspace(
    text: &str,
    cx: &mut TestAppContext,
) -> (
    Entity<AppState>,
    Entity<WorkspaceView>,
    Entity<crate::visual::VisualView>,
    adoc_engine::DocumentId,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap().keep();
    let page = adoc_index::normalize_path(&directory.join("page.adoc"));
    std::fs::write(&page, text).unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.simulate_keystrokes("secondary-k w");
    cx.run_until_parked();
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let visual = cx
        .update(|_, cx| workspace.read(cx).visual_for_test(id).cloned())
        .expect("a visual editor");
    (app, workspace, visual, id, cx)
}

fn shown_blocks(
    visual: &Entity<crate::visual::VisualView>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    cx.update(|_, cx| {
        visual
            .read(cx)
            .shown()
            .map(|shown| {
                shown
                    .model
                    .text_blocks()
                    .map(|(_, _, text)| text.display.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn text_of(
    app: &Entity<AppState>,
    id: adoc_engine::DocumentId,
    cx: &mut VisualTestContext,
) -> String {
    cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().text_string())
}

fn place_caret(
    visual: &Entity<crate::visual::VisualView>,
    offset: usize,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(visual.read(cx), cx);
        window.focus(&focus, cx);
        visual.update(cx, |visual, cx| visual.place_caret(offset, cx));
    });
    cx.run_until_parked();
}

#[gpui::test]
fn visual_mode_renders_the_document(cx: &mut TestAppContext) {
    let (app, _, visual, id, mut cx) =
        visual_workspace("= Page\n\nA *bold* word.\n\n* An item\n", cx);
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.editor_mode(id)),
        adoc_engine::EditorMode::Visual
    );
    assert_eq!(
        shown_blocks(&visual, &mut cx),
        ["Page", "A bold word.", "An item"]
    );
    let drawn = cx.update(|_, cx| visual.read(cx).layouts().borrow().len());
    assert_eq!(drawn, 3, "every text block is laid out");

    cx.simulate_keystrokes("secondary-k s");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.editor_mode(id)),
        adoc_engine::EditorMode::Source
    );
}

#[gpui::test]
fn typing_in_visual_mode_edits_the_source(cx: &mut TestAppContext) {
    let text = "= Page\n\nA *bold* word.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("word").unwrap() + 4, &mut cx);
    cx.simulate_keystrokes("s space a n d space * m o r e");
    cx.run_until_parked();
    // A lone star forms no markup, so it is written as typed.
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nA *bold* words and *more.\n"
    );
    assert_eq!(
        shown_blocks(&visual, &mut cx),
        ["Page", "A bold words and *more."]
    );

    cx.simulate_keystrokes("backspace backspace");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nA *bold* words and *mo.\n"
    );

    // Undo is the engine's, shared with the source editor.
    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nA *bold* words and *mor.\n"
    );
}

#[gpui::test]
fn clicking_text_places_the_caret(cx: &mut TestAppContext) {
    let text = "= Page\n\nFirst paragraph.\n\nSecond paragraph.\n";
    let (_, _, visual, _, mut cx) = visual_workspace(text, cx);
    let bounds = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        layouts.get(&vec![2]).map(gpui::TextLayout::bounds)
    });
    let bounds = bounds.expect("the second paragraph is laid out");
    cx.simulate_click(
        gpui::point(bounds.left() + px(1.), bounds.top() + px(2.)),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(selection.head, text.find("Second").unwrap());

    // Past the end of the text, still inside the paragraph.
    cx.simulate_click(
        gpui::point(bounds.right() - px(2.), bounds.top() + px(2.)),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(
        selection.head,
        text.find("Second paragraph.").unwrap() + "Second paragraph.".len()
    );
}

/// Clicks `count` times at the shown text index `index` of the text block at `path`.
fn click_text(
    visual: &Entity<crate::visual::VisualView>,
    path: &[usize],
    index: usize,
    count: usize,
    cx: &mut VisualTestContext,
) {
    let position = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        let layout = layouts.get(&path.to_vec()).expect("the block is laid out");
        layout
            .position_for_index(index)
            .expect("the index is laid out")
            + gpui::point(px(1.), px(2.))
    });
    let button = gpui::MouseButton::Left;
    cx.simulate_event(gpui::MouseDownEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: count,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: count,
    });
    cx.run_until_parked();
}

#[gpui::test]
fn visual_code_blocks_are_highlighted(cx: &mut TestAppContext) {
    let code = "fn main() {\n    let answer = 42;\n}";
    let text = format!("= Page\n\n[source,rust]\n----\n{code}\n----\n");
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let editable = cx.update(|_, cx| {
        let shown = visual.read(cx).shown().cloned().unwrap();
        shown.model.walk().into_iter().any(|(_, node)| {
            matches!(node.kind, adoc_engine::NodeKind::Code { .. }) && node.is_editable()
        })
    });
    assert!(
        editable,
        "the code block is edited in place, not drawn read-only"
    );
    // The editable block's text was highlighted as Rust, with styles to lay over it.
    let styles = cx.update(|_, cx| {
        visual
            .read(cx)
            .highlights
            .borrow()
            .values()
            .find(|(language, text, _)| language == "rust" && text == code)
            .map(|(_, _, styles)| styles.len())
    });
    assert!(styles.is_some_and(|count| count > 0), "{styles:?}");
}

/// Draws a frame of the visual editor, rendering it again.
fn draw_visual(visual: &Entity<crate::visual::VisualView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        visual.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear(cx);
    });
}

#[gpui::test]
fn one_visual_frame_walks_the_model_at_most_twice(cx: &mut TestAppContext) {
    use adoc_core::probe::{self, Probe};
    let text = format!("= Page\n\n{}", "A *bold* paragraph.\n\n".repeat(300));
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    place_caret(&visual, text.len() / 2, &mut cx);
    draw_visual(&visual, &mut cx);
    probe::reset();
    draw_visual(&visual, &mut cx);
    let walks = probe::count(Probe::Walk);
    assert!(walks <= 2, "{walks} walks of the model in one frame");
}

/// A page of `count` one-line paragraphs, the `n`th reading `Paragraph n.`.
fn long_page(count: usize) -> String {
    let paragraphs: String = (1..=count).map(|n| format!("Paragraph {n}.\n\n")).collect();
    format!("= Page\n\n{paragraphs}")
}

#[gpui::test]
fn visual_lays_out_only_visible_blocks(cx: &mut TestAppContext) {
    let text = long_page(2_000);
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    draw_visual(&visual, &mut cx);
    let drawn = cx.update(|_, cx| visual.read(cx).layouts().borrow().len());
    // A line and the gap after it are at least 30 px, so 900 px hold no more than 30 rows.
    assert!(drawn > 0, "the first blocks are laid out");
    assert!(drawn <= 90, "{drawn} blocks laid out");
}

#[gpui::test]
fn visual_lays_out_only_visible_blocks_of_a_section(cx: &mut TestAppContext) {
    let text = long_page(2_000).replacen("= Page\n\n", "= Page\n\n== Section\n\n", 1);
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    draw_visual(&visual, &mut cx);
    let (drawn, heading) = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        (layouts.len(), layouts.contains_key(&vec![1]))
    });
    assert!(heading, "the section's heading is laid out");
    assert!(drawn <= 90, "{drawn} blocks laid out");
}

#[gpui::test]
fn split_mode_reveals_a_block_not_laid_out(cx: &mut TestAppContext) {
    let text = long_page(2_000);
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    draw_visual(&visual, &mut cx);
    let offset = text.find("Paragraph 1500.").unwrap();
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.reveal_offset(offset, cx)));
    for _ in 0..2 {
        draw_visual(&visual, &mut cx);
    }
    let (revealed, selection) = cx.update(|_, cx| {
        let visual = visual.read(cx);
        (
            visual.layouts().borrow().contains_key(&vec![1500]),
            visual.selection(),
        )
    });
    assert!(revealed, "the block is scrolled to and laid out");
    assert!(selection.head < offset, "the selection stays where it was");
}

#[gpui::test]
fn the_visual_caret_is_revealed_in_a_block_not_laid_out(cx: &mut TestAppContext) {
    let text = long_page(2_000);
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    let last = text.find("Paragraph 2000.").unwrap();
    place_caret(&visual, last, &mut cx);
    for _ in 0..3 {
        draw_visual(&visual, &mut cx);
    }
    let (laid_out, first_laid_out) = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        (
            layouts.contains_key(&vec![2000]),
            layouts.contains_key(&vec![1]),
        )
    });
    assert!(laid_out, "the caret's block is scrolled to and laid out");
    assert!(!first_laid_out, "the top of the page is out of view");
}

#[gpui::test]
fn moving_down_goes_on_into_blocks_out_of_view(cx: &mut TestAppContext) {
    let text = long_page(200);
    let (_, _, visual, _, mut cx) = visual_workspace(&text, cx);
    place_caret(&visual, text.find("Paragraph 1.").unwrap(), &mut cx);
    for _ in 0..60 {
        cx.simulate_keystrokes("down");
        draw_visual(&visual, &mut cx);
    }
    let head = cx.update(|_, cx| visual.read(cx).selection().head);
    assert!(
        text[head..].starts_with("Paragraph 61."),
        "at {:?}",
        &text[head..(head + 15).min(text.len())]
    );
}

#[gpui::test]
fn formulas_are_measured_once(cx: &mut TestAppContext) {
    use adoc_core::probe::{self, Probe};
    let formulas: String = (1..=20)
        .map(|power| format!("Power stem:[x^{power}] here.\n\n"))
        .collect();
    let text = format!("= Page\n:stem:\n\n{formulas}");
    let (app, _, visual, _, mut cx) = visual_workspace(&text, cx);
    let rendered = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            visual
                .read(cx)
                .shown()
                .is_some_and(|shown| shown.view.inline_formulas().len() == 20)
        })
    };
    assert!(drain_until(&app, &mut cx, rendered), "the formulas render");
    probe::reset();
    for _ in 0..10 {
        draw_visual(&visual, &mut cx);
    }
    let measured = probe::count(Probe::MeasureFormula);
    assert!(measured <= 20, "{measured} formulas measured in 10 frames");
}

#[gpui::test]
fn visual_editor_draws_inline_stem(cx: &mut TestAppContext) {
    let text = "= Page\n:stem:\n\nThe root stem:[sqrt(4) = 2] here.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let rendered = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            visual
                .read(cx)
                .shown()
                .is_some_and(|shown| !shown.view.inline_formulas().is_empty())
        })
    };
    assert!(drain_until(&app, &mut cx, rendered), "the formula renders");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("stem-inline").is_some(),
        "drawn in the line"
    );
    // The text is still the paragraph's, and the formula's macro one step of the caret: typing
    // after it writes after the macro.
    assert_eq!(
        shown_blocks(&visual, &mut cx),
        ["Page", "The root sqrt(4) = 2 here."]
    );
    let after = text.find("] here").unwrap() + 1;
    place_caret(&visual, after, &mut cx);
    cx.simulate_keystrokes("!");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n:stem:\n\nThe root stem:[sqrt(4) = 2]! here.\n"
    );
}

#[gpui::test]
fn double_click_selects_a_word(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (_, _, visual, _, mut cx) = visual_workspace(text, cx);
    let plain = text.find("plain").unwrap();
    click_text(&visual, &[1], "Some pl".len(), 2, &mut cx);
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(
        (selection.start(), selection.end()),
        (plain, plain + "plain".len())
    );
    // A triple click selects the paragraph.
    click_text(&visual, &[1], "Some pl".len(), 3, &mut cx);
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    let paragraph = text.find("Some").unwrap();
    assert_eq!(
        (selection.start(), selection.end()),
        (paragraph, paragraph + "Some plain words.".len())
    );
}

#[gpui::test]
fn caret_moves_by_word(cx: &mut TestAppContext) {
    use crate::visual::input::{SelectWordLeft, WordLeft, WordRight};

    let text = "= Page\n\nSome *plain*, words.\n";
    let (_, _, visual, _, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("Some").unwrap(), &mut cx);
    let head = |cx: &mut VisualTestContext| cx.update(|_, cx| visual.read(cx).selection().head);
    cx.dispatch_action(WordRight);
    assert_eq!(head(&mut cx), text.find(" *plain").unwrap());
    // Past the strong mark's delimiters: the word ends inside them, never in a delimiter.
    cx.dispatch_action(WordRight);
    assert_eq!(head(&mut cx), text.find("*,").unwrap());
    cx.dispatch_action(WordRight);
    assert_eq!(head(&mut cx), text.find(".\n").unwrap());
    cx.dispatch_action(WordLeft);
    assert_eq!(head(&mut cx), text.find("words").unwrap());
    cx.dispatch_action(SelectWordLeft);
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(selection.anchor, text.find("words").unwrap());
    // At the start of the marked word the caret stays outside the mark, before its `*`.
    assert_eq!(selection.head, text.find("*plain").unwrap());
}

#[gpui::test]
fn ime_composition_commits_once(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;

    let text = "= Page\n\nText.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find('.').unwrap(), &mut cx);
    let undo_before = cx.update(|_, cx| app.read(cx).engine.undo_stack(id).unwrap().len());
    cx.update(|window, cx| {
        visual.update(cx, |visual, cx| {
            for stage in ["に", "にほ", "にほん", "日本"] {
                let marked = visual.marked_text_range(window, cx);
                visual.replace_and_mark_text_in_range(marked, stage, None, window, cx);
            }
        });
    });
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        text,
        "nothing is written while composing"
    );
    let marked = cx
        .update(|window, cx| visual.update(cx, |visual, cx| visual.marked_text_range(window, cx)));
    assert_eq!(
        marked,
        Some(4..6),
        "the composition is provisional, after `Text`"
    );

    cx.update(|window, cx| {
        visual.update(cx, |visual, cx| {
            let marked = visual.marked_text_range(window, cx);
            visual.replace_text_in_range(marked, "日本", window, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), "= Page\n\nText日本.\n");
    assert!(cx
        .update(|window, cx| visual.update(cx, |visual, cx| visual.marked_text_range(window, cx)))
        .is_none());
    let undo_after = cx.update(|_, cx| app.read(cx).engine.undo_stack(id).unwrap().len());
    assert_eq!(undo_after, undo_before + 1, "the commit is one edit");
}

#[gpui::test]
fn keyboard_formatting_marks_the_selection_or_what_is_typed(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let start = text.find("plain").unwrap();
    place_caret(&visual, start, &mut cx);
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.select(start, start + 5, cx)));
    cx.simulate_keystrokes("secondary-b");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n"
    );

    // With nothing selected, the mark applies to what is typed next.
    let end = text_of(&app, id, &mut cx).find('.').unwrap();
    place_caret(&visual, end, &mut cx);
    cx.simulate_keystrokes("secondary-i space n e w");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words _new_.\n"
    );
}

#[gpui::test]
fn enter_splits_the_paragraph(cx: &mut TestAppContext) {
    let text = "= Page\n\nFirst half second half.\n\n* one\n* two\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("second").unwrap(), &mut cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let after = "= Page\n\nFirst half\n\nsecond half.\n\n* one\n* two\n";
    assert_eq!(text_of(&app, id, &mut cx), after);
    assert_eq!(
        shown_blocks(&visual, &mut cx),
        ["Page", "First half", "second half.", "one", "two"]
    );

    // At the end of a paragraph, Enter begins a paragraph that exists once typed into.
    place_caret(&visual, after.find("half.").unwrap() + 5, &mut cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), after, "nothing is written yet");
    cx.simulate_keystrokes("N e w");
    cx.run_until_parked();
    let after = "= Page\n\nFirst half\n\nsecond half.\n\nNew\n\n* one\n* two\n";
    assert_eq!(text_of(&app, id, &mut cx), after);

    // At the end of a list item, another item; Tab indents it.
    place_caret(&visual, after.find("two").unwrap() + 3, &mut cx);
    cx.simulate_keystrokes("enter t h r e e tab");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nFirst half\n\nsecond half.\n\nNew\n\n* one\n* two\n** three\n"
    );
}

#[gpui::test]
fn typing_in_a_table_cell_edits_only_the_cell(cx: &mut TestAppContext) {
    let text = "= Page\n\n|===\n|Name |Value\n\n|a |1\n|===\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("a |").unwrap() + 1, &mut cx);
    cx.simulate_keystrokes("space | space b");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\n|===\n|Name |Value\n\n|a \\| b |1\n|===\n"
    );
}

#[gpui::test]
fn source_edits_update_the_visual_editor(cx: &mut TestAppContext) {
    let text = "= Page\n\nFirst paragraph.\n";
    let (app, workspace, visual, id, mut cx) = visual_workspace(text, cx);
    cx.simulate_keystrokes("secondary-k d");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.editor_mode(id)),
        adoc_engine::EditorMode::Split
    );
    let editor = cx.update(|_, cx| {
        workspace
            .read(cx)
            .active_editor_for_test()
            .cloned()
            .unwrap()
    });
    let end = text.find('.').unwrap();
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(end, end), window, cx)
        });
    });
    cx.simulate_keystrokes("space e d i t e d");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nFirst paragraph edited.\n"
    );
    // The model is rebuilt once typing in the source pauses.
    assert!(drain_until(&app, &mut cx, |cx| {
        shown_blocks(&visual, cx) == ["Page", "First paragraph edited."]
    }));
}

#[gpui::test]
fn undo_is_shared_between_editors(cx: &mut TestAppContext) {
    let text = "= Page\n\nText.\n";
    let (app, workspace, visual, id, mut cx) = visual_workspace(text, cx);
    cx.simulate_keystrokes("secondary-k d");
    cx.run_until_parked();
    place_caret(&visual, text.find('.').unwrap(), &mut cx);
    cx.simulate_keystrokes("s");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), "= Page\n\nTexts.\n");
    let editor = cx.update(|_, cx| {
        workspace
            .read(cx)
            .active_editor_for_test()
            .cloned()
            .unwrap()
    });
    assert_eq!(
        widget_text(&editor, &mut cx),
        "= Page\n\nTexts.\n",
        "the source shows the visual edit"
    );

    // Undo from the source editor undoes the visual edit, in both views.
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(0, 0), window, cx)
        });
    });
    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), text);
    assert_eq!(widget_text(&editor, &mut cx), text);
    assert_eq!(shown_blocks(&visual, &mut cx), ["Page", "Text."]);
}

#[gpui::test]
fn tables_and_titles_are_drawn_as_written(cx: &mut TestAppContext) {
    let text = "= Page\n\n.Widths\n[cols=\"1,3\"]\n|===\n|a |b\n|===\n\n\
                [cols=\"1,1a\"]\n|===\n|Plain\n|Paragraph one.\n\nParagraph two.\n|===\n\n\
                |===\n|A |B |C\n\n2+|Two columns |C\n|===\n";
    let (_, _, _, _, mut cx) = visual_workspace(text, cx);
    assert!(
        cx.debug_bounds("visual-title-1").is_some(),
        "a block's title is shown above it"
    );
    let narrow = cx
        .debug_bounds("visual-cell-1-0-0")
        .expect("the first cell");
    let wide = cx
        .debug_bounds("visual-cell-1-0-1")
        .expect("the second cell");
    let ratio = f32::from(wide.size.width) / f32::from(narrow.size.width);
    assert!((ratio - 3.).abs() < 0.1, "the columns follow cols: {ratio}");
    assert!(
        cx.debug_bounds("visual-cell-2-0-1-block-1").is_some(),
        "an AsciiDoc cell shows every block it holds"
    );
    // Without cols, every column is as wide as the others and a span takes its columns.
    let one = cx.debug_bounds("visual-cell-3-0-0").expect("a header cell");
    let two = cx
        .debug_bounds("visual-cell-3-1-0")
        .expect("the spanning cell");
    let ratio = f32::from(two.size.width) / f32::from(one.size.width);
    assert!(
        (ratio - 2.).abs() < 0.1,
        "a span takes its columns: {ratio}"
    );
}

#[gpui::test]
fn toolbar_bold_toggles_strong(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n\n* An item\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let start = text.find("plain").unwrap();
    place_caret(&visual, start, &mut cx);
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.select(start, start + 5, cx)));
    cx.run_until_parked();
    let bold = cx
        .debug_bounds("toolbar-bold")
        .expect("the toolbar is drawn");
    cx.simulate_click(bold.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n\n* An item\n"
    );

    // The list buttons act on the caret's block.
    let item = text_of(&app, id, &mut cx).find("An item").unwrap();
    place_caret(&visual, item, &mut cx);
    let numbers = cx
        .debug_bounds("toolbar-numbers")
        .expect("the list buttons are drawn");
    cx.simulate_click(numbers.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n\n. An item\n"
    );
}

#[gpui::test]
fn included_content_is_not_editable(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap().keep();
    std::fs::write(directory.join("partial.adoc"), "From the partial.\n").unwrap();
    let text = "= Page\n\nBefore.\n\ninclude::partial.adoc[]\n";
    let page = adoc_index::normalize_path(&directory.join("page.adoc"));
    std::fs::write(&page, text).unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.simulate_keystrokes("secondary-k w");
    cx.run_until_parked();
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let visual = cx
        .update(|_, cx| workspace.read(cx).visual_for_test(id).cloned())
        .unwrap();
    let included = cx.update(|_, cx| {
        let shown = visual.read(cx).shown().cloned().unwrap();
        shown
            .model
            .walk()
            .into_iter()
            .any(|(_, node)| matches!(node.kind, adoc_engine::NodeKind::Include { .. }))
    });
    assert!(included, "the include is a node of its own");
    place_caret(&visual, text.find("partial.adoc").unwrap(), &mut cx);
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        text,
        "nothing is typed into the include"
    );
}

#[gpui::test]
fn sidebar_navigation_moves_the_visual_caret(cx: &mut TestAppContext) {
    let text = "= Page\n\nIntro.\n\n== Later\n\nMore text.\n";
    let (app, workspace, visual, id, mut cx) = visual_workspace(text, cx);
    let path = cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().path.clone());
    let heading = text.find("== Later").unwrap();
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &path,
                SourceRange::new(heading, heading),
                adoc_engine::OpenDisposition::Preview,
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(
        selection.head,
        text.find("Later").unwrap(),
        "the caret lands in the heading's text"
    );
}

#[gpui::test]
fn showing_read_only_source_leaves_the_visual_caret(cx: &mut TestAppContext) {
    let mut text = String::from("= Page\n\nIntro.\n");
    for number in 1..=40 {
        text.push_str(&format!("\nParagraph {number}.\n"));
    }
    text.push_str("\n++++\n<b>raw</b>\n++++\n");
    for number in 41..=80 {
        text.push_str(&format!("\nParagraph {number}.\n"));
    }
    let (app, workspace, visual, id, mut cx) = visual_workspace(&text, cx);
    let path = cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().path.clone());
    let intro = text.find("Intro").unwrap();
    place_caret(&visual, intro, &mut cx);
    // As a click on the passthrough does: its source is shown beside the visual editor.
    let block = text.find("++++").unwrap();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(ids::VIEW_SHOW_SPLIT, CommandArgs::None, cx)?;
            app.invoke(
                ids::NAVIGATE_REVEAL,
                CommandArgs::Position(adoc_engine::NavigationPosition {
                    path: path.clone(),
                    offset: block,
                }),
                cx,
            )
        })
        .unwrap();
    });
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(
        selection.head, intro,
        "a block with no text leaves the visual caret where it was"
    );
    let line = text[..block].matches('\n').count();
    let top_line = cx.update(|_, cx| {
        workspace
            .read(cx)
            .active_editor_for_test()
            .unwrap()
            .read(cx)
            .top_line(cx)
    });
    assert!(
        top_line.is_some_and(|top| top <= line && line <= top + 3),
        "the source shows the block's line {line} near its top: top line {top_line:?}"
    );
}

#[gpui::test]
fn down_keeps_to_the_column_in_a_table(cx: &mut TestAppContext) {
    let text = "= Page\n\n|===\n|Name |Value\n\n|a |1\n|===\n";
    let (_, _, visual, _, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("Value").unwrap() + 2, &mut cx);
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    let one = text.find("|1").unwrap() + 1;
    assert!(
        (one..=one + 1).contains(&selection.head),
        "the caret goes to the cell below, not the next cell: {} not in {one}..={}",
        selection.head,
        one + 1
    );
}

#[gpui::test]
fn home_and_end_work_on_the_line(cx: &mut TestAppContext) {
    let text = "= Page\n\n----\nfirst = 1\nsecond = 2\n----\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    place_caret(&visual, text.find("first").unwrap() + 2, &mut cx);
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    assert_eq!(
        selection.head,
        text.find("first = 1").unwrap() + "first = 1".len()
    );
    cx.simulate_keystrokes("down home x");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\n----\nfirst = 1\nxsecond = 2\n----\n"
    );
}

#[gpui::test]
fn the_image_dialog_edits_the_image(cx: &mut TestAppContext) {
    let text = "= Page\n\nimage::old.png[Old,200,100]\n\nAfter.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let at = text.find("image::").unwrap();
    cx.update(|window, cx| {
        visual.update(cx, |visual, cx| visual.image_dialog(Some(at), window, cx))
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("end");
    cx.simulate_input("-new");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nimage::old.png-new[Old,200,100]\n\nAfter.\n"
    );
}

#[gpui::test]
fn the_link_dialog_takes_the_target_as_typed(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let start = text.find("plain").unwrap();
    place_caret(&visual, start, &mut cx);
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.select(start, start + 5, cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-shift-l");
    cx.run_until_parked();
    cx.simulate_input("https://example.com");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome https://example.com[plain] words.\n"
    );
}

#[gpui::test]
fn active_tab_stays_visible_with_the_preview_docked(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let pages: Vec<PathBuf> = (1..=8)
        .map(|number| {
            let page = adoc_index::normalize_path(
                &directory
                    .path()
                    .join(format!("a-rather-long-page-name-number-{number}.adoc")),
            );
            std::fs::write(&page, format!("= Page {number}\n\nText.\n")).unwrap();
            page
        })
        .collect();
    let (app, workspace, mut cx) = previewing(pages.clone(), cx);
    // Activate the last page from elsewhere, as following a link or the outline does.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.open(pages.last().unwrap()).unwrap();
            app.engine.set_active(id);
            app.drain(cx);
        });
    });
    // Two frames, as the platform would draw them.
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let visible = cx.update(|_, cx| workspace.read(cx).active_tab_visible_for_test());
    assert_eq!(visible, Some(true));
}

#[gpui::test]
fn status_bar_follows_the_visual_caret(cx: &mut TestAppContext) {
    let text = "= Page\n\nFirst paragraph.\n\nSecond é paragraph.\n";
    let (_app, workspace, visual, _id, mut cx) = visual_workspace(text, cx);
    // After the `é`, a two-byte character: line 5, column 9 counted in characters.
    let offset = text.find('é').unwrap() + 'é'.len_utf8();
    place_caret(&visual, offset, &mut cx);
    let position = cx.update(|window, cx| workspace.read(cx).caret_line_column(window, cx));
    assert_eq!(position, Some((4, 8)));
}

#[gpui::test]
fn the_problems_panel_shows_the_engines_problems(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    // A character of two bytes before the problem: the column counts characters.
    std::fs::write(root.join("café.adoc"), "= Café\n\nÉté: <<nowhere>>.\n").unwrap();
    std::fs::write(root.join("fine.adoc"), "= Fine\n").unwrap();
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
        let view = cx
            .new(|cx| WorkspaceView::new(app.clone(), Some(root.clone()), Vec::new(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_problems_for_test(cx)));
    let rows = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| workspace.read(cx).problem_rows_for_test(cx))
    };
    assert!(drain_until(&app, &mut cx, |cx| rows(cx).len() == 2));
    assert_eq!(rows(&mut cx)[1], "3:6 Unresolved AsciiDoc anchor: nowhere");
    assert!(rows(&mut cx)[0].contains("café.adoc"));
    let count = cx.update(|_, cx| app.read(cx).engine.problem_count());
    assert_eq!(count, 1);
}

#[gpui::test]
fn long_diagnostics_show_in_full_on_hover(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    let target = "a-target-page-whose-name-is-much-longer-than-the-sidebar-is-wide.adoc";
    std::fs::write(&page, format!("= Page\n\nSee xref:{target}[the target].\n")).unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_problems_for_test(cx)));
    let draw = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };
    // The file is row 0 and its first diagnostic row 1.
    let mut row = None;
    for _ in 0..400 {
        cx.update(|_, cx| app.update(cx, |app, cx| app.drain(cx)));
        draw(&mut cx);
        row = cx.debug_bounds("problems-row-1");
        if row.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let row = row.expect("a diagnostic row");
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    draw(&mut cx);
    let shown = crate::workspace_view::LAST_TREE_TOOLTIP.with(|last| last.borrow().clone());
    let shown = shown.expect("the row shows a tooltip");
    assert!(shown.contains(target), "{shown}");
}

#[gpui::test]
fn palette_hides_visual_commands_in_source_mode(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (app, _, _visual, id, mut cx) = visual_workspace(text, cx);
    let offered = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            crate::workspace_view::palette_commands(&app.read(cx).engine)
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>()
        })
    };
    assert!(offered(&mut cx).contains(&ids::EDIT_TOGGLE_STRONG));
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.engine
                .invoke(ids::VIEW_SHOW_SOURCE, CommandArgs::Document(id))
                .unwrap();
        });
    });
    cx.run_until_parked();
    let source = offered(&mut cx);
    // Bold works in the source editor too; lists are the visual editor's alone.
    assert!(source.contains(&ids::EDIT_TOGGLE_STRONG));
    assert!(!source.contains(&ids::EDIT_TOGGLE_BULLET_LIST));
    assert!(source.contains(&ids::VIEW_SHOW_VISUAL));
}

#[gpui::test]
fn marks_toggle_in_the_source_editor(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nSome plain words.\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    let editor = cx.update(|_, cx| workspace.read(cx).active_editor_for_test().unwrap().clone());
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let plain = SourceRange::new(13, 18);
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.reveal(plain, window, cx)));
    cx.run_until_parked();

    cx.simulate_keystrokes("secondary-b");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n"
    );
    // The word stays selected, so pressing again takes the mark off.
    let selection = cx.update(|_, cx| editor.read(cx).selection(cx));
    assert_eq!(selection, 14..19);
    cx.simulate_keystrokes("secondary-b");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), "= Page\n\nSome plain words.\n");

    cx.simulate_keystrokes("secondary-i");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome _plain_ words.\n"
    );
    // One undo, through the engine, as for any edit.
    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(text_of(&app, id, &mut cx), "= Page\n\nSome plain words.\n");
}

#[gpui::test]
fn inserts_work_in_the_source_editor(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Page\n\nSee the guide.\n").unwrap();
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    let editor = cx.update(|_, cx| workspace.read(cx).active_editor_for_test().unwrap().clone());
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());

    // Insert Link wraps the selection, leaving the caret where the target goes.
    let guide = SourceRange::new(16, 21);
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.reveal(guide, window, cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-shift-l");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSee the link:[guide].\n"
    );
    assert_eq!(cx.update(|_, cx| editor.read(cx).selection(cx)), 21..21);

    // Insert Table, from the end of the page, selects the first header cell to type over.
    let end = SourceRange::new(29, 29);
    cx.update(|window, cx| editor.update(cx, |editor, cx| editor.reveal(end, window, cx)));
    cx.run_until_parked();
    cx.dispatch_action(crate::editor::bindings::InsertTable);
    cx.run_until_parked();
    let text = text_of(&app, id, &mut cx);
    assert!(
        text.ends_with(".\n\n|===\n|Column 1 |Column 2\n\n| |\n| |\n|===\n"),
        "{text}"
    );
    let selection = cx.update(|_, cx| editor.read(cx).selection(cx));
    assert_eq!(&text[selection], "Column 1");
}

#[gpui::test]
fn export_from_the_palette_writes_a_pdf(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n\nExported from the palette.\n").unwrap();
    let output = directory.path().join("exported.pdf");
    let mut engine = Engine::new(Settings::default());
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);

    cx.simulate_keystrokes("secondary-shift-p");
    cx.run_until_parked();
    cx.simulate_input("Export Page to PDF");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let result = finish_export(&app, &events, &output, &mut cx);
    assert!(result.is_ok(), "{result:?}");
    assert!(std::fs::read(&output).unwrap().starts_with(b"%PDF-"));
}

#[gpui::test]
fn palette_commands_reach_the_workspace(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page], cx);
    let visible = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| app.read(cx).engine.preview_layout().visible)
    };
    assert!(!visible(&mut cx));
    cx.simulate_keystrokes("secondary-shift-p");
    cx.run_until_parked();
    cx.simulate_input("Toggle Preview");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        visible(&mut cx),
        "the palette's command reached the workspace"
    );
}

#[gpui::test]
fn quick_open_opens_the_page_chosen(cx: &mut TestAppContext) {
    init(cx);
    let root = fixture("antora-multi-component");
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let index = root.join("alpha/modules/ROOT/pages/index.adoc");
    let (_workspace, mut cx) = workspace_window(&app, vec![index], cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-p");
    cx.run_until_parked();
    // The component, from the resource id, and the start of the file name: no row's text
    // holds this as typed, so only the engine's matching finds it.
    cx.simulate_input("beta refer");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (active, preview) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let id = engine.active().unwrap();
        (
            engine.document(id).unwrap().path.clone(),
            engine.is_preview(adoc_engine::TabItem::Document(id)),
        )
    });
    assert_eq!(active, root.join("beta/modules/api/pages/reference.adoc"));
    assert!(!preview, "a page chosen by name opens as an ordinary tab");
}

#[gpui::test]
fn the_last_session_comes_back_with_its_tabs_modes_carets_and_preview(cx: &mut TestAppContext) {
    init(cx);
    let config = tempfile::tempdir().unwrap();
    let pages = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(pages.path());
    let page = |name: &str| root.join(format!("{name}.adoc"));
    for name in ["one", "two", "three"] {
        std::fs::write(page(name), format!("= Page {name}\n\nSome text.\n")).unwrap();
    }
    {
        let app = app_with(Engine::new(settings_in(config.path())), cx);
        let files = vec![page("three"), page("two")];
        let (workspace, mut cx) = workspace_window(&app, files, &mut *cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.invoke(
                    ids::WORKSPACE_OPEN_FOLDER,
                    CommandArgs::Path(root.clone()),
                    cx,
                )
                .unwrap();
                let two = app.engine.document_for_path(&page("two")).unwrap();
                app.invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(two), cx)
                    .unwrap();
                app.invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None, cx)
                    .unwrap();
            });
        });
        cx.run_until_parked();
        // The caret in the source page, which is then the active one.
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_at(
                    &page("three"),
                    SourceRange::new(9, 9),
                    adoc_engine::OpenDisposition::Persistent,
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| workspace.read(cx).write_session(cx));
    }

    // Launched with nothing to open.
    let app = app_with(Engine::new(settings_in(config.path())), cx);
    let (workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    cx.run_until_parked();
    let (roots, tabs, two, active, visible) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let tabs: Vec<PathBuf> = workspace
            .read(cx)
            .tab_items(cx)
            .into_iter()
            .map(|item| {
                engine
                    .document(item.document().unwrap())
                    .unwrap()
                    .path
                    .clone()
            })
            .collect();
        let two = engine.document_for_path(&page("two")).unwrap();
        let active = engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone();
        (
            engine.roots().to_vec(),
            tabs,
            two,
            active,
            engine.preview_layout().visible,
        )
    });
    assert_eq!(roots, std::slice::from_ref(&root));
    assert_eq!(tabs, [page("three"), page("two")]);
    assert_eq!(active, page("three"));
    assert!(visible);
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.editor_mode(two)),
        adoc_engine::EditorMode::Visual
    );
    assert!(cx.update(|_, cx| workspace.read(cx).visual_for_test(two).is_some()));
    assert_eq!(caret(&workspace, &mut cx), 9);
}

#[gpui::test]
fn open_recent_opens_the_folder_chosen(cx: &mut TestAppContext) {
    init(cx);
    let config = tempfile::tempdir().unwrap();
    let folders = tempfile::tempdir().unwrap();
    let folder = |name: &str| adoc_index::normalize_path(&folders.path().join(name));
    for name in ["alpha", "beta"] {
        std::fs::create_dir(folder(name)).unwrap();
    }
    let app = app_with(Engine::new(settings_in(config.path())), cx);
    let (_workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            for name in ["alpha", "beta"] {
                app.invoke(
                    ids::WORKSPACE_OPEN_FOLDER,
                    CommandArgs::Path(folder(name)),
                    cx,
                )
                .unwrap();
            }
        });
    });
    cx.run_until_parked();
    cx.dispatch_action(crate::editor::bindings::OpenRecent);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    cx.simulate_input("alpha");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.roots().to_vec()),
        [folder("alpha")]
    );
}

#[gpui::test]
fn new_page_makes_the_page_opens_it_and_lists_it_in_the_navigation(cx: &mut TestAppContext) {
    let (directory, app, workspace, mut cx) = drag_workspace("guide.adoc", cx);
    let root = adoc_index::normalize_path(directory.path());
    cx.dispatch_action(crate::editor::bindings::NewPage);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    cx.simulate_input("Release Notes");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let page = root.join("alpha/modules/ROOT/pages/release-notes.adoc");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "= Release Notes\n");
    let active = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone()
    });
    assert_eq!(active, page);
    assert!(!cx.update(gpui_component::WindowExt::has_active_dialog));
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| {
            workspace
                .read(cx)
                .tree_rows_for_test(adoc_engine::SidebarPanel::Navigation, cx)
                .iter()
                .any(|row| row.contains("Release Notes") || row.contains("release-notes"))
        })
    }));
}

#[gpui::test]
fn palette_commands_reach_the_visual_editor(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let start = text.find("plain").unwrap();
    place_caret(&visual, start, &mut cx);
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.select(start, start + 5, cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-shift-p");
    cx.run_until_parked();
    cx.simulate_input("Bold");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n"
    );
}

/// Clicks the element tagged with `selector`.
fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn the_menu_button_opens_into_the_menus(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("app-menu-File").is_none());

    // The button opens the first menu and shows every menu's name in its place.
    click("app-menu", &mut cx);
    assert!(cx.debug_bounds("app-menu").is_none());
    for selector in [
        "app-menu-File",
        "app-menu-Edit",
        "app-menu-Format",
        "app-menu-View",
        "app-menu-Go",
        "app-menu-Git",
        "app-menu-Help",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} is not shown"
        );
    }

    // Another name opens its menu; choosing an item runs it and folds the menus away.
    click("app-menu-View", &mut cx);
    let steps = cx.update(|_, cx| {
        let menus = crate::menus::menus(&app.read(cx).engine);
        let view = menus.iter().find(|menu| menu.name == "View").unwrap();
        view.items
            .iter()
            .filter(|item| matches!(item, crate::menus::Item::Action { .. }))
            .position(|item| {
                matches!(item, crate::menus::Item::Action { label, .. } if label == "Toggle Preview")
            })
            .unwrap()
    });
    let visible = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| app.read(cx).engine.preview_layout().visible)
    };
    assert!(!visible(&mut cx));
    for _ in 0..=steps {
        cx.simulate_keystrokes("down");
    }
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(visible(&mut cx), "the menu's command reached the workspace");
    assert!(cx.debug_bounds("app-menu").is_some());
    assert!(cx.debug_bounds("app-menu-View").is_none());
}

#[gpui::test]
fn menu_commands_reach_the_visual_editor(cx: &mut TestAppContext) {
    let text = "= Page\n\nSome plain words.\n";
    let (app, _, visual, id, mut cx) = visual_workspace(text, cx);
    let start = text.find("plain").unwrap();
    place_caret(&visual, start, &mut cx);
    cx.update(|_, cx| visual.update(cx, |visual, cx| visual.select(start, start + 5, cx)));
    cx.run_until_parked();
    click("app-menu", &mut cx);
    click("app-menu-Format", &mut cx);
    // Bold is the Format menu's first item.
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\nSome *plain* words.\n"
    );
}

/// A workspace window with tabs for four pages, `a.adoc` to `d.adoc`, in that order.
fn four_tabs(
    dir: &Path,
    cx: &mut TestAppContext,
) -> (
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
    Vec<adoc_engine::DocumentId>,
) {
    init(cx);
    let files: Vec<PathBuf> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| {
            let path = dir.join(format!("{name}.adoc"));
            std::fs::write(&path, format!("= {name}\n")).unwrap();
            path
        })
        .collect();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, files, cx);
    cx.run_until_parked();
    let ids = tab_documents(&app, &mut cx);
    assert_eq!(ids.len(), 4);
    (app, workspace, cx, ids)
}

fn tab_documents(
    app: &Entity<AppState>,
    cx: &mut VisualTestContext,
) -> Vec<adoc_engine::DocumentId> {
    cx.update(|_, cx| app.read(cx).engine.documents().map(|d| d.id).collect())
}

fn click_selector(selector: &'static str, button: gpui::MouseButton, cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"));
    cx.simulate_mouse_down(bounds.center(), button, gpui::Modifiers::none());
    cx.simulate_mouse_up(bounds.center(), button, gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn tabs_close_from_their_buttons_menu_and_keys(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (app, workspace, mut cx, ids) = four_tabs(dir.path(), cx);
    let [a, b, c, d] = ids[..] else {
        unreachable!()
    };

    // Each tab's X closes that tab.
    click_selector("tab-close-1", gpui::MouseButton::Left, &mut cx);
    assert_eq!(tab_documents(&app, &mut cx), vec![a, c, d]);
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 3);

    // The menu acts on the tab right-clicked, not the active one: Close Right on `a`.
    // Close, Close Others, then Close Right: Close Left is disabled on the first tab, and skipped.
    click_selector("editor-tab-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(tab_documents(&app, &mut cx), vec![a]);

    // The keys act on the active tab: Close Others keeps it; Close All closes the rest.
    let _ = (b, c, d);
    let [e, f] = open_more(&app, dir.path(), &["e", "f"], &mut cx)[..] else {
        unreachable!()
    };
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.engine.set_active(e);
            app.drain(cx);
        })
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("secondary-alt-t");
    cx.run_until_parked();
    assert_eq!(tab_documents(&app, &mut cx), vec![e]);
    let _ = f;
    cx.simulate_keystrokes("secondary-k secondary-w");
    cx.run_until_parked();
    assert!(tab_documents(&app, &mut cx).is_empty());
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 0);
}

#[gpui::test]
fn closing_tabs_with_unsaved_changes_asks_once(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (app, _workspace, mut cx, ids) = four_tabs(dir.path(), cx);
    for id in [ids[1], ids[2]] {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.invoke(
                    ids::EDIT_INSERT_TEXT,
                    CommandArgs::Text {
                        id,
                        range: SourceRange::new(0, 0),
                        text: "x".to_owned(),
                    },
                    cx,
                )
                .unwrap();
            })
        });
    }
    cx.run_until_parked();

    // Close Clean leaves the unsaved tabs without asking.
    cx.simulate_keystrokes("secondary-k u");
    cx.run_until_parked();
    assert_eq!(tab_documents(&app, &mut cx), vec![ids[1], ids[2]]);

    // Close All asks once about both; cancelling keeps them, confirming closes them.
    cx.simulate_keystrokes("secondary-k secondary-w");
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(tab_documents(&app, &mut cx), vec![ids[1], ids[2]]);
    cx.simulate_keystrokes("secondary-k secondary-w");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(tab_documents(&app, &mut cx).is_empty());
}

#[gpui::test]
fn the_tab_bar_s_arrows_go_back_and_forward(cx: &mut TestAppContext) {
    init(cx);
    let root = fixture("antora-multi-component");
    let page = root.join("alpha/modules/ROOT/pages/index.adoc");
    let mut engine = Engine::new(Settings::default().with_git_opt_in());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.run_until_parked();
    let (document, offset) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let id = engine.active().unwrap();
        let text = &engine.document(id).unwrap().parsed().text;
        (id, text.find("xref:beta::index.adoc").unwrap() + 6)
    });
    cx.update(|window, cx| {
        crate::workspace_view::follow_link(app.clone(), document, offset, window, cx);
    });
    cx.run_until_parked();
    let followed = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    assert_ne!(followed, document);

    click_selector("tab-bar-back", gpui::MouseButton::Left, &mut cx);
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.active()),
        Some(document)
    );
    click_selector("tab-bar-forward", gpui::MouseButton::Left, &mut cx);
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.active()),
        Some(followed)
    );
}

fn open_more(
    app: &Entity<AppState>,
    dir: &Path,
    names: &[&str],
    cx: &mut VisualTestContext,
) -> Vec<adoc_engine::DocumentId> {
    let ids = names
        .iter()
        .map(|name| {
            let path = dir.join(format!("{name}.adoc"));
            std::fs::write(&path, format!("= {name}\n")).unwrap();
            cx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    match app.invoke(ids::DOCUMENT_OPEN, CommandArgs::Path(path), cx) {
                        Ok(adoc_engine::CommandOutcome::Opened(id)) => id,
                        other => panic!("{other:?}"),
                    }
                })
            })
        })
        .collect();
    cx.run_until_parked();
    ids
}

#[gpui::test]
fn saving_over_a_change_on_disk_asks_to_reload_or_overwrite(cx: &mut TestAppContext) {
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&dir.path().join("page.adoc"));
    std::fs::write(&page, "= Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.run_until_parked();
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let edit = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.invoke(
                    ids::EDIT_INSERT_TEXT,
                    CommandArgs::Text {
                        id,
                        range: SourceRange::new(0, 0),
                        text: "mine ".to_owned(),
                    },
                    cx,
                )
                .unwrap();
            })
        });
        cx.run_until_parked();
    };

    // Reload takes the version on disk.
    edit(&mut cx);
    std::fs::write(&page, "= Theirs\n").unwrap();
    cx.simulate_keystrokes("secondary-s");
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    click_selector("save-conflict-reload", gpui::MouseButton::Left, &mut cx);
    assert_eq!(document_text(&app, &mut cx), "= Theirs\n");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "= Theirs\n");

    // Overwrite writes the buffer over it.
    edit(&mut cx);
    std::fs::write(&page, "= Theirs again\n").unwrap();
    cx.simulate_keystrokes("secondary-s");
    cx.run_until_parked();
    click_selector("save-conflict-overwrite", gpui::MouseButton::Left, &mut cx);
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "mine = Theirs\n");
    assert!(!cx.update(gpui_component::WindowExt::has_active_dialog));
}

/// A copy of the `preview-tabs` fixture's pages.
fn browsing_pages() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(fixture("preview-tabs")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), directory.path().join(entry.file_name())).unwrap();
    }
    directory
}

fn browsing_page(directory: &tempfile::TempDir, name: &str) -> PathBuf {
    adoc_index::normalize_path(&directory.path().join(format!("{name}.adoc")))
}

fn open_as(
    workspace: &Entity<WorkspaceView>,
    path: &Path,
    disposition: adoc_engine::OpenDisposition,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(path, SourceRange::default(), disposition, window, cx);
        });
    });
    cx.run_until_parked();
}

fn browse_to(workspace: &Entity<WorkspaceView>, path: &Path, cx: &mut VisualTestContext) {
    open_as(workspace, path, adoc_engine::OpenDisposition::Preview, cx);
}

/// The file names of the tabs, in order, with `*` after a preview tab's.
fn tab_names(
    app: &Entity<AppState>,
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        workspace
            .read(cx)
            .tab_documents(cx)
            .into_iter()
            .map(|id| {
                let name = engine.document(id).unwrap().path.file_stem().unwrap();
                let mark = if engine.is_preview(id) { "*" } else { "" };
                format!("{}{mark}", name.to_string_lossy())
            })
            .collect()
    })
}

fn double_click(selector: &'static str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let position = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    cx.simulate_mouse_move(position, None, gpui::Modifiers::none());
    for count in [1, 2] {
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position,
            modifiers: gpui::Modifiers::none(),
            click_count: count,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position,
            modifiers: gpui::Modifiers::none(),
            click_count: count,
        });
    }
    cx.run_until_parked();
}

#[gpui::test]
fn sidebar_selection_shows_one_preview_tab(cx: &mut TestAppContext) {
    init(cx);
    let root = fixture("antora-single-component");
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx
            .new(|cx| WorkspaceView::new(app.clone(), Some(root.clone()), Vec::new(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    // The component is row 0, and its navigation Home and Authentication rows 1 and 2.
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("nav-row-2").is_some()
    }));
    click("nav-row-1", &mut cx);
    click("nav-row-2", &mut cx);
    assert_eq!(tab_names(&app, &workspace, &mut cx), ["authentication*"]);

    // A double-click browses to the page, replacing the preview, and keeps it open.
    double_click("nav-row-1", &mut cx);
    assert_eq!(tab_names(&app, &workspace, &mut cx), ["index"]);
    click("nav-row-2", &mut cx);
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["index", "authentication*"]
    );
    click("nav-row-1", &mut cx);
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["index", "authentication*"],
        "browsing to an open page only activates it"
    );
}

#[gpui::test]
fn typing_keeps_a_preview_tab_open(cx: &mut TestAppContext) {
    init(cx);
    let pages = browsing_pages();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    browse_to(&workspace, &browsing_page(&pages, "installation"), &mut cx);
    assert_eq!(tab_names(&app, &workspace, &mut cx), ["installation*"]);
    cx.simulate_input("x");
    cx.run_until_parked();
    browse_to(&workspace, &browsing_page(&pages, "configuration"), &mut cx);
    browse_to(
        &workspace,
        &browsing_page(&pages, "troubleshooting"),
        &mut cx,
    );
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["installation", "troubleshooting*"]
    );
}

#[gpui::test]
fn preview_tabs_are_marked_and_kept_by_a_double_click(cx: &mut TestAppContext) {
    init(cx);
    let pages = browsing_pages();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    browse_to(&workspace, &browsing_page(&pages, "installation"), &mut cx);
    double_click("editor-tab-0", &mut cx);
    assert_eq!(tab_names(&app, &workspace, &mut cx), ["installation"]);
    let kept = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    browse_to(&workspace, &browsing_page(&pages, "configuration"), &mut cx);
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["installation", "configuration*"]
    );
    let id = cx.update(|_, cx| {
        app.read(cx)
            .engine
            .document_for_path(&browsing_page(&pages, "installation"))
    });
    assert_eq!(id, Some(kept), "kept without reopening");

    // The preview tab, second, says what it is.
    let tab = cx.debug_bounds("editor-tab-1").expect("a tab");
    cx.simulate_mouse_move(tab.center(), None, gpui::Modifiers::default());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("preview-tab-tooltip").is_some(),
        "a preview tab says what it is"
    );
}

#[gpui::test]
fn keep_open_runs_from_the_palette(cx: &mut TestAppContext) {
    init(cx);
    let pages = browsing_pages();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    browse_to(&workspace, &browsing_page(&pages, "installation"), &mut cx);
    cx.simulate_keystrokes("secondary-shift-p");
    cx.run_until_parked();
    cx.simulate_input("Keep Open");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(tab_names(&app, &workspace, &mut cx), ["installation"]);
}

#[gpui::test]
fn a_replaced_preview_keeps_its_tab_position(cx: &mut TestAppContext) {
    init(cx);
    let pages = browsing_pages();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (workspace, mut cx) =
        workspace_window(&app, vec![browsing_page(&pages, "introduction")], cx);
    cx.run_until_parked();
    browse_to(&workspace, &browsing_page(&pages, "installation"), &mut cx);
    open_as(
        &workspace,
        &browsing_page(&pages, "troubleshooting"),
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    browse_to(&workspace, &browsing_page(&pages, "configuration"), &mut cx);
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["introduction", "configuration*", "troubleshooting"]
    );
    let active = cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        workspace
            .active_editor_for_test()
            .map(|editor| editor.read(cx).document())
    });
    let configuration = cx.update(|_, cx| {
        app.read(cx)
            .engine
            .document_for_path(&browsing_page(&pages, "configuration"))
    });
    assert_eq!(active, configuration);
}

#[gpui::test]
fn without_preview_tabs_every_page_stays_open(cx: &mut TestAppContext) {
    init(cx);
    let pages = browsing_pages();
    let mut settings = Settings::default();
    settings.editor.preview_tabs = false;
    let app = app_with(Engine::new(settings), cx);
    let (workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    browse_to(&workspace, &browsing_page(&pages, "installation"), &mut cx);
    browse_to(&workspace, &browsing_page(&pages, "configuration"), &mut cx);
    assert_eq!(
        tab_names(&app, &workspace, &mut cx),
        ["installation", "configuration"]
    );
}

/// A workspace window on a copy of the `sidebar-expansion` fixture, its trees drawn.
fn expansion_workspace(
    settings: Settings,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    copy_tree(&fixture("sidebar-expansion"), directory.path());
    let root = adoc_index::normalize_path(directory.path());
    let app = app_with(Engine::new(settings), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx
            .new(|cx| WorkspaceView::new(app.clone(), Some(root.clone()), Vec::new(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("nav-row-1").is_some()
    }));
    (directory, app, workspace, cx)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

fn rows(
    workspace: &Entity<WorkspaceView>,
    panel: adoc_engine::SidebarPanel,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    cx.update(|_, cx| workspace.read(cx).tree_rows_for_test(panel, cx))
}

const NAV_AT_TWO: [&str; 10] = [
    "Guide 2.0 +",
    "  Home +",
    "    Install -",
    "    Install",
    "  FAQ",
    "Guide 1.0 +",
    "  Home +",
    "    Install -",
    "    Install",
    "  FAQ",
];

#[gpui::test]
fn sidebar_trees_open_two_levels_by_default(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    assert_eq!(rows(&workspace, Navigation, &mut cx), NAV_AT_TWO);
    // Component, then module, open; the families show, their resources do not.
    let modules = rows(&workspace, Modules, &mut cx);
    assert!(
        modules.iter().all(|row| row.matches("  ").count() <= 2),
        "{modules:#?}"
    );
    assert!(
        modules
            .iter()
            .any(|row| row.starts_with("    page") && row.ends_with('-')),
        "{modules:#?}"
    );
}

#[gpui::test]
fn sidebar_depth_follows_the_settings(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    // Opening the folder reloads the settings, so they come from a file.
    let settings_directory = tempfile::tempdir().unwrap();
    let user = settings_directory.path().join("settings.toml");
    std::fs::write(
        &user,
        "[sidebar.navigation]\ndefault_expansion = \"all\"\n\n\
         [sidebar.modules]\ndefault_expansion = 0\n",
    )
    .unwrap();
    let settings = Settings::load(Some(&user), None).unwrap();
    let (_directory, _app, workspace, mut cx) = expansion_workspace(settings, cx);
    let navigation = rows(&workspace, Navigation, &mut cx);
    assert!(
        navigation.contains(&"      Linux".to_owned()),
        "{navigation:#?}"
    );
    assert_eq!(
        rows(&workspace, Modules, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"]
    );
}

#[gpui::test]
fn a_collapsed_branch_survives_a_rebuild(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::Navigation;
    let (directory, app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    // Row 6 is version 1.0's Home.
    click("nav-row-6", &mut cx);
    let mut want: Vec<String> = NAV_AT_TWO.iter().map(|row| (*row).to_owned()).collect();
    want[6] = "  Home -".to_owned();
    want.drain(7..9);
    assert_eq!(rows(&workspace, Navigation, &mut cx), want);

    // A navigation entry added before it, the catalogue rebuilt: still closed.
    let nav = directory.path().join("guide-1.0/modules/ROOT/nav.adoc");
    let text = std::fs::read_to_string(&nav).unwrap();
    std::fs::write(&nav, format!("* xref:faq.adoc[First]\n{text}")).unwrap();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let _ = app.invoke(ids::WORKSPACE_REINDEX, CommandArgs::None, cx);
        })
    });
    assert!(drain_until(&app, &mut cx, |cx| {
        rows(&workspace, Navigation, cx).contains(&"  First".to_owned())
    }));
    let rows = rows(&workspace, Navigation, &mut cx);
    assert!(rows.contains(&"  Home -".to_owned()), "{rows:#?}");
    assert!(
        rows.contains(&"  Home +".to_owned()),
        "2.0's Home is its own: {rows:#?}"
    );
}

#[gpui::test]
fn the_panel_menu_acts_on_its_panel_only(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let modules = rows(&workspace, Modules, &mut cx);
    let tabs = cx.update(|_, cx| workspace.read(cx).tab_count());

    // Right-click a row: All of Navigation, then Expand All.
    click_selector("nav-row-1", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    let navigation = rows(&workspace, Navigation, &mut cx);
    assert!(
        navigation.contains(&"      Linux".to_owned()),
        "{navigation:#?}"
    );
    assert_eq!(
        rows(&workspace, Modules, &mut cx),
        modules,
        "Modules untouched"
    );
    assert_eq!(
        cx.update(|_, cx| workspace.read(cx).tab_count()),
        tabs,
        "no page opened"
    );

    // Collapse One Level, twice: Install, then Home.
    for _ in 0..2 {
        click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
        cx.simulate_keystrokes("down down down down enter");
        cx.run_until_parked();
    }
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        [
            "Guide 2.0 +",
            "  Home -",
            "  FAQ",
            "Guide 1.0 +",
            "  Home -",
            "  FAQ"
        ]
    );

    // Reset to Configured Default.
    click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("up enter");
    cx.run_until_parked();
    assert_eq!(rows(&workspace, Navigation, &mut cx), NAV_AT_TWO);
}

#[gpui::test]
fn the_panel_space_offers_the_menu_too(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::Navigation;
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let last = cx.debug_bounds("nav-row-9").expect("the last row");
    let below = last.bottom_left() + gpui::point(px(20.), px(40.));
    cx.simulate_mouse_down(below, gpui::MouseButton::Right, gpui::Modifiers::none());
    cx.simulate_mouse_up(below, gpui::MouseButton::Right, gpui::Modifiers::none());
    cx.run_until_parked();
    // Collapse All.
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"]
    );
}

#[gpui::test]
fn each_panel_keeps_its_branches_when_switching(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    click("nav-row-0", &mut cx);
    let navigation = rows(&workspace, Navigation, &mut cx);
    assert_eq!(navigation[0], "Guide 2.0 -");
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_modules_for_test(cx)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click("families-row-0", &mut cx);
    assert_eq!(rows(&workspace, Modules, &mut cx)[0], "Guide 2.0 -");
    assert_eq!(rows(&workspace, Navigation, &mut cx), navigation);
}

#[gpui::test]
fn collapsing_hands_the_selection_to_the_row_still_shown(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::Navigation;
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    // Expand All, then select 2.0's Linux, row 3.
    click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click("nav-row-3", &mut cx);
    let selected = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| workspace.read(cx).selected_row_for_test(Navigation, cx))
    };
    assert_eq!(selected(&mut cx).as_deref(), Some("Linux"));
    // Collapse One Level closes Install, hiding Linux: Install is selected.
    click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down down down enter");
    cx.run_until_parked();
    assert_eq!(selected(&mut cx).as_deref(), Some("Install"));
    // Collapse All: the version row.
    click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(selected(&mut cx).as_deref(), Some("Guide 2.0"));
}

#[gpui::test]
fn the_palette_opens_and_closes_the_panel_shown(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let modules = rows(&workspace, Modules, &mut cx);
    let run = |name: &str, cx: &mut VisualTestContext| {
        cx.simulate_keystrokes("secondary-shift-p");
        cx.run_until_parked();
        cx.simulate_input(name);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    };
    run("Collapse All in Sidebar", &mut cx);
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"]
    );
    assert_eq!(rows(&workspace, Modules, &mut cx), modules);

    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_modules_for_test(cx)));
    cx.run_until_parked();
    run("Expand All in Sidebar", &mut cx);
    let expanded = rows(&workspace, Modules, &mut cx);
    assert!(expanded.len() > modules.len(), "{expanded:#?}");
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"],
        "Navigation untouched"
    );
}

#[gpui::test]
fn shift_f10_opens_the_panel_menu(cx: &mut TestAppContext) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    let (_directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let modules = rows(&workspace, Modules, &mut cx);
    // Clicking the version row, which leads nowhere, focuses the tree and closes the row.
    click("nav-row-0", &mut cx);
    assert_eq!(rows(&workspace, Navigation, &mut cx)[0], "Guide 2.0 -");
    cx.simulate_keystrokes("shift-f10");
    cx.run_until_parked();
    // Expand All.
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    let navigation = rows(&workspace, Navigation, &mut cx);
    assert!(
        navigation.contains(&"      Linux".to_owned()),
        "{navigation:#?}"
    );
    assert_eq!(rows(&workspace, Modules, &mut cx), modules);
}

/// A workspace window on a mixed repository, its Files panel shown and its root listed.
fn files_workspace(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    PathBuf,
    Entity<AppState>,
    Entity<WorkspaceView>,
    Entity<crate::files_view::FilesView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    for (path, text) in [
        ("README.md", "# Readme\n"),
        (".adocfoundry.toml", "[editor]\n"),
        ("antora-playbook.yml", "site:\n  title: Docs\n"),
        ("docs/antora.yml", "name: docs\nversion: ~\n"),
        ("docs/modules/ROOT/pages/index.adoc", "= Index\n"),
        (
            "resources/themes/company-theme.yml",
            "base:\n  font-size: 10\n",
        ),
        ("node_modules/pad/index.js", "\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
    std::fs::copy(
        fixture("image-viewer/modules/ROOT/images/logo.png"),
        root.join("logo.png"),
    )
    .unwrap();
    std::fs::write(root.join("guide.pdf"), b"%PDF-1.7\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx
            .new(|cx| WorkspaceView::new(app.clone(), Some(root.clone()), Vec::new(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let files =
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_files_for_test(cx)));
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        files_rows(&files, cx)
            .iter()
            .any(|row| row.ends_with("README.md"))
    }));
    (directory, root, app, workspace, files, cx)
}

fn files_rows(
    files: &Entity<crate::files_view::FilesView>,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    cx.update(|_, cx| files.read(cx).rows_for_test(cx))
}

/// The index of the Files row labelled `label`.
fn files_row(
    files: &Entity<crate::files_view::FilesView>,
    label: &str,
    cx: &mut VisualTestContext,
) -> usize {
    let rows = files_rows(files, cx);
    rows.iter()
        .position(|row| row.trim_start() == label)
        .unwrap_or_else(|| panic!("no {label} in {rows:#?}"))
}

fn click_files_row(
    files: &Entity<crate::files_view::FilesView>,
    label: &str,
    count: usize,
    cx: &mut VisualTestContext,
) {
    let index = files_row(files, label, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let selector: &'static str = Box::leak(format!("files-row-{index}").into_boxed_str());
    let position = cx
        .debug_bounds(selector)
        .expect("the row is drawn")
        .center();
    cx.simulate_mouse_move(position, None, gpui::Modifiers::none());
    for click in 1..=count {
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position,
            modifiers: gpui::Modifiers::none(),
            click_count: click,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position,
            modifiers: gpui::Modifiers::none(),
            click_count: click,
        });
    }
    cx.run_until_parked();
}

#[gpui::test]
fn files_shows_the_folder_as_it_is_on_disk(cx: &mut TestAppContext) {
    let (_directory, root, app, _workspace, files, mut cx) = files_workspace(cx);
    let rows = files_rows(&files, &mut cx);
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        rows,
        [
            name.as_str(),
            "  docs",
            "  resources",
            "  .adocfoundry.toml",
            "  antora-playbook.yml",
            "  blob.bin",
            "  guide.pdf",
            "  logo.png",
            "  README.md",
        ],
        "folders first; dotfiles shown; node_modules excluded"
    );

    // Opening a folder lists it, and only it.
    click_files_row(&files, "docs", 1, &mut cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        files_rows(&files, cx).contains(&"    antora.yml".to_owned())
    }));
    let rows = files_rows(&files, &mut cx);
    assert!(rows.contains(&"    modules".to_owned()), "{rows:#?}");
    assert!(cx.update(|_, cx| app
        .read(cx)
        .engine
        .directory(&root.join("docs/modules"))
        .is_none()));

    // Show Excluded Files, from the panel's state.
    cx.update(|_, cx| files.update(cx, |files, cx| files.toggle_excluded(cx)));
    cx.run_until_parked();
    assert!(files_rows(&files, &mut cx).contains(&"  node_modules (excluded)".to_owned()));
}

#[gpui::test]
fn files_open_by_their_kind(cx: &mut TestAppContext) {
    let (_directory, root, app, workspace, files, mut cx) = files_workspace(cx);
    let tabs = |cx: &mut VisualTestContext| cx.update(|_, cx| workspace.read(cx).tab_count());

    // A YAML file opens as text, in a preview tab, outside the index.
    click_files_row(&files, "antora-playbook.yml", 1, &mut cx);
    let playbook = root.join("antora-playbook.yml");
    let (asciidoc, preview) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let id = engine.document_for_path(&playbook).expect("opened");
        (engine.document(id).unwrap().asciidoc, engine.is_preview(id))
    });
    assert!(!asciidoc && preview);
    assert_eq!(tabs(&mut cx), 1);

    // Double-clicking keeps it open.
    click_files_row(&files, "antora-playbook.yml", 2, &mut cx);
    let preview = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine.is_preview(engine.document_for_path(&playbook).unwrap())
    });
    assert!(!preview);

    // An image opens in an image tab, in the preview tab, outside Antora and the index.
    click_files_row(&files, "logo.png", 1, &mut cx);
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );
    let logo = root.join("logo.png");
    let (image, preview) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        assert!(engine.document_for_path(&logo).is_none());
        let id = engine.image_for_path(&logo).expect("opened");
        (id, engine.is_preview(id))
    });
    assert!(preview);
    assert_eq!(tabs(&mut cx), 2);
    // Double-clicking keeps it open, as for a page.
    click_files_row(&files, "logo.png", 2, &mut cx);
    assert!(!cx.update(|_, cx| app.read(cx).engine.is_preview(image)));

    // A PDF goes to the system; a binary file is refused: neither opens a tab.
    let opened = || crate::files_view::OPENED_EXTERNALLY.with(|opened| opened.borrow().clone());
    assert!(opened().is_empty(), "the image went nowhere else");
    click_files_row(&files, "guide.pdf", 1, &mut cx);
    assert_eq!(opened(), [root.join("guide.pdf")]);
    click_files_row(&files, "blob.bin", 1, &mut cx);
    assert_eq!(tabs(&mut cx), 2);
    assert!(cx.update(|_, cx| app
        .read(cx)
        .engine
        .document_for_path(&root.join("blob.bin"))
        .is_none()));
}

#[gpui::test]
fn reveal_in_files_opens_the_way_to_the_active_page(cx: &mut TestAppContext) {
    let (_directory, root, app, workspace, files, mut cx) = files_workspace(cx);
    let page = root.join("docs/modules/ROOT/pages/index.adoc");
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &page,
                SourceRange::default(),
                adoc_engine::OpenDisposition::Persistent,
                window,
                cx,
            );
        })
    });
    cx.run_until_parked();
    let focused = cx.update(|window, cx| window.focused(cx));
    cx.dispatch_action(crate::editor::bindings::RevealInFiles);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| files.read(cx).selected_for_test(cx)) == Some(page.clone())
    }));
    let rows = files_rows(&files, &mut cx);
    assert!(
        rows.contains(&"          index.adoc".to_owned()),
        "{rows:#?}"
    );
    assert_eq!(
        cx.update(|window, cx| window.focused(cx)),
        focused,
        "the editor keeps focus"
    );
}

#[gpui::test]
fn files_menu_copies_paths_and_collapses(cx: &mut TestAppContext) {
    let (_directory, root, app, workspace, files, mut cx) = files_workspace(cx);
    click_files_row(&files, "resources", 1, &mut cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        files_rows(&files, cx).contains(&"    themes".to_owned())
    }));
    click_files_row(&files, "themes", 1, &mut cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        files_rows(&files, cx).contains(&"      company-theme.yml".to_owned())
    }));

    // Copy Relative Path, the menu's fourth item for a file.
    let index = files_row(&files, "company-theme.yml", &mut cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let selector: &'static str = Box::leak(format!("files-row-{index}").into_boxed_str());
    click_selector(selector, gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down down down enter");
    cx.run_until_parked();
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some(
            std::path::Path::new("resources/themes/company-theme.yml")
                .display()
                .to_string()
                .as_str()
        )
    );
    assert_eq!(
        cx.update(|_, cx| workspace.read(cx).tab_count()),
        0,
        "nothing opened"
    );

    // A page opened elsewhere is selected when it is shown, without the panel taking focus.
    let theme = root.join("resources/themes/company-theme.yml");
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &theme,
                SourceRange::default(),
                adoc_engine::OpenDisposition::Persistent,
                window,
                cx,
            );
        })
    });
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| files.read(cx).selected_for_test(cx)),
        Some(theme)
    );

    // Collapse All leaves the root's row.
    cx.update(|_, cx| files.update(cx, |files, cx| files.collapse_all(cx)));
    cx.run_until_parked();
    assert_eq!(files_rows(&files, &mut cx).len(), 1);
}

// --- images ------------------------------------------------------------------------------------

/// A workspace window on a copy of the `image-viewer` fixture, and its images folder.
fn image_workspace(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    PathBuf,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    copy_tree(&fixture("image-viewer"), directory.path());
    let root = adoc_index::normalize_path(directory.path());
    let app = app_with(Engine::new(Settings::default()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx
            .new(|cx| WorkspaceView::new(app.clone(), Some(root.clone()), Vec::new(), window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| !app.read(cx).engine.has_pending())
    }));
    (
        directory,
        root.join("modules/ROOT/images"),
        app,
        workspace,
        cx,
    )
}

fn active_image(
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> Option<Entity<crate::image_view::ImageView>> {
    cx.update(|_, cx| workspace.read(cx).active_image_view().cloned())
}

/// Waits until the active tab's image has loaded, or failed to.
fn image_settled(
    app: &Entity<AppState>,
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> crate::image_view::LoadState {
    let mut state = None;
    assert!(drain_until(app, cx, |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        state = active_image(workspace, cx)
            .map(|view| cx.update(|_, cx| view.read(cx).state().clone()))
            .filter(|state| *state != crate::image_view::LoadState::Loading);
        state.is_some()
    }));
    state.unwrap()
}

fn image_info(
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> Option<crate::image_decode::ImageInfo> {
    let view = active_image(workspace, cx)?;
    cx.update(|_, cx| view.read(cx).info().cloned())
}

fn open_path(
    workspace: &Entity<WorkspaceView>,
    path: &Path,
    disposition: adoc_engine::OpenDisposition,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(path, SourceRange::default(), disposition, window, cx);
        })
    });
    cx.run_until_parked();
}

/// The index of the Modules row labelled `label`, once every branch is open.
fn modules_row(
    workspace: &Entity<WorkspaceView>,
    label: &str,
    cx: &mut VisualTestContext,
) -> usize {
    let rows = rows(workspace, adoc_engine::SidebarPanel::Modules, cx);
    rows.iter()
        .position(|row| row.trim() == label)
        .unwrap_or_else(|| panic!("no {label} in {rows:#?}"))
}

fn click_modules_row(
    workspace: &Entity<WorkspaceView>,
    label: &str,
    count: usize,
    cx: &mut VisualTestContext,
) {
    let index = modules_row(workspace, label, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let selector: &'static str = Box::leak(format!("families-row-{index}").into_boxed_str());
    let position = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    for click_count in 1..=count {
        cx.simulate_event(gpui::MouseDownEvent {
            position,
            button: gpui::MouseButton::Left,
            modifiers: gpui::Modifiers::none(),
            click_count,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            position,
            button: gpui::MouseButton::Left,
            modifiers: gpui::Modifiers::none(),
            click_count,
        });
    }
    cx.run_until_parked();
}

#[gpui::test]
fn images_open_in_the_viewer_from_modules_and_share_the_preview_tab(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_modules_for_test(cx)));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                ids::SIDEBAR_EXPAND_ALL,
                CommandArgs::Panel(adoc_engine::SidebarPanel::Modules),
                cx,
            )
            .unwrap();
        })
    });
    cx.run_until_parked();

    click_modules_row(&workspace, "logo.png", 1, &mut cx);
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );
    let info = image_info(&workspace, &mut cx).unwrap();
    assert_eq!((info.format, info.width, info.height), ("PNG", 24, 16));
    let logo = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        assert!(engine.document_for_path(&images.join("logo.png")).is_none());
        engine.image_for_path(&images.join("logo.png")).unwrap()
    });
    assert!(cx.update(|_, cx| app.read(cx).engine.is_preview(logo)));

    // The next image browsed to takes the preview tab's place.
    click_modules_row(&workspace, "screenshot.jpg", 1, &mut cx);
    image_settled(&app, &workspace, &mut cx);
    assert_eq!(image_info(&workspace, &mut cx).unwrap().format, "JPEG");
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 1);

    // A double-click keeps it; the next image opens beside it.
    click_modules_row(&workspace, "screenshot.jpg", 2, &mut cx);
    click_modules_row(&workspace, "logo.png", 1, &mut cx);
    image_settled(&app, &workspace, &mut cx);
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 2);

    // A page browsed to replaces the image in the preview tab too.
    click_modules_row(&workspace, "index.adoc", 1, &mut cx);
    let items = cx.update(|_, cx| workspace.read(cx).tab_items(cx));
    assert_eq!(items.len(), 2);
    assert!(items[0].image().is_some() && items[1].document().is_some());
}

#[gpui::test]
fn the_viewer_fits_zooms_and_keeps_a_chosen_zoom_through_a_resize(cx: &mut TestAppContext) {
    let (directory, _images, app, workspace, mut cx) = image_workspace(cx);
    // Wider than any window: it opens shrunk to fit, never past actual size.
    let wide = directory.path().join("wide.png");
    image::RgbaImage::from_pixel(6000, 200, image::Rgba([10, 20, 30, 255]))
        .save(&wide)
        .unwrap();
    open_path(
        &workspace,
        &wide,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    image_settled(&app, &workspace, &mut cx);
    let view = active_image(&workspace, &mut cx).unwrap();
    let percent =
        |cx: &mut VisualTestContext| cx.update(|window, cx| view.read(cx).zoom_percent(window));
    let initial = percent(&mut cx);
    assert!(initial < 100, "{initial}%");
    assert!(cx.debug_bounds("image-picture").is_some());
    assert!(!cx.update(|_, cx| view.read(cx).info().unwrap().has_alpha));

    // 100% is one image pixel to a screen pixel: half as wide as the image in points here,
    // where the test window's scale factor is two.
    click("image-actual-size", &mut cx);
    assert_eq!(percent(&mut cx), 100);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let picture = cx.debug_bounds("image-picture").unwrap();
    assert_eq!(picture.size.width, px(3000.));

    click("image-zoom-in", &mut cx);
    assert_eq!(percent(&mut cx), 125);
    cx.simulate_keystrokes("secondary--");
    cx.run_until_parked();
    assert_eq!(percent(&mut cx), 100, "the zoom keys work in the viewer");

    // A chosen zoom stays through a resize; Fit follows it.
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(percent(&mut cx), 100);
    click("image-fit", &mut cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let fitted = percent(&mut cx);
    cx.simulate_resize(size(px(1400.), px(900.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        percent(&mut cx) > fitted,
        "{fitted}% then {}%",
        percent(&mut cx)
    );
    assert_eq!(
        cx.update(|_, cx| view.read(cx).zoom()),
        crate::image_view::Zoom::Fit
    );
}

#[gpui::test]
fn a_transparent_image_says_so_and_a_small_one_is_not_enlarged(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    open_path(
        &workspace,
        &images.join("logo.png"),
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    image_settled(&app, &workspace, &mut cx);
    let view = active_image(&workspace, &mut cx).unwrap();
    assert!(cx.update(|_, cx| view.read(cx).info().unwrap().has_alpha));
    assert_eq!(
        cx.update(|window, cx| view.read(cx).zoom_percent(window)),
        100
    );
    let info = cx
        .debug_bounds("image-info")
        .expect("the information line is drawn");
    assert!(info.size.width > px(0.));
}

#[gpui::test]
fn a_damaged_image_says_why_and_retry_shows_it_once_mended(cx: &mut TestAppContext) {
    let (directory, images, app, workspace, mut cx) = image_workspace(cx);
    let picture = directory.path().join("picture.png");
    std::fs::copy(images.join("broken.png"), &picture).unwrap();
    open_path(
        &workspace,
        &picture,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );

    let state = image_settled(&app, &workspace, &mut cx);
    assert!(
        matches!(
            state,
            crate::image_view::LoadState::Failed(crate::image_decode::DecodeError::Corrupt(_))
        ),
        "{state:?}"
    );
    assert!(cx.debug_bounds("image-error").is_some());
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.documents().count()),
        0
    );

    std::fs::copy(images.join("logo.png"), &picture).unwrap();
    click("image-retry", &mut cx);
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );
    assert!(cx.debug_bounds("image-error").is_none());
}

#[gpui::test]
fn reload_shows_the_new_pixels_and_a_deleted_file_shows_stale(cx: &mut TestAppContext) {
    let (directory, images, app, workspace, mut cx) = image_workspace(cx);
    let picture = directory.path().join("picture.png");
    std::fs::copy(images.join("logo.png"), &picture).unwrap();
    open_path(
        &workspace,
        &picture,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    image_settled(&app, &workspace, &mut cx);
    assert_eq!(image_info(&workspace, &mut cx).unwrap().width, 24);

    // Other bytes at the same path, even another format behind the old name.
    std::fs::copy(images.join("screenshot.jpg"), &picture).unwrap();
    click("image-reload", &mut cx);
    image_settled(&app, &workspace, &mut cx);
    let info = image_info(&workspace, &mut cx).unwrap();
    assert_eq!((info.format, info.width, info.height), ("JPEG", 32, 20));

    std::fs::remove_file(&picture).unwrap();
    click("image-reload", &mut cx);
    let state = image_settled(&app, &workspace, &mut cx);
    assert!(matches!(
        state,
        crate::image_view::LoadState::Failed(crate::image_decode::DecodeError::Unreadable(_))
    ));
    assert!(
        cx.debug_bounds("image-stale").is_some(),
        "the last pixels are marked stale"
    );
    assert!(cx.debug_bounds("image-picture").is_some());
}

#[gpui::test]
fn an_older_load_never_replaces_a_newer_one(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    open_path(
        &workspace,
        &images.join("logo.png"),
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    image_settled(&app, &workspace, &mut cx);
    let view = active_image(&workspace, &mut cx).unwrap();
    let older = cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let older = view.generation();
            view.reload_for_test(cx);
            older
        })
    });
    // The older load's answer arrives late, and is dropped.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.finish_load(
                older,
                Err(crate::image_decode::DecodeError::Corrupt("late".to_owned())),
                cx,
            );
        })
    });
    assert_eq!(
        cx.update(|_, cx| view.read(cx).state().clone()),
        crate::image_view::LoadState::Loading
    );
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );
}

#[gpui::test]
fn closing_an_image_while_it_loads_leaves_no_tab(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &images.join("scan.tiff"),
                SourceRange::default(),
                adoc_engine::OpenDisposition::Persistent,
                window,
                cx,
            );
        });
        let id = app.read(cx).engine.active_image().unwrap();
        app.update(cx, |app, cx| {
            app.invoke(ids::DOCUMENT_CLOSE, CommandArgs::Image(id), cx)
                .unwrap();
        });
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 0);
    assert!(active_image(&workspace, &mut cx).is_none());
}

#[gpui::test]
fn text_commands_leave_pages_alone_while_an_image_is_shown(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    let page = images.join("../pages/index.adoc");
    open_path(
        &workspace,
        &page,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(0, 0),
                    text: "// edited\n".to_owned(),
                },
                cx,
            )
            .unwrap();
        })
    });
    let logo = images.join("logo.png");
    let before = std::fs::read(&logo).unwrap();
    open_path(
        &workspace,
        &logo,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    image_settled(&app, &workspace, &mut cx);

    cx.dispatch_action(crate::editor::bindings::Save);
    cx.dispatch_action(crate::editor::bindings::Undo);
    cx.run_until_parked();
    assert!(
        cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().dirty),
        "Save and Undo have no page to act on"
    );
    cx.dispatch_action(crate::editor::bindings::SaveAll);
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().dirty));
    assert_eq!(std::fs::read(&logo).unwrap(), before);
    assert!(
        cx.debug_bounds("editor-modes").is_none(),
        "no Source, Visual or Split"
    );
}

#[gpui::test]
fn unsupported_and_oddly_named_images_open_in_the_viewer(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    open_path(
        &workspace,
        &images.join("Photo Café.PNG"),
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );

    let heic = images.join("photo.heic");
    open_path(
        &workspace,
        &heic,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Failed(crate::image_decode::DecodeError::Unsupported(
            "HEIC".to_owned()
        ))
    );
    // Nothing went to the system until asked.
    let opened = || crate::files_view::OPENED_EXTERNALLY.with(|opened| opened.borrow().clone());
    assert!(opened().is_empty());
    click("image-error-open-externally", &mut cx);
    assert_eq!(opened(), [heic]);
}

#[gpui::test]
fn following_an_image_link_shows_the_image(cx: &mut TestAppContext) {
    let (_directory, images, app, workspace, mut cx) = image_workspace(cx);
    let page = images.join("../pages/index.adoc");
    open_path(
        &workspace,
        &page,
        adoc_engine::OpenDisposition::Persistent,
        &mut cx,
    );
    let (document, offset) = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let id = engine.active().unwrap();
        let text = engine.document(id).unwrap().text_string();
        (id, text.find("logo.png").unwrap() + 1)
    });
    cx.update(|window, cx| {
        crate::workspace_view::follow_link(app.clone(), document, offset, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        image_settled(&app, &workspace, &mut cx),
        crate::image_view::LoadState::Ready
    );
    assert_eq!(image_info(&workspace, &mut cx).unwrap().format, "PNG");
    // Back returns to the page.
    cx.dispatch_action(crate::editor::bindings::GoBack);
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.active()),
        Some(document)
    );
}

#[gpui::test]
fn diagnostics_show_the_report_and_copy_it(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.run_until_parked();

    cx.dispatch_action(crate::editor::bindings::ShowDiagnostics);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    assert!(cx.debug_bounds("diagnostics-report").is_some());

    click("diagnostics-copy", &mut cx);
    let copied = cx
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .unwrap();
    assert!(copied.starts_with("AdocFoundry diagnostics\n"), "{copied}");
    for line in [
        &format!("  Version:  AdocFoundry {}\n", env!("CARGO_PKG_VERSION")),
        "\nDisplay\n  Graphics:",
        "  Editor font:",
        "\nTools\n  asciidoctor:",
        "\nSettings\n",
    ] {
        assert!(copied.contains(line), "{line:?} is not in\n{copied}");
    }
}

#[gpui::test]
fn about_shows_the_version_and_each_components_licence(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.run_until_parked();

    cx.dispatch_action(crate::editor::bindings::ShowAbout);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    assert!(cx.debug_bounds("about-details").is_some());
    assert!(cx.debug_bounds("about-licence").is_some());

    let copied = |cx: &mut VisualTestContext| {
        click("about-copy", cx);
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .unwrap()
    };
    let own = copied(&mut cx);
    assert!(
        own.starts_with(&format!(
            "AdocFoundry {}\nLicence: Apache-2.0\n\nLICENSE\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{own}"
    );
    click("about-component-1", &mut cx);
    let first = copied(&mut cx);
    assert_ne!(first, own);
    assert!(
        first.lines().nth(1).unwrap().starts_with("Licence: "),
        "{first}"
    );
}

// A dialog that slides in on the wall clock moves between the frame a click is aimed at and
// the frame its release lands on, so a slow test run would miss the button.
#[gpui::test]
fn dialogs_take_clicks_however_late_they_come(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= A Page\n").unwrap();
    let app = app_with(Engine::new(Settings::default()), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page], cx);
    cx.run_until_parked();

    cx.dispatch_action(crate::editor::bindings::ShowDiagnostics);
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    click("diagnostics-copy", &mut cx);
    assert!(cx
        .update(|_, cx| cx.read_from_clipboard())
        .is_some_and(|item| item.text().is_some()));
}

/// The labels of the rows selected in Navigation and Modules.
fn followed(
    workspace: &Entity<WorkspaceView>,
    cx: &mut VisualTestContext,
) -> (Option<String>, Option<String>) {
    use adoc_engine::SidebarPanel::{Modules, Navigation};
    cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        (
            workspace.selected_row_for_test(Navigation, cx),
            workspace.selected_row_for_test(Modules, cx),
        )
    })
}

#[gpui::test]
fn sidebar_trees_follow_the_active_page(cx: &mut TestAppContext) {
    use adoc_engine::{
        OpenDisposition::Persistent,
        SidebarPanel::{Modules, Navigation},
    };
    let (directory, app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let root = adoc_index::normalize_path(directory.path());
    let pages = root.join("guide-2.0/modules/ROOT/pages");
    let selected_index = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| workspace.read(cx).selected_index_for_test(Navigation, cx))
    };

    // Linux sits under Install, closed at the default depth: the way to it opens.
    let linux = pages.join("linux.adoc");
    open_as(&workspace, &linux, Persistent, &mut cx);
    assert_eq!(
        followed(&workspace, &mut cx),
        (Some("Linux".to_owned()), Some("linux.adoc".to_owned()))
    );
    let navigation = rows(&workspace, Navigation, &mut cx);
    assert_eq!(
        navigation[..4],
        ["Guide 2.0 +", "  Home +", "    Install +", "      Linux"]
    );
    assert_eq!(selected_index(&mut cx), Some(3));
    assert!(
        rows(&workspace, Modules, &mut cx).contains(&"      linux.adoc".to_owned()),
        "{:#?}",
        rows(&workspace, Modules, &mut cx)
    );

    // Install is listed twice: the first entry is selected.
    open_as(&workspace, &pages.join("install.adoc"), Persistent, &mut cx);
    assert_eq!(selected_index(&mut cx), Some(2));
    // A click on the second keeps that one.
    open_as(&workspace, &pages.join("faq.adoc"), Persistent, &mut cx);
    assert_eq!(followed(&workspace, &mut cx).0.as_deref(), Some("FAQ"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click("nav-row-4", &mut cx);
    assert_eq!(selected_index(&mut cx), Some(4));
    assert_eq!(followed(&workspace, &mut cx).0.as_deref(), Some("Install"));

    // A page in no navigation file is followed in Modules only.
    let more = root.join("guide-2.0/modules/extra/pages/more.adoc");
    open_as(&workspace, &more, Persistent, &mut cx);
    assert_eq!(
        followed(&workspace, &mut cx),
        (None, Some("more.adoc".to_owned()))
    );

    // So is an image, in its family.
    let image = root.join("guide-1.0/modules/ROOT/images/diagram.svg");
    open_as(&workspace, &image, Persistent, &mut cx);
    assert_eq!(
        followed(&workspace, &mut cx),
        (None, Some("diagram.svg".to_owned()))
    );

    // Files has followed too, without being shown.
    let files =
        cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.show_files_for_test(cx)));
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| files.read(cx).selected_for_test(cx)) == Some(image.clone())
    }));
}

#[gpui::test]
fn a_branch_closed_over_the_active_page_stays_closed(cx: &mut TestAppContext) {
    use adoc_engine::{OpenDisposition::Persistent, SidebarPanel::Navigation};
    let (directory, _app, workspace, mut cx) = expansion_workspace(Settings::default(), cx);
    let pages = adoc_index::normalize_path(directory.path()).join("guide-2.0/modules/ROOT/pages");
    let linux = pages.join("linux.adoc");
    open_as(&workspace, &linux, Persistent, &mut cx);
    // Collapse All, then the same page again: nothing reopens.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click_selector("nav-row-0", gpui::MouseButton::Right, &mut cx);
    cx.simulate_keystrokes("down down down enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"]
    );
    open_as(&workspace, &linux, Persistent, &mut cx);
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 -", "Guide 1.0 -"]
    );
    // Another page opens the way to it.
    open_as(&workspace, &pages.join("faq.adoc"), Persistent, &mut cx);
    assert_eq!(
        rows(&workspace, Navigation, &mut cx),
        ["Guide 2.0 +", "  Home -", "  FAQ", "Guide 1.0 -"]
    );
    assert_eq!(followed(&workspace, &mut cx).0.as_deref(), Some("FAQ"));
}

#[gpui::test]
fn with_follow_active_off_the_trees_stay_put(cx: &mut TestAppContext) {
    use adoc_engine::{OpenDisposition::Persistent, SidebarPanel::Navigation};
    let settings_dir = tempfile::tempdir().unwrap();
    let user = settings_dir.path().join("settings.toml");
    std::fs::write(&user, "[sidebar]\nfollow_active = false\n").unwrap();
    let settings = Settings::load(Some(&user), None).unwrap();
    let (directory, _app, workspace, mut cx) = expansion_workspace(settings, cx);
    let before = rows(&workspace, Navigation, &mut cx);
    let linux = adoc_index::normalize_path(directory.path())
        .join("guide-2.0/modules/ROOT/pages/linux.adoc");
    open_as(&workspace, &linux, Persistent, &mut cx);
    assert_eq!(rows(&workspace, Navigation, &mut cx), before);
    assert_eq!(followed(&workspace, &mut cx), (None, None));
}

/// Settings, and the app's state, kept in `directory`, so recovery and local history have
/// somewhere to write.
fn settings_in(directory: &Path) -> Settings {
    let file = directory.join("adocfoundry.toml");
    if !file.exists() {
        std::fs::write(&file, "").unwrap();
    }
    Settings::load(Some(&file), None)
        .unwrap()
        .with_git_opt_in()
        .with_state_dir(Some(directory.join("state")))
}

#[gpui::test]
fn unsaved_edits_of_a_session_that_ended_are_offered_at_start_up(cx: &mut TestAppContext) {
    init(cx);
    let config = tempfile::tempdir().unwrap();
    let pages = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&pages.path().join("page.adoc"));
    std::fs::write(&page, "= Page\n").unwrap();
    // The session before ended with an edit unsaved.
    {
        let mut engine = Engine::new(settings_in(config.path()));
        engine.enable_recovery();
        let id = engine.open(&page).unwrap();
        engine
            .invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(7, 7),
                    text: "Unsaved.\n".to_owned(),
                },
            )
            .unwrap();
        engine.write_recovery_now();
    }

    let app = app_with(Engine::new(settings_in(config.path())), cx);
    let (_workspace, mut cx) = workspace_window(&app, Vec::new(), cx);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    assert!(cx.debug_bounds("recovery-item-0").is_some());
    assert!(cx.debug_bounds("recovery-diff").is_some());

    click("recovery-recover", &mut cx);
    assert!(!cx.update(gpui_component::WindowExt::has_active_dialog));
    assert_eq!(document_text(&app, &mut cx), "= Page\nUnsaved.\n");
    assert!(cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine.document(engine.active().unwrap()).unwrap().dirty
    }));
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "= Page\n");
}

#[gpui::test]
fn local_history_restores_a_saved_version(cx: &mut TestAppContext) {
    init(cx);
    let config = tempfile::tempdir().unwrap();
    let pages = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&pages.path().join("page.adoc"));
    std::fs::write(&page, "first\n").unwrap();
    let app = app_with(Engine::new(settings_in(config.path())), cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.run_until_parked();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let id = app.engine.active().unwrap();
            app.invoke(
                ids::EDIT_REPLACE_RANGE,
                CommandArgs::Text {
                    id,
                    range: SourceRange::new(0, 6),
                    text: "second\n".to_owned(),
                },
                cx,
            )
            .unwrap();
            app.invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id), cx)
                .unwrap();
        });
    });
    cx.run_until_parked();

    cx.dispatch_action(crate::editor::bindings::LocalHistory);
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    assert!(cx.debug_bounds("history-item-0").is_some());
    assert!(cx.debug_bounds("history-diff").is_some());
    // The older version: the file before the save.
    click("history-item-1", &mut cx);
    click("history-restore", &mut cx);
    assert!(!cx.update(gpui_component::WindowExt::has_active_dialog));
    assert_eq!(document_text(&app, &mut cx), "first\n");
    assert_eq!(std::fs::read_to_string(&page).unwrap(), "second\n");
}

#[gpui::test]
fn a_misspelt_word_is_underlined_and_replaced_in_one_undo_step(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("page.adoc");
    let text = "= Page\n\nWe recieve it.\n";
    std::fs::write(&path, text).unwrap();
    let mut engine = Engine::new(Settings::default());
    let document = engine.open(&path).unwrap();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, document, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    settle_engine(&app, &mut cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).spelling_hints_for_test(cx)),
        ["Unknown word: recieve"]
    );

    let start = text.find("recieve").unwrap();
    let choices =
        cx.update(|_, cx| crate::spelling::choices(&app.read(cx).engine, document, start + 2));
    assert_eq!(
        choices.last().map(|(label, _)| label.as_str()),
        Some("Add to Workspace Dictionary")
    );
    let (first, choice) = choices[0].clone();
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(start, start), window, cx)
        });
    });
    cx.dispatch_action(crate::spelling::ApplySpelling { document, choice });
    cx.run_until_parked();
    let replaced = format!("= Page\n\nWe {first} it.\n");
    assert_eq!(document_text(&app, &mut cx), replaced);
    assert_eq!(widget_text(&editor, &mut cx), replaced);

    cx.simulate_keystrokes("secondary-z");
    cx.run_until_parked();
    assert_eq!(document_text(&app, &mut cx), text);
}

#[gpui::test]
fn a_misspelt_word_in_the_visual_editor_is_underlined_and_replaced(cx: &mut TestAppContext) {
    let text = "= Page\n\nWe recieve it.\n";
    let (app, _workspace, visual, id, mut cx) = visual_workspace(text, cx);
    settle_engine(&app, &mut cx);
    let start = text.find("recieve").unwrap();
    let underlined = cx.update(|_, cx| {
        let visual = visual.read(cx);
        visual
            .shown()
            .map(|shown| visual.misspelt(shown, cx))
            .unwrap_or_default()
    });
    assert_eq!(
        underlined,
        [SourceRange::new(start, start + "recieve".len())]
    );

    let first = cx.update(|_, cx| {
        crate::spelling::choices(&app.read(cx).engine, id, start)[0]
            .0
            .clone()
    });
    // Right-click the word, then take the first item.
    let position = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        layouts
            .get(&vec![1])
            .expect("the paragraph is laid out")
            .position_for_index("We re".len())
            .expect("laid out")
            + gpui::point(px(1.), px(2.))
    });
    let button = gpui::MouseButton::Right;
    cx.simulate_event(gpui::MouseDownEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button,
        position,
        modifiers: gpui::Modifiers::none(),
        click_count: 1,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(
        text_of(&app, id, &mut cx),
        format!("= Page\n\nWe {first} it.\n")
    );
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(ids::EDIT_UNDO, CommandArgs::Document(id), cx)
        })
    })
    .unwrap();
    assert_eq!(text_of(&app, id, &mut cx), text);
}

#[gpui::test]
fn the_pdf_preview_shows_the_pages_and_a_click_goes_to_the_source(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    let text = "= Page\n\nFirst paragraph.\n\n<<<\n\nLast paragraph.\n";
    std::fs::write(&page, text).unwrap();
    let (app, workspace, mut cx) = previewing(vec![page.clone()], cx);
    cx.dispatch_action(crate::editor::bindings::TogglePdfPreview);
    let typeset = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            workspace
                .read(cx)
                .preview()
                .read(cx)
                .pdf_shown()
                .and_then(|shown| shown.pages.as_ref().ok().map(Vec::len))
                == Some(2)
        })
    };
    assert!(drain_until(&app, &mut cx, typeset), "the pages are shown");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // A click on the second page's line goes to its paragraph.
    let anchor = cx.update(|_, cx| {
        let preview = workspace.read(cx).preview().read(cx);
        let pages = preview.pdf_shown().unwrap().pages.clone().unwrap();
        pages[1]
            .anchors
            .iter()
            .find(|anchor| anchor.offset == text.find("Last paragraph").unwrap())
            .cloned()
            .expect("the paragraph's line")
    });
    let middle = ((anchor.top + anchor.bottom) / 2.0) as f32;
    workspace.update_in(&mut cx, |workspace, _, cx| {
        workspace
            .preview()
            .update(cx, |preview, cx| preview.pdf_scroll_to_page(1, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let position = cx
        .update(|_, cx| {
            workspace
                .read(cx)
                .preview()
                .read(cx)
                .pdf_position(1, middle)
        })
        .unwrap();
    cx.simulate_click(position, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        caret(&workspace, &mut cx),
        text.find("Last paragraph").unwrap()
    );

    // Back to the rendered page.
    cx.dispatch_action(crate::editor::bindings::TogglePdfPreview);
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| app.read(cx).engine.preview_layout().pdf));
}

/// The book fixture copied to `directory/book` with `settings` as its `.adocfoundry.toml`, opened
/// in a workspace window on `page`.
fn profile_workspace(
    directory: &Path,
    settings: &str,
    page: &str,
    cx: &mut TestAppContext,
) -> (
    PathBuf,
    Entity<AppState>,
    std::sync::mpsc::Receiver<adoc_engine::EngineEvent>,
    VisualTestContext,
) {
    let root = adoc_index::normalize_path(&directory.join("book"));
    copy_dir(&fixture("pdf/book"), &root);
    std::fs::write(root.join(".adocfoundry.toml"), settings).unwrap();
    let page = root.join(page);
    let mut engine_settings = Settings::load(None, Some(&root)).unwrap();
    engine_settings.git.enabled = false;
    let mut engine = Engine::new(engine_settings);
    let events = engine.subscribe();
    let app = app_with(engine, cx);
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![page.clone()],
                window,
                cx,
            )
        });
        Root::new(view, window, cx)
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| app.read(cx).engine.active().is_some())
    }));
    (root, app, events, cx)
}

#[gpui::test]
fn export_pdf_profile_writes_what_the_command_line_writes(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let (root, app, events, mut cx) = profile_workspace(
        directory.path(),
        "[pdf.profiles.install]\npage = \"guide/modules/ROOT/pages/install.adoc\"\noutput = \"build/install.pdf\"\ntheme = \"default-sans\"\n",
        "guide/modules/ROOT/pages/use.adoc",
        cx,
    );
    let output = root.join("build/install.pdf");
    // What the command line writes: the profile's arguments, exported and waited for.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            let args = app.engine.profile_export_args("install").unwrap();
            app.engine
                .export_blocking(args, std::time::Duration::from_secs(120))
                .unwrap();
        })
    });
    let from_cli = std::fs::read(&output).unwrap();
    std::fs::remove_file(&output).unwrap();
    let _ = events.try_iter().count();

    cx.dispatch_action(crate::editor::bindings::ExportPdfProfile);
    cx.run_until_parked();
    cx.simulate_input("install");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let mut finished = None;
    assert!(drain_until(&app, &mut cx, |_| {
        finished = finished.take().or_else(|| {
            events.try_iter().find_map(|event| match event {
                adoc_engine::EngineEvent::ExportFinished { output, result } => {
                    Some((output, result))
                }
                _ => None,
            })
        });
        finished.is_some()
    }));
    let (written, result) = finished.unwrap();
    assert_eq!(written, output);
    assert!(result.is_ok(), "{result:?}");
    assert!(
        std::fs::read(&output).unwrap() == from_cli,
        "the app's PDF differs from the command line's"
    );
}

#[gpui::test]
fn export_page_with_theme_offers_the_bundled_and_workspace_themes(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let (root, app, events, mut cx) = profile_workspace(
        directory.path(),
        "[pdf]\nthemes_dir = \"themes\"\n",
        "guide/modules/ROOT/pages/use.adoc",
        cx,
    );
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(
        root.join("themes/sans-theme.yml"),
        "extends: default-sans\nbase:\n  font-color: #112233\n",
    )
    .unwrap();
    cx.dispatch_action(crate::editor::bindings::ExportPagePdfWithTheme);
    cx.run_until_parked();
    cx.simulate_input("sans —");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let output = directory.path().join("themed.pdf");
    let result = finish_export(&app, &events, &output, &mut cx);
    assert!(result.is_ok(), "{result:?}");
    // The workspace theme extends the sans theme: the PDF embeds Noto Sans, not Noto Serif.
    let bytes = String::from_utf8_lossy(&std::fs::read(&output).unwrap()).into_owned();
    assert!(bytes.contains("NotoSans"), "not the sans theme");
    assert!(!bytes.contains("NotoSerif"), "the default theme was used");
}

/// The Antora workspace with two components, copied, with Alpha's page `page` open.
fn drag_workspace(
    page: &str,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    copy_dir(&fixture("antora-multi-component"), &root);
    let page = root.join("alpha/modules/ROOT/pages").join(page);
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![page.clone()],
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("nav-row-0").is_some()
            && cx.update(|_, cx| app.read(cx).engine.active().is_some())
    }));
    (directory, app, workspace, cx)
}

/// The selector of the Navigation row labelled `label`.
fn nav_row(
    workspace: &Entity<WorkspaceView>,
    label: &str,
    cx: &mut VisualTestContext,
) -> &'static str {
    let rows = cx.update(|_, cx| {
        workspace
            .read(cx)
            .tree_rows_for_test(adoc_engine::SidebarPanel::Navigation, cx)
    });
    let index = rows
        .iter()
        .position(|row| row.trim_start().trim_end_matches([' ', '+', '-']) == label)
        .unwrap_or_else(|| panic!("no {label} row in {rows:?}"));
    // Debug selectors are static.
    format!("nav-row-{index}").leak()
}

/// Where the caret goes at `offset` in the active source editor.
fn source_point(
    workspace: &Entity<WorkspaceView>,
    offset: usize,
    cx: &mut VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    cx.update(|_, cx| {
        let editor = workspace
            .read(cx)
            .active_editor_for_test()
            .unwrap()
            .read(cx);
        editor
            .state_for_test()
            .read(cx)
            .range_to_bounds(&(offset..offset))
    })
    .expect("the offset is laid out")
    .left_center()
        + gpui::point(px(1.), px(0.))
}

/// Drags the element drawn for `selector` to `to` with the mouse, and drops it there.
fn drag_to(selector: &'static str, to: gpui::Point<gpui::Pixels>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let from = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    let button = gpui::MouseButton::Left;
    cx.simulate_mouse_move(from, None, gpui::Modifiers::none());
    cx.simulate_mouse_down(from, button, gpui::Modifiers::none());
    cx.simulate_mouse_move(
        from + gpui::point(px(20.), px(0.)),
        Some(button),
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_move(to, Some(button), gpui::Modifiers::none());
    cx.simulate_mouse_up(to, button, gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn dragging_a_page_into_the_source_editor_inserts_an_xref(cx: &mut TestAppContext) {
    let (_directory, app, workspace, mut cx) = drag_workspace("index.adoc", cx);
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let text = text_of(&app, id, &mut cx);
    // The end of the line that links to Beta.
    let at = text.find("\n\nThe xref").unwrap();
    let editor = cx
        .update(|_, cx| workspace.read(cx).active_editor_for_test().cloned())
        .expect("a source editor");
    let to = source_point(&workspace, at, &mut cx);
    let row = nav_row(&workspace, "Beta home", &mut cx);
    drag_to(row, to, &mut cx);

    let xref = "xref:beta::index.adoc[Beta]";
    assert_eq!(
        text_of(&app, id, &mut cx),
        format!("{}{xref}{}", &text[..at], &text[at..])
    );
    assert_eq!(
        cx.update(|_, cx| editor.read(cx).cursor(cx)),
        at + xref.len(),
        "the caret follows the xref"
    );
    assert_eq!(
        cx.update(|_, cx| app.read(cx).engine.active()),
        Some(id),
        "a drag opens nothing"
    );
}

#[gpui::test]
fn dragging_a_page_into_the_visual_editor_inserts_an_xref(cx: &mut TestAppContext) {
    let (_directory, app, workspace, mut cx) = drag_workspace("index.adoc", cx);
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    cx.simulate_keystrokes("secondary-k w");
    cx.run_until_parked();
    let visual = cx
        .update(|_, cx| workspace.read(cx).visual_for_test(id).cloned())
        .expect("a visual editor");
    let text = text_of(&app, id, &mut cx);
    // Before "See", at the start of the first paragraph.
    let at = text.find("See").unwrap();
    let to = cx.update(|_, cx| {
        let layouts = visual.read(cx).layouts().borrow();
        layouts
            .get(&vec![1])
            .expect("the paragraph is laid out")
            .position_for_index(0)
            .expect("its start is laid out")
            + gpui::point(px(1.), px(2.))
    });
    let row = nav_row(&workspace, "reference.adoc", &mut cx);
    drag_to(row, to, &mut cx);

    assert_eq!(
        text_of(&app, id, &mut cx),
        format!(
            "{}xref:beta:api:reference.adoc[API Reference]{}",
            &text[..at],
            &text[at..]
        )
    );
    // Typing goes on after the link.
    cx.simulate_keystrokes("space");
    cx.run_until_parked();
    assert!(text_of(&app, id, &mut cx).contains("[API Reference] See"));
}

#[gpui::test]
fn dragging_an_outline_section_inserts_a_reference_to_it(cx: &mut TestAppContext) {
    let (_directory, app, workspace, mut cx) = drag_workspace("guide.adoc", cx);
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let text = text_of(&app, id, &mut cx);
    let at = text.find("Set it up.").unwrap() + "Set it up.".len();
    let to = source_point(&workspace, at, &mut cx);
    // The page's title is row 0, its Setup section row 1.
    drag_to("outline-row-1", to, &mut cx);
    assert_eq!(
        text_of(&app, id, &mut cx),
        format!("{}<<setup,Setup>>{}", &text[..at], &text[at..])
    );

    // The page's own title links nowhere from the page.
    let text = text_of(&app, id, &mut cx);
    drag_to(
        "outline-row-0",
        source_point(&workspace, 0, &mut cx),
        &mut cx,
    );
    assert_eq!(text_of(&app, id, &mut cx), text);
}

/// Two components, copied: Alpha's guide declares `setup`, and pages in both name it.
fn anchored_workspace(
    page: &str,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("alpha/antora.yml", "name: alpha\nversion: '1.0'\n");
    write(
        "alpha/modules/ROOT/pages/guide.adoc",
        "= Guide\n\n[[setup]]\n== Setup\n\nSee <<setup>>.\n",
    );
    write(
        "alpha/modules/ROOT/pages/index.adoc",
        "= Alpha\n\nStart with xref:guide.adoc#setup[setting up].\n",
    );
    write("beta/antora.yml", "name: beta\nversion: ~\n");
    write(
        "beta/modules/ROOT/pages/index.adoc",
        "= Beta\n\nRead xref:alpha::guide.adoc#setup[Alpha's set-up].\n",
    );
    let page = root.join(page);
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![page.clone()],
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| {
            app.read(cx).engine.active().is_some() && !app.read(cx).engine.is_indexing()
        })
    }));
    (directory, app, workspace, cx)
}

/// Puts the source caret at `offset` of the active page.
fn source_caret(workspace: &Entity<WorkspaceView>, offset: usize, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(offset, offset), window, cx)
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn find_references_lists_them_by_file_and_opens_one(cx: &mut TestAppContext) {
    let (_directory, app, workspace, mut cx) =
        anchored_workspace("alpha/modules/ROOT/pages/guide.adoc", cx);
    let guide = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let offset = text_of(&app, guide, &mut cx).find("setup]]").unwrap();
    source_caret(&workspace, offset, &mut cx);

    cx.simulate_keystrokes("alt-shift-f12");
    cx.run_until_parked();
    let rows = cx.update(|_, cx| workspace.read(cx).references_for_test(cx));
    assert_eq!(
        rows,
        [
            "3 references in 3 files",
            "alpha/modules/ROOT/pages/guide.adoc",
            "6  See <<setup>>.",
            "alpha/modules/ROOT/pages/index.adoc",
            "3  Start with xref:guide.adoc#setup[setting up].",
            "beta/modules/ROOT/pages/index.adoc",
            "3  Read xref:alpha::guide.adoc#setup[Alpha's set-up].",
        ]
    );

    // A click opens the page at the reference.
    click("reference-2", &mut cx);
    let opened = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        engine
            .document(engine.active().unwrap())
            .unwrap()
            .path
            .clone()
    });
    assert!(
        opened.ends_with("beta/modules/ROOT/pages/index.adoc"),
        "{opened:?}"
    );
}

#[gpui::test]
fn go_to_symbol_opens_a_section_in_another_component(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("alpha/antora.yml", "name: alpha\nversion: '1.0'\n");
    write(
        "alpha/modules/ROOT/pages/index.adoc",
        "= Alpha\n\nWelcome.\n",
    );
    write("beta/antora.yml", "name: beta\nversion: ~\n");
    let guide = "= Guide\n\n== Before\n\nFirst.\n\n== Installing the Server\n\nThen.\n";
    write("beta/modules/admin/pages/guide.adoc", guide);
    let alpha = root.join("alpha/modules/ROOT/pages/index.adoc");
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![alpha.clone()],
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| {
            app.read(cx).engine.active().is_some() && !app.read(cx).engine.is_indexing()
        })
    }));
    let active_path = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let engine = &app.read(cx).engine;
            engine
                .document(engine.active().unwrap())
                .unwrap()
                .path
                .clone()
        })
    };
    let caret = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            workspace
                .read(cx)
                .active_editor_for_test()
                .unwrap()
                .read(cx)
                .cursor(cx)
        })
    };

    cx.simulate_keystrokes("secondary-t");
    cx.run_until_parked();
    cx.simulate_input("instal serv");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        active_path(&mut cx),
        root.join("beta/modules/admin/pages/guide.adoc")
    );
    assert_eq!(caret(&mut cx), guide.find("Installing").unwrap());

    // In the page, with the page's own symbols.
    cx.simulate_keystrokes("secondary-r");
    cx.run_until_parked();
    cx.simulate_input("bef");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(caret(&mut cx), guide.find("Before").unwrap());

    // Back returns to the section gone to first, then to Alpha.
    cx.simulate_keystrokes("ctrl--");
    cx.run_until_parked();
    assert_eq!(caret(&mut cx), guide.find("Installing").unwrap());
    cx.simulate_keystrokes("ctrl--");
    cx.run_until_parked();
    assert_eq!(active_path(&mut cx), alpha);
}

#[gpui::test]
fn f2_on_a_heading_renames_its_section_id(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_index::normalize_path(directory.path());
    let page = root.join("page.adoc");
    let text = "= Page\n\n== First Steps\n\nSee <<_first_steps>>.\n";
    std::fs::write(&page, text).unwrap();
    std::fs::write(
        root.join("other.adoc"),
        "= Other\n\nRead xref:page.adoc#_first_steps[the first steps].\n",
    )
    .unwrap();
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let mut workspace = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| {
            WorkspaceView::new(
                app.clone(),
                Some(root.clone()),
                vec![page.clone()],
                window,
                cx,
            )
        });
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| {
            app.read(cx).engine.active().is_some() && !app.read(cx).engine.is_indexing()
        })
    }));
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    let heading = text.find("First").unwrap();
    cx.update(|window, cx| {
        let editor = workspace.read(cx).active_editor_for_test().unwrap().clone();
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(heading, heading), window, cx)
        });
    });
    cx.run_until_parked();

    // F2 asks for the new id, starting from the generated one; Enter previews, Enter applies.
    cx.simulate_keystrokes("f2");
    cx.run_until_parked();
    assert!(cx.update(gpui_component::WindowExt::has_active_dialog));
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("begin");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    assert_eq!(
        text_of(&app, id, &mut cx),
        "= Page\n\n[[begin]]\n== First Steps\n\nSee <<begin>>.\n"
    );
    let other = cx.update(|_, cx| {
        let engine = &app.read(cx).engine;
        let other = engine.document_for_path(&root.join("other.adoc")).unwrap();
        engine.document(other).unwrap().text_string()
    });
    assert_eq!(
        other,
        "= Other\n\nRead xref:page.adoc#begin[the first steps].\n"
    );
}

/// A page open in the workspace, with every start-up event drained, for counting renders.
fn settled_workspace(
    text: &str,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<AppState>,
    Entity<WorkspaceView>,
    VisualTestContext,
) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = adoc_index::normalize_path(&directory.path().join("page.adoc"));
    std::fs::write(&page, text).unwrap();
    let app = app_with(Engine::new(Settings::default().with_git_opt_in()), cx);
    let (workspace, mut cx) = workspace_window(&app, vec![page], cx);
    assert!(drain_until(&app, &mut cx, |cx| {
        cx.update(|_, cx| workspace.read(cx).active_editor_for_test().is_some())
    }));
    settle(&app, &mut cx);
    (directory, app, workspace, cx)
}

/// Lets every debounce run out and drains what it produced.
fn settle(app: &Entity<AppState>, cx: &mut VisualTestContext) {
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cx.update(|_, cx| app.update(cx, |app, cx| app.drain(cx)));
        cx.run_until_parked();
    }
}

#[gpui::test]
fn typing_renders_the_workspace_once_per_keystroke(cx: &mut TestAppContext) {
    use crate::renders::{self, View};
    let (_directory, app, _workspace, mut cx) = settled_workspace("= Page\n\nSome words.\n", cx);
    renders::reset();
    let mut each = Vec::new();
    for key in "a b c d e f g h i j".split(' ') {
        let before = renders::count(View::Workspace);
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        each.push(renders::count(View::Workspace) - before);
    }
    let typed = renders::count(View::Workspace);
    settle(&app, &mut cx);
    let settled = renders::count(View::Workspace) - typed;
    assert!(document_text(&app, &mut cx).contains("abcdefghij"));
    // Once per keystroke, for the caret's line and column in the status bar, and once more as
    // the first one marks the page's tab unsaved.
    assert_eq!(each, [2, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert!(
        settled <= 4,
        "the settles rendered the workspace {settled} times"
    );
    // The editor redraws for its own text, not for the settles.
    assert!(
        renders::count(View::Editor) <= 12,
        "{}",
        renders::count(View::Editor)
    );
}

#[gpui::test]
fn word_count_change_does_not_redraw_the_editor(cx: &mut TestAppContext) {
    use crate::renders::{self, View};
    let (_directory, app, _workspace, mut cx) = settled_workspace("= Page\n\nSome words.\n", cx);
    let id = cx.update(|_, cx| app.read(cx).engine.active().unwrap());
    renders::reset();
    cx.update(|_, cx| {
        app.update(cx, |_, cx| {
            cx.emit(adoc_engine::EngineEvent::WordCountChanged(id))
        })
    });
    cx.run_until_parked();
    eprintln!(
        "word count: workspace {}, editor {}",
        renders::count(View::Workspace),
        renders::count(View::Editor)
    );
    // The status bar shows it; the editor does not.
    assert!(renders::count(View::Workspace) <= 1);
    assert_eq!(renders::count(View::Editor), 0);

    // An event no part of the workspace window shows draws nothing.
    renders::reset();
    cx.update(|_, cx| {
        app.update(cx, |_, cx| {
            cx.emit(adoc_engine::EngineEvent::DirectoryListed(
                std::path::PathBuf::from("/elsewhere"),
            ))
        })
    });
    cx.run_until_parked();
    assert_eq!(renders::count(View::Workspace), 0);
    assert_eq!(renders::count(View::Editor), 0);
}

/// The visual editor draws the engine's misspellings, so a spelling change redraws it; an event
/// it does not draw, such as a word count, does not.
#[gpui::test]
fn the_visual_editor_redraws_for_spelling_but_not_word_counts(cx: &mut TestAppContext) {
    use crate::renders::{self, View};
    let (app, _, _visual, id, mut cx) = visual_workspace("= Page\n\nSome wrods here.\n", cx);
    cx.run_until_parked();
    renders::reset();
    cx.update(|_, cx| {
        app.update(cx, |_, cx| {
            cx.emit(adoc_engine::EngineEvent::WordCountChanged(id))
        })
    });
    cx.run_until_parked();
    assert_eq!(renders::count(View::Visual), 0);
    cx.update(|_, cx| {
        app.update(cx, |_, cx| {
            cx.emit(adoc_engine::EngineEvent::SpellingChanged(id))
        })
    });
    cx.run_until_parked();
    assert_eq!(renders::count(View::Visual), 1);
}

#[gpui::test]
fn a_restored_session_s_pages_open_after_the_first_draw(cx: &mut TestAppContext) {
    init(cx);
    let config = tempfile::tempdir().unwrap();
    let pages = tempfile::tempdir().unwrap();
    let one = adoc_index::normalize_path(&pages.path().join("one.adoc"));
    let two = adoc_index::normalize_path(&pages.path().join("two.adoc"));
    std::fs::write(&one, "= One\n\nThe first page.\n").unwrap();
    std::fs::write(&two, "= Two\n\nThe second page.\n").unwrap();
    let caret = "= Two\n\nThe second".len();
    // The session before ended with the source page active and the visual one behind it.
    {
        let mut engine = Engine::new(settings_in(config.path()));
        engine.enable_sessions();
        let one = engine.open(&one).unwrap();
        let two = engine.open(&two).unwrap();
        engine
            .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(two))
            .unwrap();
        engine.set_active(one);
        engine
            .write_session(&std::collections::HashMap::from([(one, 0), (two, caret)]))
            .unwrap();
    }

    let app = app_with(Engine::new(settings_in(config.path())), cx);
    let mut workspace = None;
    let mut opened_while_built = None;
    let window = cx.open_window(size(px(1400.), px(900.)), |window, cx| {
        let view = cx.new(|cx| WorkspaceView::new(app.clone(), None, Vec::new(), window, cx));
        // GPUI draws the window as soon as it is built; nothing of the session is open yet.
        opened_while_built = Some(app.read(cx).engine.tab_order().len());
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    let workspace = workspace.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    assert_eq!(opened_while_built, Some(0));
    assert_eq!(cx.update(|_, cx| workspace.read(cx).tab_count()), 2);

    // The visual page behind has no model until it is shown; then its caret comes back.
    let two = cx.update(|_, cx| app.read(cx).engine.document_for_path(&two).unwrap());
    assert!(cx.update(|_, cx| app.read(cx).engine.visual(two).is_none()));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.engine.set_active(two);
            app.drain(cx);
        });
    });
    cx.run_until_parked();
    let head = cx.update(|_, cx| {
        let visual = workspace.read(cx).visual_for_test(two).unwrap();
        visual.read(cx).selection().head
    });
    assert_eq!(head, caret);
}

/// A page whose earlier line changes on disk while the caret is on its last.
const BEFORE_RELOAD: &str = "= Page\n\nFirst.\n\nLast.\n";

fn reload_with_an_earlier_line_changed(
    app: &Entity<AppState>,
    id: adoc_engine::DocumentId,
    cx: &mut VisualTestContext,
) -> String {
    let path = cx.update(|_, cx| app.read(cx).engine.document(id).unwrap().path.clone());
    let after = BEFORE_RELOAD.replace("First.", "First, changed.");
    std::fs::write(&path, &after).unwrap();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.invoke(ids::DOCUMENT_RELOAD, CommandArgs::Document(id), cx)
                .unwrap();
        });
    });
    cx.run_until_parked();
    after
}

#[gpui::test]
fn caret_survives_an_edit_elsewhere(cx: &mut TestAppContext) {
    init(cx);
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, BEFORE_RELOAD).unwrap();
    let mut engine = Engine::new(Settings::default());
    let id = engine.open(&page).unwrap();
    let app = app_with(engine, cx);
    let (editor, window) = editor_window(&app, id, cx);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let end = BEFORE_RELOAD.len() - 1;
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| {
            editor.reveal(SourceRange::new(end, end), window, cx);
        });
    });
    cx.run_until_parked();

    let after = reload_with_an_earlier_line_changed(&app, id, &mut cx);
    assert_eq!(widget_text(&editor, &mut cx), after);
    let moved = end + ", changed".len();
    assert_eq!(cx.update(|_, cx| editor.read(cx).cursor(cx)), moved);
}

#[gpui::test]
fn visual_caret_survives_an_edit_elsewhere(cx: &mut TestAppContext) {
    let (app, _, visual, id, mut cx) = visual_workspace(BEFORE_RELOAD, cx);
    let end = BEFORE_RELOAD.len() - 1;
    place_caret(&visual, end, &mut cx);

    reload_with_an_earlier_line_changed(&app, id, &mut cx);
    let selection = cx.update(|_, cx| visual.read(cx).selection());
    let moved = end + ", changed".len();
    assert_eq!((selection.anchor, selection.head), (moved, moved));
}
