//! Application state: the engine, its event pump, and window creation.

use std::{
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

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

/// The engine is pumped no more often than this, however often something wakes the pump.
const MIN_TICK: Duration = Duration::from_millis(16);
/// With nothing due, the pump sleeps until something wakes it, or this long. Nothing is known to
/// need the timeout: it bounds how long a wake-up missed by mistake could hold work up.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The id Linux desktops match a window to its desktop entry by, and so find its icon: the
/// Wayland app id and the X11 `WM_CLASS`. It is the desktop entry's name and the Flatpak id.
const APP_ID: &str = "io.github.bovinemagnet.AdocFoundry";

/// The app's engine, which wakes the pump whenever it is reached mutably. Only a mutable call can
/// schedule work, and every one comes through here, so the sleeping pump hears of them all
/// without each call having to remember to wake it (#222).
pub struct PumpedEngine {
    engine: Engine,
    wake: smol::channel::Sender<()>,
}

impl PumpedEngine {
    /// Tells the pump to look at the engine again. A pump already told is told once.
    fn wake(&self) {
        let _ = self.wake.try_send(());
    }
}

impl Deref for PumpedEngine {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        &self.engine
    }
}

impl DerefMut for PumpedEngine {
    fn deref_mut(&mut self) -> &mut Engine {
        self.wake();
        &mut self.engine
    }
}

/// The single owner of the engine. Views hold an `Entity<AppState>`, invoke commands through it,
/// and subscribe to the engine events it re-emits.
pub struct AppState {
    pub engine: PumpedEngine,
    /// How the releases feed is read: the system's `curl`, or a stand-in in tests.
    pub fetch: crate::updates::Fetch,
    /// How an update is readied: downloaded, checked and staged, or a stand-in in tests.
    pub prepare: crate::updates::Prepare,
    /// An update readied and waiting for the quit it asked for, which puts it in place.
    pub(crate) staged_update: Option<crate::updates::Staged>,
    events: Receiver<EngineEvent>,
    _pump: Task<()>,
    _quit: gpui::Subscription,
    /// The workspace window and its view, which Quit asks about unsaved pages.
    pub(crate) main: Option<(gpui::AnyWindowHandle, gpui::WeakEntity<WorkspaceView>)>,
    /// How many times the pump task has run.
    #[cfg(test)]
    pub(crate) pumps: usize,
}

impl EventEmitter<EngineEvent> for AppState {}

impl AppState {
    pub fn new(mut engine: Engine, cx: &mut Context<Self>) -> Self {
        crate::fonts::init(cx);
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
        let (wake, woken) = smol::channel::bounded(1);
        // Sleeps until the engine's next deadline or until something wakes it, so an idle app
        // takes no wake-ups of its own.
        let pump = cx.spawn(async move |this, cx| loop {
            // Whatever wakes it from here on is seen by the pump below or wakes it again.
            let _ = woken.try_recv();
            let Ok(due) = this.update(cx, |this, cx| this.pump_tick(cx)) else {
                break;
            };
            cx.background_executor().timer(MIN_TICK).await;
            let wait = due.map_or(IDLE_TIMEOUT, |due| {
                due.saturating_duration_since(Instant::now())
            });
            let timer = cx.background_executor().timer(wait);
            smol::future::or(timer, async {
                let _ = woken.recv().await;
            })
            .await;
        });
        // Edits made in the last moments before quitting are kept too.
        let quit = cx.on_app_quit(|this, _| {
            this.engine.write_recovery_now();
            // An update still waiting when the app quits some other way was never answered for.
            drop(this.staged_update.take());
            async {}
        });
        #[cfg_attr(test, allow(unused_mut))]
        let mut state = Self {
            engine: PumpedEngine { engine, wake },
            fetch: crate::updates::system_fetch(),
            prepare: crate::updates::system_prepare(),
            staged_update: None,
            events,
            _pump: pump,
            _quit: quit,
            main: None,
            #[cfg(test)]
            pumps: 0,
        };
        #[cfg(not(test))]
        state.wake_from_other_threads();
        state
    }

    /// Has worker results and changes on disk wake the pump. GPUI's test scheduler fails a test
    /// when another thread wakes a task, so an app test asks for this itself, having allowed it
    /// with `allow_parking`.
    pub(crate) fn wake_from_other_threads(&mut self) {
        let wake = self.engine.wake.clone();
        self.engine.engine.set_waker(move || {
            let _ = wake.try_send(());
        });
    }

    /// Applies pending worker and watcher results and re-emits the engine's events. Each view
    /// redraws itself for the events it shows, so nothing is redrawn here; an event that says
    /// only that something is ready to read again is passed on once however often it came.
    pub fn drain(&mut self, cx: &mut Context<Self>) {
        self.drain_answered(None, cx);
    }

    /// Drains as [`AppState::drain`] does, leaving out the failure of `answered`, which its
    /// caller answers itself. A pump can schedule work, so the pump task is told.
    fn drain_answered(&mut self, answered: Option<CommandId>, cx: &mut Context<Self>) {
        crate::typing_bench::measure("pump", || self.engine.pump());
        self.publish(answered, cx);
    }

    /// The pump task's own drain, which does not wake it: it reads the next deadline here.
    /// Returns when the engine next has something to do.
    fn pump_tick(&mut self, cx: &mut Context<Self>) -> Option<Instant> {
        #[cfg(test)]
        {
            self.pumps += 1;
        }
        crate::typing_bench::measure("pump", || self.engine.engine.pump());
        self.publish(None, cx);
        self.engine.next_due()
    }

    /// Re-emits the engine's events.
    fn publish(&mut self, answered: Option<CommandId>, cx: &mut Context<Self>) {
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
            | EngineEvent::AuditChanged
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
    let mut workspace = None;
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| WorkspaceView::new(app.clone(), folder, files.clone(), window, cx));
        workspace = Some(view.downgrade());
        cx.new(|cx| Root::new(view, window, cx))
    });
    match (opened, workspace) {
        (Ok(handle), Some(workspace)) => {
            app.update(cx, |app, _| app.main = Some((handle.into(), workspace)));
        }
        (Err(error), _) => eprintln!("adocfoundry: could not open the main window: {error}"),
        (Ok(_), None) => {}
    }
}

/// Opens `paths`, handed over by the system while the app runs, in the main window: a folder
/// as the workspace, and files in tabs, opening the workspace they belong to when none is open.
/// With the main window gone, as macOS allows, opens one for them.
pub fn open_paths(app: &Entity<AppState>, paths: Vec<PathBuf>, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let main = app
        .read(cx)
        .main
        .as_ref()
        .and_then(|(handle, workspace)| Some((*handle, workspace.upgrade()?)));
    let Some((handle, workspace)) = main else {
        let launch = adoc_engine::launch_targets(paths);
        return open_main_window(app.clone(), launch.folder, launch.files, cx);
    };
    let _ = handle.update(cx, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.open_paths(paths, window, cx));
        window.activate_window();
    });
}

/// The actions that belong to the app rather than a window: Quit, from any window, the
/// keymap, the palette or a menu; on macOS, hiding, and the app menu in the menu bar.
pub fn bind_app_actions(app: &Entity<AppState>, cx: &mut App) {
    use crate::editor::bindings::{
        HideApp, HideOtherApps, OpenSettings, Quit, ShowAbout, ShowAllApps,
    };
    let quitting = app.clone();
    cx.on_action(move |_: &Quit, cx| quit(&quitting, cx));
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
    if cfg!(target_os = "macos") {
        use gpui::{Menu, MenuItem, SystemMenuType};
        // The app menu macOS users look for first; the rest is the ☰ menu in the title bar.
        // About and Settings act in the workspace window, as their ☰ entries do.
        cx.set_menus([Menu::new("AdocFoundry").items([
            MenuItem::action("About AdocFoundry", ShowAbout),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide AdocFoundry", HideApp),
            MenuItem::action("Hide Others", HideOtherApps),
            MenuItem::action("Show All", ShowAllApps),
            MenuItem::separator(),
            MenuItem::action(crate::menus::quit_title(), Quit),
        ])]);
    }
}

/// File › Quit, from any window or the macOS app menu: the workspace window asks about unsaved
/// pages first, as closing it does, and quits once answered; with that window gone (macOS keeps
/// the app running without it) the app quits at once.
///
/// The action arrives while its window is being updated, so the asking waits until that update
/// is over; only a workspace window that is gone quits without it.
pub fn quit(app: &Entity<AppState>, cx: &mut App) {
    let main = app.read(cx).main.clone();
    let app = app.clone();
    cx.defer(move |cx| {
        if let Some((handle, workspace)) =
            main.filter(|(_, workspace)| workspace.upgrade().is_some())
        {
            let asked = handle.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.quit(window, cx))
            });
            if asked.is_ok() {
                return;
            }
        }
        quit_now(&app, cx);
    });
}

/// Quits the app, once any question about unsaved pages is answered. An update waiting for
/// this quit is put in place first. Tests see that the quit was asked for instead.
pub(crate) fn quit_now(app: &Entity<AppState>, cx: &mut App) {
    if let Some(staged) = app.update(cx, |app, _| app.staged_update.take()) {
        if let Err(error) = staged.finish() {
            eprintln!("adocfoundry: the update was not installed: {error}");
        }
    }
    #[cfg(test)]
    {
        QUIT_ASKED.with(|asked| asked.set(true));
    }
    #[cfg(not(test))]
    cx.quit();
}

#[cfg(test)]
thread_local! {
    /// Whether the app was asked to quit, in a test, where quitting would end it.
    pub(crate) static QUIT_ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
                    let layout = app.engine.preview_layout();
                    if layout.detached {
                        // A PDF window turns off as its toggle would, putting back what was
                        // there before it (#420); any other preview docks again.
                        let command = if layout.pdf && layout.before_pdf.is_some() {
                            adoc_engine::commands::ids::VIEW_TOGGLE_PDF_WINDOW
                        } else {
                            adoc_engine::commands::ids::VIEW_DETACH_PREVIEW
                        };
                        let _ = app.invoke(command, CommandArgs::None, cx);
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
