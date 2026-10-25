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
        cx.update(|_, cx| app.read(cx).engine.active().is_some())
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
            .iter()
            .find(|((_, language, text), _)| language == "rust" && text == code)
            .map(|(_, styles)| styles.len())
    });
    assert!(styles.is_some_and(|count| count > 0), "{styles:?}");
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
            workspace.open_at(&path, SourceRange::new(heading, heading), window, cx);
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
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
    let app = app_with(engine, cx);
    let (_workspace, mut cx) = workspace_window(&app, vec![page.clone()], cx);
    cx.run_until_parked();
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
