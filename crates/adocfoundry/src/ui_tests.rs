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
    let (trail, _) = block_for(&workspace, &page, &text, "Paragraph 20.", &mut cx);
    let extent = cx.update(|_, cx| preview.read(cx).extent(&trail)).unwrap();
    cx.update(|_, cx| {
        preview.update(cx, |preview, cx| preview.scroll_for_test(extent.top, cx));
    });
    cx.run_until_parked();
    let line = text[..text.find("Paragraph 20.").unwrap()]
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
