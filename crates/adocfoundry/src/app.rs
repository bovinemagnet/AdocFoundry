//! Application state: the engine, its event pump, and window creation.

use std::{path::PathBuf, sync::mpsc::Receiver, time::Duration};

use adoc_engine::{CommandArgs, CommandId, CommandOutcome, Engine, EngineError, EngineEvent};
use gpui::{
    px, size, AnyView, App, AppContext as _, Bounds, Context, Entity, EventEmitter, Task,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::Root;

use crate::{mirror_view::MirrorView, preview::PreviewView, workspace_view::WorkspaceView};

/// How often the engine is pumped while work is pending, and while idle.
const BUSY_TICK: Duration = Duration::from_millis(16);
const IDLE_TICK: Duration = Duration::from_millis(100);

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
        // Unsaved edits survive a crash, and saved versions are kept to restore.
        engine.enable_recovery();
        engine.enable_local_history();
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

    /// Applies pending worker and watcher results and re-emits every engine event.
    pub fn drain(&mut self, cx: &mut Context<Self>) {
        self.engine.pump();
        let events: Vec<EngineEvent> = self.events.try_iter().collect();
        if events.is_empty() {
            return;
        }
        for event in events {
            cx.emit(event);
        }
        cx.notify();
    }

    /// Invokes an engine command and publishes the events it caused before returning.
    pub fn invoke(
        &mut self,
        command: CommandId,
        args: CommandArgs,
        cx: &mut Context<Self>,
    ) -> Result<CommandOutcome, EngineError> {
        let outcome = self.engine.invoke(command, args);
        self.drain(cx);
        outcome
    }
}

/// The kinds of secondary window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowKind {
    /// A read-only view of the active document that follows its edits.
    Mirror,
    /// The rendered preview, detached from the workspace window. Closing it docks the preview
    /// again.
    Preview,
}

pub fn open_main_window(
    app: Entity<AppState>,
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    cx: &mut App,
) {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        ..WindowOptions::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| WorkspaceView::new(app.clone(), folder, files.clone(), window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(error) = opened {
        eprintln!("adocfoundry: could not open the main window: {error}");
    }
}

/// Opens a secondary window sharing `app`, returning it and its view. Window creation lives
/// here so that window kinds are added without touching the main window.
pub fn open_secondary_window(
    app: Entity<AppState>,
    kind: WindowKind,
    cx: &mut App,
) -> Option<(WindowHandle<Root>, AnyView)> {
    let bounds = Bounds::centered(None, size(px(720.), px(640.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        ..WindowOptions::default()
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
    });
    match opened {
        Ok(handle) => view.map(|view| (handle, view)),
        Err(error) => {
            eprintln!("adocfoundry: could not open a window: {error}");
            None
        }
    }
}
