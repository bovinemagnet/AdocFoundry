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
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
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
        let text = &engine.document(id).unwrap().parsed.text;
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
    let mut engine = Engine::new(Settings::default());
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

/// Drains the engine until `done` holds: preview rendering runs on worker threads.
fn drain_until(
    app: &Entity<AppState>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    for _ in 0..400 {
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
            workspace.open_at(&second, SourceRange::default(), window, cx);
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
