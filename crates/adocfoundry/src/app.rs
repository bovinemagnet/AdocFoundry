//! Application state: the engine, its event pump, and window creation.

use std::{path::PathBuf, sync::mpsc::Receiver, time::Duration};

use adoc_engine::{
    CommandArgs, CommandId, CommandOutcome, Engine, EngineError, EngineEvent, WindowPlacement,
};
use gpui::{
    point, px, size, AnyView, App, AppContext as _, Bounds, Context, Entity, EventEmitter, Pixels,
    Task, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::Root;

use crate::{
    graph_view::{GraphMode, GraphView},
    mirror_view::MirrorView,
    preview::PreviewView,
    workspace_view::WorkspaceView,
};

/// How often the engine is pumped while work is pending, and while idle.
const BUSY_TICK: Duration = Duration::from_millis(16);
const IDLE_TICK: Duration = Duration::from_millis(100);

/// The id Linux desktops match a window to its desktop entry by, and so find its icon: the
/// Wayland app id and the X11 `WM_CLASS`. It is the desktop entry's name and the Flatpak id.
const APP_ID: &str = "io.github.bovinemagnet.AdocFoundry";

/// The single owner of the engine. Views hold an `Entity<AppState>`, invoke commands through it,
/// and subscribe to the engine events it re-emits.
pub struct AppState {
    pub engine: Engine,
    /// How the releases feed is read: the system's `curl`, or a stand-in in tests.
    pub fetch: crate::updates::Fetch,
    events: Receiver<EngineEvent>,
    _pump: Task<()>,
    _quit: gpui::Subscription,
}

impl EventEmitter<EngineEvent> for AppState {}

impl AppState {
    pub fn new(mut engine: Engine, cx: &mut Context<Self>) -> Self {
        // The app shows documents as tabs, so browsing may reuse a preview tab.
        engine.enable_preview_tabs();
        // The status bar shows the active page's words.
        engine.enable_word_count();
        // Both editors underline misspelt words.
        engine.enable_spelling();
        // Unsaved edits survive a crash, and saved versions are kept to restore.
        engine.enable_recovery();
        engine.enable_local_history();
        // The last session and the folders opened recently are kept for the next launch.
        engine.enable_sessions();
        // The workspace's problems, for the Problems panel, kept by the engine off the render.
        engine.enable_problems();
        let events = engine.subscribe();
        let pump = cx.spawn(async move |this, cx| loop {
            let busy = this
                .update(cx, |this, cx| {
                    this.drain(cx);
                    this.engine.has_pending()
                })
                .unwrap_or(false);
            let tick = if busy { BUSY_TICK } else { IDLE_TICK };
            cx.background_executor().timer(tick).await;
            if this.upgrade().is_none() {
                break;
            }
        });
        // Edits made in the last moments before quitting are kept too.
        let quit = cx.on_app_quit(|this, _| {
            this.engine.write_recovery_now();
            async {}
        });
        Self {
            engine,
            fetch: crate::updates::system_fetch(),
            events,
            _pump: pump,
            _quit: quit,
        }
    }

    /// Applies pending worker and watcher results and re-emits the engine's events. Each view
    /// redraws itself for the events it shows, so nothing is redrawn here; an event that says
    /// only that something is ready to read again is passed on once however often it came.
    pub fn drain(&mut self, cx: &mut Context<Self>) {
        self.drain_answered(None, cx);
    }

    /// Drains as [`AppState::drain`] does, leaving out the failure of `answered`, which its
    /// caller answers itself.
    fn drain_answered(&mut self, answered: Option<CommandId>, cx: &mut Context<Self>) {
        self.engine.pump();
        let mut events: Vec<EngineEvent> = Vec::new();
        for event in self.events.try_iter() {
            if matches!(&event, EngineEvent::CommandFailed { command, .. } if Some(*command) == answered)
            {
                continue;
            }
            if !(is_idempotent(&event) && events.contains(&event)) {
                events.push(event);
            }
        }
        for event in events {
            cx.emit(event);
        }
    }

    /// Invokes an engine command and publishes the events it caused before returning. A
    /// failure is reported once, by the window, through `EngineEvent::CommandFailed`: callers
    /// do not show the error they get back. A save conflict is the exception, answered by its
    /// caller with a dialog.
    pub fn invoke(
        &mut self,
        command: CommandId,
        args: CommandArgs,
        cx: &mut Context<Self>,
    ) -> Result<CommandOutcome, EngineError> {
        let outcome = self.engine.invoke(command, args);
        let answered = matches!(outcome, Err(EngineError::SaveConflict { .. })).then_some(command);
        self.drain_answered(answered, cx);
        outcome
    }
}

/// Whether `event` only says that something is ready to read again, so that twice is once.
/// Events that carry what changed (edits, results, failures) are each passed on.
fn is_idempotent(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::DiagnosticsUpdated { .. }
            | EngineEvent::WordCountChanged(_)
            | EngineEvent::SpellingChanged(_)
            | EngineEvent::GitHunksChanged(_)
            | EngineEvent::GitBlameChanged(_)
            | EngineEvent::GitStatusChanged { .. }
            | EngineEvent::ProblemsChanged
            | EngineEvent::NavigationChanged
            | EngineEvent::PreviewLayoutChanged
            | EngineEvent::DirectoryListed(_)
            | EngineEvent::ImageChanged(_)
    )
}

/// The kinds of secondary window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowKind {
    /// A read-only view of the active document that follows its edits.
    Mirror,
    /// The rendered preview, detached from the workspace window. Closing it docks the preview
    /// again.
    Preview,
    /// The workspace's link graph. It opens where it was last, at the size it had.
    Graph,
}

/// The graph window's size the first time it opens.
const GRAPH_WINDOW_SIZE: (f32, f32) = (1100., 760.);

pub fn open_main_window(
    app: Entity<AppState>,
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    cx: &mut App,
) {
    let options = window_options(Bounds::centered(None, size(px(1280.), px(800.)), cx));
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| WorkspaceView::new(app.clone(), folder, files.clone(), window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(error) = opened {
        eprintln!("adocfoundry: could not open the main window: {error}");
    }
}

/// The options every window opens with, so that each is grouped under the app and shows its
/// icon.
fn window_options(bounds: Bounds<gpui::Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        app_id: Some(APP_ID.to_owned()),
        ..WindowOptions::default()
    }
}

/// Opens a secondary window sharing `app`, returning it and its view. Window creation lives
/// here so that window kinds are added without touching the main window.
pub fn open_secondary_window(
    app: Entity<AppState>,
    kind: WindowKind,
    cx: &mut App,
) -> Option<(WindowHandle<Root>, AnyView)> {
    let options = match kind {
        WindowKind::Graph => {
            let mut options = window_options(Bounds::default());
            options.window_bounds =
                Some(graph_window_bounds(app.read(cx).engine.graph_window(), cx));
            options
        }
        _ => window_options(Bounds::centered(None, size(px(720.), px(640.)), cx)),
    };
    let mut view: Option<AnyView> = None;
    let opened = cx.open_window(options, |window, cx| match kind {
        WindowKind::Mirror => {
            let mirror = cx.new(|cx| MirrorView::new(app.clone(), window, cx));
            view = Some(mirror.clone().into());
            cx.new(|cx| Root::new(mirror, window, cx))
        }
        WindowKind::Preview => {
            let preview = cx.new(|cx| PreviewView::new(app.clone(), true, window, cx));
            let docking = app.clone();
            window.on_window_should_close(cx, move |_, cx| {
                docking.update(cx, |app, cx| {
                    if app.engine.preview_layout().detached {
                        let _ = app.invoke(
                            adoc_engine::commands::ids::VIEW_DETACH_PREVIEW,
                            CommandArgs::None,
                            cx,
                        );
                    }
                });
                true
            });
            view = Some(preview.clone().into());
            cx.new(|cx| Root::new(preview, window, cx))
        }
        WindowKind::Graph => {
            let graph = cx.new(|cx| {
                GraphView::new(
                    app.clone(),
                    gpui::WeakEntity::new_invalid(),
                    GraphMode::Window,
                    window,
                    cx,
                )
            });
            // Where the window is as it closes; the view notes it as it moves and is resized.
            let closing = app.clone();
            window.on_window_should_close(cx, move |window, cx| {
                record_graph_window(&closing, window, cx);
                true
            });
            window.set_window_title("Graph");
            view = Some(graph.clone().into());
            cx.new(|cx| Root::new(graph, window, cx))
        }
    });
    match opened {
        Ok(handle) => view.map(|view| (handle, view)),
        Err(error) => {
            eprintln!("adocfoundry: could not open a window: {error}");
            None
        }
    }
}

/// Notes where the graph window is, for the session to keep.
pub(crate) fn record_graph_window(app: &Entity<AppState>, window: &Window, cx: &mut App) {
    let (bounds, maximised) = match window.window_bounds() {
        WindowBounds::Windowed(bounds) => (bounds, false),
        WindowBounds::Maximized(bounds) | WindowBounds::Fullscreen(bounds) => (bounds, true),
    };
    let placement = WindowPlacement {
        x: bounds.origin.x.into(),
        y: bounds.origin.y.into(),
        width: bounds.size.width.into(),
        height: bounds.size.height.into(),
        maximised,
    };
    app.update(cx, |app, _| app.engine.set_graph_window(placement));
}

/// Where the graph window opens: where it was, unless less than half of that is on a display
/// now, when it is centred on the main display at its size, shrunk to fit; centred at its first
/// size when it has never been open.
fn graph_window_bounds(placement: Option<WindowPlacement>, cx: &App) -> WindowBounds {
    let first = size(px(GRAPH_WINDOW_SIZE.0), px(GRAPH_WINDOW_SIZE.1));
    let Some(placement) = placement else {
        return WindowBounds::Windowed(Bounds::centered(None, first, cx));
    };
    let saved = Bounds::new(
        point(px(placement.x), px(placement.y)),
        size(
            px(placement.width.max(200.)),
            px(placement.height.max(150.)),
        ),
    );
    let displays: Vec<Bounds<Pixels>> = cx
        .displays()
        .iter()
        .map(|display| display.bounds())
        .collect();
    let main = cx.primary_display();
    let bounds = match placed(
        saved,
        &displays,
        main.as_ref().map(|display| display.bounds()),
    ) {
        Some(bounds) => bounds,
        None => Bounds::centered(main.map(|display| display.id()), saved.size, cx),
    };
    if placement.maximised {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// `saved` when at least half of it lies on `displays`; else its size shrunk to fit `main`,
/// centred on it. `None` when no display is known, to let the platform centre it.
fn placed(
    saved: Bounds<Pixels>,
    displays: &[Bounds<Pixels>],
    main: Option<Bounds<Pixels>>,
) -> Option<Bounds<Pixels>> {
    let area =
        |bounds: Bounds<Pixels>| f32::from(bounds.size.width) * f32::from(bounds.size.height);
    let visible: f32 = displays
        .iter()
        .map(|display| area(display.intersect(&saved)))
        .sum();
    if visible * 2.0 >= area(saved) {
        return Some(saved);
    }
    let main = main?;
    let fitted = size(
        saved.size.width.min(main.size.width),
        saved.size.height.min(main.size.height),
    );
    Some(Bounds::new(
        point(
            main.origin.x + (main.size.width - fitted.width) / 2.,
            main.origin.y + (main.size.height - fitted.height) / 2.,
        ),
        fitted,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_window_mostly_off_every_display_is_brought_back() {
        let main = Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)));
        let side = Bounds::new(point(px(1440.), px(0.)), size(px(1920.), px(1080.)));
        let on_side = Bounds::new(point(px(1600.), px(100.)), size(px(1000.), px(700.)));
        assert_eq!(placed(on_side, &[main, side], Some(main)), Some(on_side));

        // The second display is gone: the window comes back to the main one, shrunk to fit.
        let big = Bounds::new(point(px(1600.), px(100.)), size(px(1800.), px(1000.)));
        let back = placed(big, &[main], Some(main)).unwrap();
        assert_eq!(back.size, size(px(1440.), px(900.)));
        assert_eq!(back.origin, point(px(0.), px(0.)));

        // Half on screen is enough.
        let straddling = Bounds::new(point(px(940.), px(0.)), size(px(1000.), px(500.)));
        assert_eq!(placed(straddling, &[main], Some(main)), Some(straddling));
    }

    #[test]
    fn every_window_names_the_app() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(720.), px(640.)));
        assert_eq!(window_options(bounds).app_id.as_deref(), Some(APP_ID));
    }

    #[test]
    fn desktop_entry_is_found_by_the_app_id() {
        let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/linux")
            .join(format!("{APP_ID}.desktop"));
        let text =
            std::fs::read_to_string(&entry).expect("the desktop entry is named for the app id");
        assert!(text
            .lines()
            .any(|line| line == format!("StartupWMClass={APP_ID}")));
        assert!(text.lines().any(|line| line == format!("Icon={APP_ID}")));
    }
}
