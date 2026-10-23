//! Application state: the engine, its event pump, and window creation.

use std::{path::PathBuf, sync::mpsc::Receiver, time::Duration};

use adoc_engine::{CommandArgs, CommandId, CommandOutcome, Engine, EngineError, EngineEvent};
use gpui::{
    px, size, App, AppContext as _, Bounds, Context, Entity, EventEmitter, Task, WindowBounds,
    WindowOptions,
};
use gpui_component::Root;

use crate::{mirror_view::MirrorView, workspace_view::WorkspaceView};

/// How often the engine is pumped while work is pending, and while idle.
const BUSY_TICK: Duration = Duration::from_millis(16);
const IDLE_TICK: Duration = Duration::from_millis(100);

/// The single owner of the engine. Views hold an `Entity<AppState>`, invoke commands through it,
/// and subscribe to the engine events it re-emits.
pub struct AppState {
    pub engine: Engine,
    events: Receiver<EngineEvent>,
    _pump: Task<()>,
}

impl EventEmitter<EngineEvent> for AppState {}

impl AppState {
    pub fn new(mut engine: Engine, cx: &mut Context<Self>) -> Self {
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
        Self {
            engine,
            events,
            _pump: pump,
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

/// The kinds of secondary window. Phase 2 adds a detachable preview here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowKind {
    /// A read-only view of the active document that follows its edits.
    Mirror,
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

/// Opens a secondary window sharing `app`. Window creation lives here so that later window
/// kinds are added without touching the main window.
pub fn open_secondary_window(app: Entity<AppState>, kind: WindowKind, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(720.), px(640.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        ..WindowOptions::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = match kind {
            WindowKind::Mirror => cx.new(|cx| MirrorView::new(app.clone(), window, cx)),
        };
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(error) = opened {
        eprintln!("adocfoundry: could not open a window: {error}");
    }
}
