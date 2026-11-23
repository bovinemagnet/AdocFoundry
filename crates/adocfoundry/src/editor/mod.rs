//! The source editor: gpui-component's code editor mirroring one engine document.
//!
//! The widget owns a display buffer for performance. Every widget edit is sent to the engine as
//! `edit.insert_text` or `edit.replace_range`, exactly as the widget reports it
//! (`InputEditsEvent`); every engine change from elsewhere (undo, replace, rename, a reload from
//! disk) is applied back to the widget. The engine rope is the truth.

pub mod bindings;
pub mod blame;
pub mod changes;
pub mod language;
mod providers;
pub mod sync;

use std::{cell::RefCell, rc::Rc};

use adoc_core::{DiagnosticSeverity, SourceRange};
use adoc_engine::{
    commands::ids, CommandArgs, CommandId, CommandOutcome, DocumentId, EngineEvent, ImageSource,
};
use gpui::{
    div, prelude::FluentBuilder as _, relative, AppContext as _, Context, Entity,
    EntityInputHandler as _, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window,
};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity as WidgetSeverity},
    input::{
        Editor, EditorState, GutterDecoration, GutterDecorationCollection, InputEdit,
        InputEditsEvent, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
        RopeExt as _, TabSize, TextDecoration, TextDecorationCollection,
    },
    native_menu::NativeMenu,
    ActiveTheme as _, WindowExt as _,
};

use crate::{app::AppState, sidebar::DraggedLink};
use providers::EngineProviders;

/// Sends the difference between the engine's text and `widget_text` to the engine as one edit,
/// returning the document's version after it: the version whose change the widget has already.
pub fn sync_widget_to_engine(
    app: &mut AppState,
    document: DocumentId,
    widget_text: &ropey::Rope,
    cx: &mut Context<AppState>,
) -> Option<u64> {
    let open = app.engine.document(document)?;
    let (range, replacement) = sync::diff(&open.text, widget_text)?;
    let command = if range.is_empty() {
        ids::EDIT_INSERT_TEXT
    } else {
        ids::EDIT_REPLACE_RANGE
    };
    let args = CommandArgs::Text {
        id: document,
        range: SourceRange::new(range.start, range.end),
        text: replacement,
    };
    if let Err(error) = app.invoke(command, args, cx) {
        // The widget and engine disagree in a way an edit cannot express; the engine wins
        // when the editor next reconciles.
        eprintln!("adocfoundry: edit rejected: {error}");
        return None;
    }
    app.engine.document(document).map(|open| open.version)
}

/// The widget text the engine already has without the widget's edit event for it: set when the
/// engine was brought up to date by a diff (a provider asking before the event arrived) or the
/// widget was given the engine's text, so the edits up to that text are not sent again.
pub type Synced = Rc<RefCell<Option<ropey::Rope>>>;

/// Sends one of the widget's edits to the engine, `after` being the widget's text just after it,
/// returning the document's version after it. A replacement of the whole text (how the widget
/// reports an IME composition) is narrowed to what changed first.
fn send_widget_edit(
    app: &mut AppState,
    document: DocumentId,
    edit: &InputEdit,
    after: &ropey::Rope,
    cx: &mut Context<AppState>,
) -> Result<Option<u64>, ()> {
    let Some(open) = app.engine.document(document) else {
        return Ok(None);
    };
    let whole = open.text.len();
    // A replacement of the whole text, or one reaching past the engine's (a composition
    // committed: the widget held its provisional text, the engine did not), is narrowed to
    // what changed.
    if (edit.start_byte == 0 && edit.old_end_byte == whole && whole > 0)
        || edit.old_end_byte > whole
    {
        return Ok(sync_widget_to_engine(app, document, after, cx));
    }
    let replacement = after.slice(edit.start_byte..edit.new_end_byte).to_string();
    // An edit that changes nothing (a composition marked again in place) is not sent: on a
    // shared page it would still be sent to everyone as a deletion and an insertion.
    let open = app.engine.document(document).map(|open| &open.text);
    if open.is_some_and(|text| {
        edit.old_end_byte <= text.len()
            && text.is_char_boundary(edit.start_byte)
            && text.is_char_boundary(edit.old_end_byte)
            && text.slice(edit.start_byte..edit.old_end_byte) == replacement.as_str()
    }) {
        return Ok(None);
    }
    let command = if edit.old_end_byte == edit.start_byte {
        ids::EDIT_INSERT_TEXT
    } else {
        ids::EDIT_REPLACE_RANGE
    };
    let args = CommandArgs::Text {
        id: document,
        range: SourceRange::new(edit.start_byte, edit.old_end_byte),
        text: replacement,
    };
    if let Err(error) = app.invoke(command, args, cx) {
        // Refused (a shared page this participant may not edit just now, or an edit the
        // engine cannot take): the caller puts the engine's text back in the widget.
        eprintln!("adocfoundry: edit rejected: {error}");
        return Err(());
    }
    Ok(app.engine.document(document).map(|open| open.version))
}

/// The character boundary under `point` nearest to it, among the rows the editor has laid out.
fn offset_at(state: &EditorState, point: gpui::Point<gpui::Pixels>) -> Option<usize> {
    let rows = state.visible_row_range()?;
    let map = state.display_map();
    let text = state.text();
    let first = map.display_row_to_buffer_line(rows.start);
    let last = map
        .display_row_to_buffer_line(rows.end.saturating_sub(1).max(rows.start))
        .min(text.lines_len().saturating_sub(1));
    let mut nearest: Option<(gpui::Pixels, usize)> = None;
    for line in first..=last {
        let (start, end) = (text.line_start_offset(line), text.line_end_offset(line));
        let line_text = text.slice(start..end).to_string();
        let boundaries = line_text.char_indices().map(|(at, _)| start + at);
        for offset in boundaries.chain([end]) {
            let Some(bounds) = state.range_to_bounds(&(offset..offset)) else {
                continue;
            };
            if point.y < bounds.top() || point.y >= bounds.bottom() {
                continue;
            }
            let distance = (bounds.left() - point.x).abs();
            if nearest.is_none_or(|(closest, _)| distance < closest) {
                nearest = Some((distance, offset));
            }
        }
    }
    nearest.map(|(_, offset)| offset)
}

/// What the editor tells the window around it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEditorEvent {
    /// The caret moved to this byte offset.
    CaretMoved(usize),
}

pub struct SourceEditorView {
    app: Entity<AppState>,
    document: DocumentId,
    readonly: bool,
    /// Read-only because the page is shared and this participant may not edit it now.
    shared_readonly: bool,
    /// The other participants' carets and selections as drawn on a shared page, and this
    /// participant's own as last shared.
    presence: Option<Presence>,
    /// The carets last drawn.
    shown_carets: Vec<adoc_engine::PeerCaret>,
    /// The page's comments as drawn: a fill over each open comment's text and a mark in the
    /// gutter (#762), and the ranges last drawn.
    comment_marks: Option<Presence>,
    shown_comments: Vec<std::ops::Range<usize>>,
    /// The page's suggestions as drawn (#763), and what was last drawn.
    suggestion_marks: Option<SuggestionMarks>,
    shown_suggestions: ShownSuggestions,
    shared_selection: Option<(usize, usize)>,
    caret: Option<usize>,
    /// The widget's scroll offset as last drawn, which the change markers follow.
    scroll: Option<gpui::Point<gpui::Pixels>>,
    /// The document versions the widget's own edits made, which the engine's
    /// `DocumentChanged` for them need not reconcile.
    own_versions: std::collections::BTreeSet<u64>,
    synced: Synced,
    state: Entity<EditorState>,
    /// The change markers as last painted.
    marks: Rc<RefCell<Vec<changes::GutterMark>>>,
    /// The change whose card is open, with its text in `HEAD` and where it was clicked.
    open_change: Option<(adoc_engine::Hunk, String, gpui::Point<gpui::Pixels>)>,
    /// Whether the blame column is shown.
    show_blame: bool,
    /// The gutter line the pointer rests on, and the blame tooltip once it has rested long enough.
    hover_line: Option<usize>,
    hover_tip: Option<(String, gpui::Point<gpui::Pixels>)>,
    _hover: Option<gpui::Task<()>>,
    _diagnostics: Option<gpui::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Puts `replacement` in place of `range` of the widget's text, keeping the selection on the
/// text it was on.
fn apply_change(
    state: &mut EditorState,
    range: &std::ops::Range<usize>,
    replacement: String,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    let selection = state.selected_range();
    let inserted = replacement.len();
    state.set_selected_range(range.clone(), cx);
    state.replace(replacement, window, cx);
    let mapped = |offset| sync::map_offset(offset, range, inserted);
    state.set_selected_range(mapped(selection.start)..mapped(selection.end), cx);
}

/// Where the other participants' carets are drawn: decoration collections the widget keeps.
#[derive(Clone)]
struct Presence {
    ranges: RangeDecorationCollection,
    gutter: GutterDecorationCollection,
}

/// Where the page's suggestions are marked (#763): decoration collections the widget keeps.
#[derive(Clone)]
struct SuggestionMarks {
    frames: RangeDecorationCollection,
    struck: TextDecorationCollection,
    gutter: GutterDecorationCollection,
}

/// The page's suggestions as last marked (#763), for tests: the frames round their ranges, the
/// parts struck through, and the ranges marked in the gutter.
#[derive(Clone, Debug, Default)]
pub struct ShownSuggestions {
    pub frames: Vec<RangeDecoration>,
    pub struck: Vec<TextDecoration>,
    pub gutter: Vec<std::ops::Range<usize>>,
}

/// The end of the character at `offset`, so a gutter mark covers the caret's line however it
/// stands; `offset` itself at the end of the text.
fn around_end(text: &ropey::Rope, offset: usize) -> usize {
    (offset + 1..=text.len())
        .find(|end| text.is_char_boundary(*end))
        .unwrap_or(offset)
}

/// Diagnostics are redrawn once typing pauses for this long, not on every keystroke.
const DIAGNOSTICS_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
/// Lines left above a place navigated to, so it is read in context.
const REVEAL_MARGIN: usize = 3;
/// How long the pointer rests on the gutter before the line's blame shows.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

impl SourceEditorView {
    pub fn new(
        app: Entity<AppState>,
        document: DocumentId,
        readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (text, soft_wrap, tab_width, language) = {
            let engine = &app.read(cx).engine;
            let text = engine
                .document(document)
                .map(|open| open.text.to_string())
                .unwrap_or_default();
            let editor = &engine.settings().editor;
            let language = engine
                .document(document)
                .map(|open| language::for_document(&open.path, open.asciidoc))
                .unwrap_or_else(|| language::ASCIIDOC.to_owned());
            (text, editor.soft_wrap, editor.tab_width as usize, language)
        };
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .folding(true)
                .soft_wrap(soft_wrap)
                .tab_size(TabSize {
                    tab_size: tab_width,
                    hard_tabs: false,
                })
                .default_value(text);
            state.set_readonly(readonly, cx);
            state
        });

        let synced = Synced::default();
        let providers = Rc::new(EngineProviders::new(
            app.downgrade(),
            document,
            synced.clone(),
        ));
        state.update(cx, |state, cx| {
            let lsp = state.lsp_mut();
            lsp.definition_provider = Some(providers.clone());
            lsp.hover_provider = Some(providers.clone());
            if !readonly {
                lsp.completion_provider = Some(providers.clone());
            }
            let follow_from = providers.clone();
            let follow_app = app.downgrade();
            // A modifier-click on a link: follow it through the engine so history is kept.
            lsp.show_document = Some(Rc::new(move |_, window, cx| {
                let offset = follow_from.last_definition_offset();
                if let Some(app) = follow_app.upgrade() {
                    crate::workspace_view::follow_link(app, document, offset, window, cx);
                }
                true
            }));
            cx.notify();
        });

        let mut subscriptions = vec![cx.subscribe_in(
            &state,
            window,
            |this, _, event: &InputEditsEvent, window, cx| {
                let _timed = crate::typing_bench::timed("edit");
                let edits = event.edits();
                // Edits up to the text the engine already has were sent another way.
                let sent = this
                    .synced
                    .take()
                    .and_then(|synced| edits.iter().position(|(_, after)| *after == synced));
                let document = this.document;
                let mut refused = false;
                for (edit, after) in &edits[sent.map_or(0, |at| at + 1)..] {
                    match this.app.update(cx, |app, cx| {
                        send_widget_edit(app, document, edit, after, cx)
                    }) {
                        Ok(version) => this.own_versions.extend(version),
                        Err(()) => refused = true,
                    }
                }
                // The widget shows what the engine refused: put the engine's text back
                // once the widget's own edit has finished.
                if refused {
                    cx.defer_in(window, |this, window, cx| {
                        this.reconcile(window, cx);
                        this.sync_readonly(cx);
                    });
                }
            },
        )];
        subscriptions.push(cx.subscribe_in(&app, window, |this, _, event, window, cx| {
            let _timed = crate::typing_bench::timed("events.editor");
            match event {
                // The engine echoing an edit the widget made needs nothing applied back.
                EngineEvent::DocumentChanged { id, version, .. } if *id == this.document => {
                    if !this.own_versions.remove(version) {
                        this.reconcile(window, cx);
                    }
                    this.sync_readonly(cx);
                    // The others' carets, and the comments, moved with the text.
                    this.show_presence(cx);
                    this.show_comments(cx);
                    this.show_suggestions(cx);
                }
                EngineEvent::CommentsChanged(id) if *id == this.document => {
                    this.show_comments(cx);
                    this.show_suggestions(cx);
                }
                EngineEvent::SessionChanged => {
                    this.sync_readonly(cx);
                    this.show_presence(cx);
                    this.follow_caret(cx);
                }
                EngineEvent::PresenceChanged(id) if *id == this.document => {
                    this.show_presence(cx);
                    this.follow_caret(cx);
                }
                EngineEvent::GitHunksChanged(id) | EngineEvent::GitBlameChanged(id)
                    if *id == this.document =>
                {
                    cx.notify();
                }
                EngineEvent::SettingsChanged => cx.notify(),
                EngineEvent::DiagnosticsUpdated { path } if this.is_path(path, cx) => {
                    this._diagnostics = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DIAGNOSTICS_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.show_diagnostics(cx));
                    }));
                }
                EngineEvent::SpellingChanged(id) if *id == this.document => {
                    this._diagnostics = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(DIAGNOSTICS_DELAY).await;
                        let _ = this.update(cx, |this, cx| this.show_diagnostics(cx));
                    }));
                }
                _ => {}
            }
        }));

        subscriptions.push(cx.observe(&state, |this: &mut Self, state, cx| {
            let (caret, scroll) = {
                let state = state.read(cx);
                (state.cursor(), state.scroll_offset())
            };
            let moved = this.caret != Some(caret);
            if moved {
                this.caret = Some(caret);
                cx.emit(SourceEditorEvent::CaretMoved(caret));
            }
            this.share_selection(cx);
            // The change markers follow the editor's scrolling; nothing else here changes with
            // the widget's state, which redraws itself.
            if moved || this.scroll != Some(scroll) {
                this.scroll = Some(scroll);
                cx.notify();
            }
        }));

        let mut view = Self {
            app,
            document,
            readonly,
            shared_readonly: false,
            presence: None,
            shown_carets: Vec::new(),
            comment_marks: None,
            shown_comments: Vec::new(),
            suggestion_marks: None,
            shown_suggestions: ShownSuggestions::default(),
            shared_selection: None,
            caret: None,
            scroll: None,
            own_versions: std::collections::BTreeSet::new(),
            synced,
            state,
            marks: Rc::default(),
            open_change: None,
            show_blame: false,
            hover_line: None,
            hover_tip: None,
            _hover: None,
            _diagnostics: None,
            _subscriptions: subscriptions,
        };
        view.show_diagnostics(cx);
        view.sync_readonly(cx);
        view.show_comments(cx);
        view.show_suggestions(cx);
        view
    }

    /// Marks the page's open comments: their text filled in the theme's warning colour and a
    /// mark in the gutter. Resolved and detached comments are not marked; the Comments panel
    /// lists them, and names each comment, so the colour is never all that says one is there.
    /// Suggestions are marked apart (`show_suggestions`).
    fn show_comments(&mut self, cx: &mut Context<Self>) {
        let ranges: Vec<std::ops::Range<usize>> = self
            .app
            .read(cx)
            .engine
            .comments(self.document)
            .into_iter()
            .filter(|comment| {
                !comment.detached && comment.resolved.is_none() && comment.suggestion.is_none()
            })
            .map(|comment| comment.range.start..comment.range.end)
            .collect();
        if ranges.is_empty() && self.comment_marks.is_none() {
            return;
        }
        let marks = match &self.comment_marks {
            Some(marks) => marks.clone(),
            None => {
                let marks = self.state.update(cx, |state, cx| Presence {
                    ranges: state.create_range_decorations_collection(Vec::new(), cx),
                    gutter: state.create_gutter_decorations_collection(Vec::new(), cx),
                });
                self.comment_marks = Some(marks.clone());
                marks
            }
        };
        let text = self.state.read(cx).text().clone();
        let len = text.len();
        let colour = cx.theme().warning;
        let drawn: Vec<std::ops::Range<usize>> = ranges
            .into_iter()
            .filter(|range| range.start < range.end && range.end <= len)
            .collect();
        marks.ranges.set(
            drawn
                .iter()
                .map(|range| {
                    RangeDecoration::new(range.clone())
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(colour.opacity(0.22))
                })
                .collect(),
            cx,
        );
        marks.gutter.set(
            drawn
                .iter()
                .map(|range| GutterDecoration::new(range.clone()).with_color(colour))
                .collect(),
            cx,
        );
        self.shown_comments = drawn;
    }

    /// Marks the page's undecided suggestions (#763) in the theme's info colour, apart from the
    /// comments' warning fill and the other participants' chart colours: a frame round each
    /// one's range and a mark in the gutter, and, while it can be accepted, the part accepting
    /// would remove struck through. The strike sets nothing that changes the layout. What it
    /// would insert is not drawn in the text; the card and the status bar say it. Accepted,
    /// rejected and detached suggestions are not marked.
    fn show_suggestions(&mut self, cx: &mut Context<Self>) {
        use adoc_engine::SuggestionState;
        let suggestions: Vec<(std::ops::Range<usize>, Option<std::ops::Range<usize>>)> = self
            .app
            .read(cx)
            .engine
            .comments(self.document)
            .into_iter()
            .filter(|comment| !comment.detached)
            .filter_map(|comment| {
                let suggestion = comment.suggestion?;
                let struck = match suggestion.state {
                    SuggestionState::Pending => {
                        Some(suggestion.removed.start..suggestion.removed.end)
                    }
                    SuggestionState::Outdated => None,
                    SuggestionState::Accepted | SuggestionState::Rejected => return None,
                };
                Some((comment.range.start..comment.range.end, struck))
            })
            .collect();
        if suggestions.is_empty() && self.suggestion_marks.is_none() {
            return;
        }
        let marks = match &self.suggestion_marks {
            Some(marks) => marks.clone(),
            None => {
                let marks = self.state.update(cx, |state, cx| SuggestionMarks {
                    frames: state.create_range_decorations_collection(Vec::new(), cx),
                    struck: state.create_decorations_collection(Vec::new(), cx),
                    gutter: state.create_gutter_decorations_collection(Vec::new(), cx),
                });
                self.suggestion_marks = Some(marks.clone());
                marks
            }
        };
        let len = self.state.read(cx).text().len();
        let colour = cx.theme().info;
        let drawn = |range: &std::ops::Range<usize>| range.start < range.end && range.end <= len;
        let mut shown = ShownSuggestions::default();
        for (range, struck) in suggestions.iter().filter(|(range, _)| drawn(range)) {
            shown.frames.push(
                RangeDecoration::new(range.clone())
                    .with_style(RangeDecorationStyle::Frame)
                    .with_color(colour),
            );
            shown.gutter.push(range.clone());
            if let Some(struck) = struck.as_ref().filter(|struck| drawn(struck)) {
                shown.struck.push(TextDecoration::new(
                    struck.clone(),
                    gpui::HighlightStyle {
                        strikethrough: Some(gpui::StrikethroughStyle {
                            thickness: gpui::px(1.),
                            color: Some(colour),
                        }),
                        ..gpui::HighlightStyle::default()
                    },
                ));
            }
        }
        marks.frames.set(shown.frames.clone(), cx);
        marks.struck.set(shown.struck.clone(), cx);
        marks.gutter.set(
            shown
                .gutter
                .iter()
                .map(|range| GutterDecoration::new(range.clone()).with_color(colour))
                .collect(),
            cx,
        );
        self.shown_suggestions = shown;
    }

    /// The comments' ranges as last marked, for tests.
    #[cfg(test)]
    pub fn shown_comments(&self) -> &[std::ops::Range<usize>] {
        &self.shown_comments
    }

    /// The suggestions' marks as last drawn, for tests.
    #[cfg(test)]
    pub fn shown_suggestions(&self) -> &ShownSuggestions {
        &self.shown_suggestions
    }

    /// Selects `range` and scrolls it into view: where Next Comment and the Comments panel go.
    pub fn select_range(
        &mut self,
        range: SourceRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            let len = state.text().len();
            state.set_selected_range(range.start.min(len)..range.end.min(len), cx);
        });
        self.scroll_to_line_of(range.start, REVEAL_MARGIN, cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Puts the keyboard's focus in the editor.
    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        let focus = self.state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Shows this participant's selection to the others on a shared page, when it changed: the
    /// engine paces what it sends.
    fn share_selection(&mut self, cx: &mut Context<Self>) {
        let (range, caret) = {
            let state = self.state.read(cx);
            (state.selected_range(), state.cursor())
        };
        // The anchor is the end the caret is not at.
        let anchor = if caret == range.start {
            range.end
        } else {
            range.start
        };
        let selection = (anchor, caret);
        if self.shared_selection == Some(selection) {
            return;
        }
        let document = self.document;
        if self.app.read(cx).engine.shared_file(document).is_none() {
            return;
        }
        self.shared_selection = Some(selection);
        self.app.update(cx, |app, _| {
            app.engine.share_selection(document, anchor, caret);
        });
    }

    /// Draws the other participants' carets and selections on a shared page, each in their own
    /// colour: the selection filled, a frame round the character at the caret, and a mark in
    /// the gutter. Their names are in the Session panel, so colour is never all that says who.
    fn show_presence(&mut self, cx: &mut Context<Self>) {
        let carets = self.app.read(cx).engine.peer_carets(self.document);
        if carets.is_empty() && self.presence.is_none() {
            return;
        }
        let presence = match &self.presence {
            Some(presence) => presence.clone(),
            None => {
                let presence = self.state.update(cx, |state, cx| Presence {
                    ranges: state.create_range_decorations_collection(Vec::new(), cx),
                    gutter: state.create_gutter_decorations_collection(Vec::new(), cx),
                });
                self.presence = Some(presence.clone());
                presence
            }
        };
        let text = self.state.read(cx).text().clone();
        let len = text.len();
        let mut ranges = Vec::new();
        let mut gutter = Vec::new();
        for caret in &carets {
            let colour = crate::collaboration::peer_colour(caret.peer, cx);
            let (start, end) = (caret.anchor.min(caret.head), caret.anchor.max(caret.head));
            if start < end && end <= len {
                ranges.push(
                    RangeDecoration::new(start..end)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(colour.opacity(0.25)),
                );
            }
            // The caret: the character after it, or before it at the end of the page.
            let head = caret.head.min(len);
            let next = (head + 1..=len)
                .find(|offset| text.is_char_boundary(*offset))
                .filter(|_| head < len);
            let around = match next {
                Some(next) => head..next,
                None => (0..head)
                    .rev()
                    .find(|offset| text.is_char_boundary(*offset))
                    .map_or(head..head, |previous| previous..head),
            };
            if !around.is_empty() {
                ranges.push(RangeDecoration::new(around).with_color(colour));
            }
            gutter.push(
                GutterDecoration::new(head..head.max(around_end(&text, head))).with_color(colour),
            );
        }
        presence.ranges.set(ranges, cx);
        presence.gutter.set(gutter, cx);
        self.shown_carets = carets;
    }

    /// Makes the widget read-only while the page is shared and this participant may not edit
    /// it (a Viewer, out of touch, or waiting to be resynchronised): the engine refuses such
    /// edits whatever the widget does, and this keeps them from being typed at all.
    fn sync_readonly(&mut self, cx: &mut Context<Self>) {
        let shared_readonly = matches!(
            self.app.read(cx).engine.shared_file(self.document),
            Some((_, _, adoc_engine::SharedRole::Guest { can_edit: false }))
        );
        if shared_readonly != self.shared_readonly {
            self.shared_readonly = shared_readonly;
            let readonly = self.readonly || shared_readonly;
            self.state
                .update(cx, |state, cx| state.set_readonly(readonly, cx));
        }
    }

    pub fn document(&self) -> DocumentId {
        self.document
    }

    fn is_path(&self, path: &std::path::Path, cx: &Context<Self>) -> bool {
        self.app
            .read(cx)
            .engine
            .document(self.document)
            .is_some_and(|open| open.path == path)
    }

    #[cfg(test)]
    pub fn state_for_test(&self) -> &Entity<EditorState> {
        &self.state
    }

    /// The caret's zero-based line and column (in UTF-16 code units, as the widget counts).
    pub fn cursor_line_column(&self, cx: &gpui::App) -> (u32, u32) {
        let position = self.state.read(cx).cursor_position();
        (position.line, position.character)
    }

    /// The widget's current text.
    pub fn widget_text(&self, cx: &gpui::App) -> String {
        self.state.read(cx).text().to_string()
    }

    /// The caret's byte offset.
    pub fn cursor(&self, cx: &gpui::App) -> usize {
        self.state.read(cx).cursor()
    }

    /// The selection, as byte offsets.
    pub fn selection(&self, cx: &gpui::App) -> std::ops::Range<usize> {
        self.state.read(cx).selected_range()
    }

    /// Runs `command` on the selection through the engine (a mark such as `edit.toggle_strong`,
    /// or an insert such as `edit.insert_table`), then selects what the engine says to: the
    /// marked text, or where typing goes next.
    pub fn run_on_selection(
        &mut self,
        command: CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection(cx);
        let document = self.document;
        let outcome = self.app.update(cx, |app, cx| {
            app.invoke(
                command,
                CommandArgs::Range {
                    id: document,
                    range: SourceRange::new(selection.start, selection.end),
                },
                cx,
            )
        });
        if let Ok(CommandOutcome::Selection(selection)) = outcome {
            self.reconcile(window, cx);
            self.state.update(cx, |state, cx| {
                let len = state.text().len();
                state.set_selected_range(selection.start.min(len)..selection.end.min(len), cx);
            });
        }
    }

    /// Inserts an xref to the page or section dragged in from the sidebar at the text under
    /// the pointer, or at the caret when there is none, and puts the caret after it.
    pub(crate) fn drop_link(
        &mut self,
        link: &DraggedLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.readonly {
            return;
        }
        let engine = &self.app.read(cx).engine;
        let Some(xref) = engine.xref_to(self.document, &link.path, link.section) else {
            window.push_notification(link.unlinkable(), cx);
            return;
        };
        let at = offset_at(self.state.read(cx), window.mouse_position())
            .unwrap_or_else(|| self.cursor(cx));
        let text = xref.source();
        let document = self.document;
        let inserted = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_INSERT_TEXT,
                CommandArgs::Text {
                    id: document,
                    range: SourceRange::new(at, at),
                    text: text.clone(),
                },
                cx,
            )
        });
        // A failure is reported by the window.
        if inserted.is_err() {
            return;
        }
        self.reconcile(window, cx);
        let caret = at + text.len();
        self.state.update(cx, |state, cx| {
            state.set_selected_range(caret..caret, cx);
            state.focus(window, cx);
        });
    }

    /// Copies `images` where the page can name them and writes them at `at`, with the caret
    /// where the alt text goes.
    fn place_images(
        &mut self,
        images: Vec<ImageSource>,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let document = self.document;
        let outcome = self.app.update(cx, |app, cx| {
            app.invoke(
                ids::EDIT_PLACE_IMAGES,
                CommandArgs::Images {
                    id: document,
                    range: SourceRange::new(at, at),
                    images,
                },
                cx,
            )
        });
        // A failure is reported by the window.
        if let Ok(CommandOutcome::Selection(selection)) = outcome {
            self.reconcile(window, cx);
            self.state.update(cx, |state, cx| {
                state.set_selected_range(selection.start..selection.end, cx);
                state.focus(window, cx);
            });
        }
    }

    /// Image files dropped on the editor go in at the text under the pointer, or at the caret
    /// when there is none.
    fn drop_files(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.readonly {
            return;
        }
        let images = crate::image_paste::dropped_images(paths, window, cx);
        if images.is_empty() {
            return;
        }
        let at = offset_at(self.state.read(cx), window.mouse_position())
            .unwrap_or_else(|| self.cursor(cx));
        self.place_images(images, at, window, cx);
    }

    /// Takes a paste holding an image, returning whether it did; anything else is left to the
    /// widget to paste as text.
    fn paste_image(
        &mut self,
        item: &gpui::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::image_paste::{ask_name, pasted_image, PastedImage};
        let asciidoc = self
            .app
            .read(cx)
            .engine
            .document(self.document)
            .is_some_and(|document| document.asciidoc);
        if self.readonly || !asciidoc {
            return false;
        }
        let at = self.cursor(cx);
        match pasted_image(item) {
            None => false,
            Some(PastedImage::Files(paths)) => {
                let images = paths.into_iter().map(ImageSource::File).collect();
                self.place_images(images, at, window, cx);
                true
            }
            Some(PastedImage::Picture(picture)) => {
                let view = cx.weak_entity();
                let place = move |image, _alt, window: &mut Window, cx: &mut gpui::App| {
                    // The caret stays where it was while the dialog is up.
                    let _ = view.update(cx, |view, cx| {
                        view.place_images(vec![image], at, window, cx);
                    });
                };
                let app = self.app.clone();
                let document = self.document;
                ask_name(&app, document, picture, false, place, window, cx);
                true
            }
        }
    }

    /// Places the caret at `range` and focuses the editor.
    pub fn reveal(&self, range: SourceRange, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let len = state.text().len();
            state.set_selected_range(range.start.min(len)..range.end.min(len), cx);
            state.focus(window, cx);
        });
        // The editor would scroll only far enough to bring the caret to an edge, which can
        // leave it just out of view; the place navigated to goes near the top instead.
        self.scroll_to_line_of(range.start, REVEAL_MARGIN, cx);
    }

    /// While this participant follows another on this page, keeps their caret in view: the
    /// editor scrolls only once it would leave it (#767).
    fn follow_caret(&self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let engine = &app.engine;
        let Some(leader) = engine.following() else {
            return;
        };
        if engine.active() != Some(self.document) {
            return;
        }
        let Some(caret) = engine
            .peer_carets(self.document)
            .into_iter()
            .find(|caret| caret.peer == leader)
        else {
            return;
        };
        let state = self.state.read(cx);
        let text = state.text();
        let line = text.offset_to_position(caret.head.min(text.len())).line as usize;
        let row = state.display_map().buffer_line_to_display_row(line);
        if state
            .visible_row_range()
            .is_some_and(|rows| rows.contains(&row))
        {
            return;
        }
        self.scroll_to_line_of(caret.head, REVEAL_MARGIN, cx);
    }

    /// The person scrolled or clicked in the editor: following another participant ends, as
    /// in Zed (#767).
    fn took_over(&mut self, cx: &mut Context<Self>) {
        if self.app.read(cx).engine.following().is_some() {
            self.app.update(cx, |app, cx| {
                let _ = app.invoke(ids::COLLAB_STOP_FOLLOWING, CommandArgs::None, cx);
            });
        }
    }

    /// Scrolls so the line holding `offset` is at the top, leaving the caret where it is.
    pub fn scroll_to_offset(&self, offset: usize, cx: &mut Context<Self>) {
        self.scroll_to_line_of(offset, 0, cx);
    }

    /// Scrolls so the line holding `offset` is `margin` lines below the top.
    fn scroll_to_line_of(&self, offset: usize, margin: usize, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let Some(line_height) = state.line_height() else {
                return;
            };
            let text = state.text().clone();
            let line = text.offset_to_position(offset.min(text.len())).line as usize;
            let row = state
                .display_map()
                .buffer_line_to_display_row(line)
                .saturating_sub(margin);
            let x = state.scroll_offset().x;
            state.set_scroll_offset(gpui::point(x, -(line_height * row as f32)), cx);
        });
    }

    /// The other participants' carets as last drawn, for tests.
    #[cfg(test)]
    pub fn shown_carets(&self) -> &[adoc_engine::PeerCaret] {
        &self.shown_carets
    }

    /// The first source line shown, for tests of scroll sync.
    #[cfg(test)]
    pub fn top_line(&self, cx: &gpui::App) -> Option<usize> {
        let state = self.state.read(cx);
        let line_height = state.line_height()?;
        let row = (-state.scroll_offset().y / line_height).round() as usize;
        Some(state.display_map().display_row_to_buffer_line(row))
    }

    /// Applies the engine's text to the widget when they differ, as one replacement, keeping
    /// the selection on the text it was on.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine_text) = self
            .app
            .read(cx)
            .engine
            .document(self.document)
            .map(|open| open.text.clone())
        else {
            return;
        };
        // Whatever the widget made before is settled now.
        self.own_versions.clear();
        self.state.update(cx, |state, cx| {
            // An input method's composition in progress: provisional text the widget holds and
            // the engine does not, until it is committed.
            let composing = state.marked_text_range(window, cx).map(|utf16| {
                let text = state.text();
                let marked = text.offset_utf16_to_offset(utf16.start)
                    ..text.offset_utf16_to_offset(utf16.end);
                let composed = text.slice(marked.clone()).to_string();
                (marked, composed)
            });
            match composing {
                None => {
                    let Some((range, replacement)) = sync::diff(state.text(), &engine_text) else {
                        return;
                    };
                    adoc_core::probe::hit(adoc_core::probe::Probe::Reconcile);
                    apply_change(state, &range, replacement, window, cx);
                }
                Some((marked, composed)) => {
                    // The widget's text as the engine should have it: without the composition.
                    let mut base = state.text().clone();
                    base.remove(marked.clone());
                    let Some((range, replacement)) = sync::diff(&base, &engine_text) else {
                        return;
                    };
                    adoc_core::probe::hit(adoc_core::probe::Probe::Reconcile);
                    // A replacement goes to the composition rather than the selection while it
                    // lasts: end it, apply the change, and mark it again where the change moved
                    // it, so the input method goes on. A change reaching across the
                    // composition's place ends it, the engine's text winning.
                    state.unmark_text(window, cx);
                    let around = range.end <= marked.start || range.start >= marked.start;
                    if around {
                        let shift = if range.start >= marked.start {
                            composed.len()
                        } else {
                            0
                        };
                        let range = range.start + shift..range.end + shift;
                        let inserted = replacement.len();
                        apply_change(state, &range, replacement, window, cx);
                        let start = sync::map_offset(marked.start, &range, inserted);
                        let text = state.text();
                        let utf16 = text.offset_to_offset_utf16(start)
                            ..text.offset_to_offset_utf16(start + composed.len());
                        state.replace_and_mark_text_in_range(
                            Some(utf16),
                            &composed,
                            None,
                            window,
                            cx,
                        );
                    } else if let Some((range, replacement)) =
                        sync::diff(state.text(), &engine_text)
                    {
                        apply_change(state, &range, replacement, window, cx);
                    }
                }
            }
            // The widget reports the change too, and the engine has it already.
            self.synced.replace(Some(state.text().clone()));
        });
    }

    fn show_diagnostics(&mut self, cx: &mut Context<Self>) {
        let engine = &self.app.read(cx).engine;
        let diagnostics = engine
            .document(self.document)
            .map(|open| open.diagnostics.clone())
            .unwrap_or_default();
        let misspellings = engine.misspellings(self.document).to_vec();
        self.state.update(cx, |state, cx| {
            let text = state.text().clone();
            let Some(set) = state.diagnostics_mut() else {
                return;
            };
            set.clear();
            set.extend(diagnostics.into_iter().map(|diagnostic| {
                let start = text.offset_to_position(diagnostic.range.start.min(text.len()));
                let end = text.offset_to_position(diagnostic.range.end.min(text.len()));
                Diagnostic::new(start..end, diagnostic.message)
                    .with_severity(match diagnostic.severity {
                        DiagnosticSeverity::Error => WidgetSeverity::Error,
                        DiagnosticSeverity::Warning => WidgetSeverity::Warning,
                        DiagnosticSeverity::Information => WidgetSeverity::Info,
                    })
                    .with_code(diagnostic.code.as_str())
            }));
            set.extend(misspellings.into_iter().map(|misspelling| {
                let start = text.offset_to_position(misspelling.range.start.min(text.len()));
                let end = text.offset_to_position(misspelling.range.end.min(text.len()));
                Diagnostic::new(start..end, format!("Unknown word: {}", misspelling.word))
                    .with_severity(WidgetSeverity::Hint)
                    .with_code(adoc_engine::SPELLING_CODE)
            }));
            cx.notify();
        });
    }

    /// Opens the card of the `index`th change, clicked at `position`.
    pub fn open_change(
        &mut self,
        index: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let Some(hunk) = engine.git().hunks_for(self.document).get(index).cloned() else {
            return;
        };
        let head = engine.head_text(self.document, &hunk).unwrap_or_default();
        self.open_change = Some((hunk, head, position));
        cx.notify();
    }

    fn close_change(&mut self, cx: &mut Context<Self>) {
        if self.open_change.take().is_some() {
            cx.notify();
        }
    }

    /// Replaces the open change with its text in `HEAD`, as one undo step.
    pub(crate) fn revert_open_change(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some((hunk, _, _)) = self.open_change.take() else {
            return;
        };
        let args = CommandArgs::Offset {
            id: self.document,
            offset: hunk.range.start,
        };
        // A failure is reported by the window, through `EngineEvent::CommandFailed`.
        let _ = self
            .app
            .update(cx, |app, cx| app.invoke(ids::GIT_REVERT_HUNK, args, cx));
        cx.notify();
    }

    /// Shows or hides the blame column, asking the engine for the blame when shown.
    pub fn toggle_blame(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let document = self.document;
        let result = self.app.update(cx, |app, cx| {
            app.invoke(ids::GIT_SHOW_BLAME, CommandArgs::Document(document), cx)
        });
        // A failure is reported by the window, through `EngineEvent::CommandFailed`.
        if result.is_ok() {
            self.show_blame = !self.show_blame;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn shows_blame(&self) -> bool {
        self.show_blame
    }

    /// The pointer moved over gutter line `line` (or off the gutter): after a rest of 400 ms
    /// there, when `[git] blame_on_hover` is set, the line's blame shows in a tooltip.
    pub fn hover_gutter(
        &mut self,
        line: Option<usize>,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if line == self.hover_line {
            return;
        }
        self.hover_line = line;
        if self.hover_tip.take().is_some() {
            cx.notify();
        }
        self._hover = None;
        let Some(line) = line else {
            return;
        };
        if !self.app.read(cx).engine.settings().git.blame_on_hover {
            return;
        }
        let document = self.document;
        self.app
            .update(cx, |app, _| app.engine.request_blame(document));
        self._hover = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.hover_line != Some(line) {
                    return;
                }
                let blame = this.app.read(cx).engine.blame_for(document);
                if let Some(blamed) = blame.and_then(|blame| blame.get(line).cloned()) {
                    let text = blame::describe(&blamed, blame::now());
                    this.hover_tip = Some((text, position));
                    cx.notify();
                }
            });
        }));
    }

    /// The blame tooltip's text, once shown.
    #[cfg(test)]
    pub fn hover_tip_for_test(&self) -> Option<String> {
        self.hover_tip.as_ref().map(|(text, _)| text.clone())
    }

    /// The change markers as last painted: their kinds and bounds.
    #[cfg(test)]
    pub fn marks_for_test(&self) -> Vec<changes::GutterMark> {
        self.marks.borrow().clone()
    }

    /// The messages of the spelling hints the editor shows.
    #[cfg(test)]
    pub fn spelling_hints_for_test(&self, cx: &gpui::App) -> Vec<String> {
        self.state
            .read(cx)
            .diagnostics()
            .map(|set| {
                set.iter()
                    .filter(|entry| {
                        entry.diagnostic.code.as_deref() == Some(adoc_engine::SPELLING_CODE)
                    })
                    .map(|entry| entry.diagnostic.message.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The text in `HEAD` shown by the open change card.
    #[cfg(test)]
    pub fn open_change_for_test(&self) -> Option<String> {
        self.open_change.as_ref().map(|(_, head, _)| head.clone())
    }

    /// Types `text` at the caret as a keystroke would, for the typing benchmark.
    pub fn type_for_benchmark(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let _timed = crate::typing_bench::timed("type");
        self.state
            .update(cx, |state, cx| state.insert(text.to_owned(), window, cx));
    }

    /// Runs the engine's `edit.undo` or `edit.redo` for this document.
    pub fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let document = self.document;
        let command = if redo { ids::EDIT_REDO } else { ids::EDIT_UNDO };
        self.app.update(cx, |app, cx| {
            let _ = app.invoke(command, CommandArgs::Document(document), cx);
        });
    }
}

impl SourceEditorView {
    /// The card of an open change: the lines it replaced in `HEAD`, with Revert and Copy.
    fn change_card(
        &self,
        hunk: &adoc_engine::Hunk,
        head: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{
            prelude::FluentBuilder as _, InteractiveElement as _, StatefulInteractiveElement as _,
        };
        use gpui_component::{
            button::{Button, ButtonVariants as _},
            h_flex, v_flex, Sizable as _,
        };
        let title = match hunk.kind {
            adoc_engine::HunkKind::Added => "Added since the last commit",
            adoc_engine::HunkKind::Modified => "Before this change",
            adoc_engine::HunkKind::Deleted => "Deleted since the last commit",
        };
        let lines: Vec<String> = head.lines().map(str::to_owned).collect();
        let copy = head.to_owned();
        gpui::deferred(
            gpui::anchored().position(position).snap_to_window().child(
                v_flex()
                    .id("change-card")
                    .debug_selector(|| "change-card".to_owned())
                    .occlude()
                    .gap_2()
                    .p_2()
                    .max_w(gpui::px(560.))
                    .bg(cx.theme().popover)
                    .text_color(cx.theme().popover_foreground)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .shadow_md()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_change(cx)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(title),
                    )
                    .when(!lines.is_empty(), |card| {
                        card.child(
                            v_flex()
                                .id("change-head")
                                .max_h(gpui::px(240.))
                                .overflow_y_scroll()
                                .p_1()
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .font_family(
                                    crate::fonts::source(self.app.read(cx).engine.settings(), cx).0,
                                )
                                .text_sm()
                                .children(lines.into_iter().map(|line| {
                                    div().child(if line.is_empty() {
                                        " ".to_owned()
                                    } else {
                                        line
                                    })
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("revert-change")
                                    .small()
                                    .primary()
                                    .label("Revert Change")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.revert_open_change(window, cx);
                                    })),
                            )
                            .when(!copy.is_empty(), |row| {
                                row.child(
                                    Button::new("copy-change")
                                        .small()
                                        .ghost()
                                        .label("Copy")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy.clone(),
                                            ));
                                            this.close_change(cx);
                                        })),
                                )
                            }),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl gpui::EventEmitter<SourceEditorEvent> for SourceEditorView {}

impl Focusable for SourceEditorView {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for SourceEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::renders::rendered(crate::renders::View::Editor);
        let _timed = crate::typing_bench::timed("render.editor");
        let settings = self.app.read(cx).engine.settings();
        let (font_family, font_size) = crate::fonts::source(settings, cx);
        let font_size = font_size as f32 * settings.appearance.font_scale.factor();
        let engine = &self.app.read(cx).engine;
        let hunks = if engine.settings().git.show_markers {
            changes::near_view(self.state.read(cx), engine.git().hunks_for(self.document))
        } else {
            Vec::new()
        };
        let colours = changes::MarkColours {
            added: cx.theme().success,
            modified: cx.theme().info,
            deleted: cx.theme().danger,
        };
        let blame = self
            .show_blame
            .then(|| engine.blame_for(self.document))
            .flatten();
        let hover = engine.settings().git.blame_on_hover;
        let card = self
            .open_change
            .as_ref()
            .map(|(hunk, head, position)| self.change_card(hunk, head, *position, cx));
        let tip = self.hover_tip.as_ref().map(|(text, position)| {
            gpui::deferred(
                gpui::anchored()
                    .position(*position + gpui::point(gpui::px(12.), gpui::px(12.)))
                    .snap_to_window()
                    .child(
                        div()
                            .debug_selector(|| "blame-tip".to_owned())
                            .px_2()
                            .py_1()
                            .text_xs()
                            .bg(cx.theme().popover)
                            .text_color(cx.theme().popover_foreground)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .child(text.clone()),
                    ),
            )
            .with_priority(1)
        });
        let font = gpui::font(font_family.clone());
        // Sized from the source font, so its text keeps to it at any size or scale.
        let blame_size = blame::text_size(font_size);
        let blame_width = self.show_blame.then(|| {
            let text_system = window.text_system();
            let id = text_system.resolve_font(&font);
            blame::column_width(text_system.em_advance(id, blame_size).ok())
        });
        let editor = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.took_over(cx)))
            .child({
                // The editor handles the wheel itself and keeps it from its parent, so the turn
                // is caught before it, in the capture phase.
                let this = cx.entity().downgrade();
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        window.on_mouse_event(move |event: &gpui::ScrollEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture
                                && bounds.contains(&event.position)
                            {
                                let _ = this.update(cx, |this, cx| this.took_over(cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_full()
            })
            .child(
                Editor::new(&self.state)
                    .bordered(false)
                    .h(relative(1.))
                    .font_family(font_family)
                    .text_size(gpui::px(font_size))
                    .text_color(cx.theme().foreground)
                    .context_menu({
                        let app = self.app.clone();
                        let state = self.state.clone();
                        let document = self.document;
                        let readonly = self.readonly;
                        move |menu, _, cx| context_menu(menu, &app, &state, document, readonly, cx)
                    })
                    .on_paste({
                        let view = cx.weak_entity();
                        move |item, window, cx| {
                            view.update(cx, |view, cx| view.paste_image(item, window, cx))
                                .unwrap_or(false)
                        }
                    }),
            )
            .child(changes::overlay(
                cx.weak_entity(),
                self.state.clone(),
                hunks,
                self.marks.clone(),
                colours,
            ))
            .when(hover, |editor| {
                editor.child(blame::hover_watch(cx.weak_entity(), self.state.clone()))
            })
            .on_drop(cx.listener(|this, link: &DraggedLink, window, cx| {
                this.drop_link(link, window, cx);
            }))
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                    this.drop_files(paths, window, cx);
                }),
            )
            .when_some(blame, |editor, blame| {
                // Painted after the editor, into the space kept for it on the left.
                editor.child(blame::column(
                    self.state.clone(),
                    blame,
                    font,
                    blame_size,
                    cx.theme().muted_foreground,
                    blame_width.unwrap_or_default(),
                ))
            });
        gpui_component::h_flex()
            .size_full()
            .on_action(
                cx.listener(|this, action: &crate::spelling::ApplySpelling, _, cx| {
                    if action.document != this.document {
                        return;
                    }
                    // A failure is reported by the window.
                    let _ = crate::spelling::apply(&this.app, action.document, &action.choice, cx);
                }),
            )
            .when_some(blame_width, |row, width| {
                row.child(
                    div()
                        .debug_selector(|| "blame-column".to_owned())
                        .flex_none()
                        .h_full()
                        .w(width)
                        .bg(cx.theme().muted),
                )
            })
            .child(editor)
            .children(card)
            .children(tip)
    }
}

/// The editor's menu: on a misspelt word its suggestions and dictionaries first, then the
/// editor's own items, which a custom menu must list itself.
fn context_menu(
    mut menu: NativeMenu,
    app: &Entity<AppState>,
    state: &Entity<EditorState>,
    document: DocumentId,
    readonly: bool,
    cx: &gpui::App,
) -> NativeMenu {
    use gpui_component::input::{Copy, Cut, GoToDefinition, Paste, SelectAll};
    let editor = state.read(cx);
    if !readonly {
        let choices = crate::spelling::choices(&app.read(cx).engine, document, editor.cursor());
        let mut suggested = false;
        for (label, choice) in choices.iter().cloned() {
            let replace = matches!(choice, crate::spelling::Choice::Replace { .. });
            if !replace && suggested {
                menu = menu.separator();
                suggested = false;
            }
            suggested |= replace;
            menu = menu.menu(
                label,
                Box::new(crate::spelling::ApplySpelling { document, choice }),
            );
        }
        if !choices.is_empty() {
            menu = menu.separator();
        }
    }
    let capabilities = editor.context_menu_capabilities();
    let editable = capabilities.is_editable();
    menu.menu_with_disabled(
        "Go to Definition",
        !capabilities.has_definition(),
        Box::new(GoToDefinition),
    )
    .separator()
    .menu_with_disabled(
        "Cut",
        !(editable && capabilities.is_copyable()),
        Box::new(Cut),
    )
    .menu_with_disabled("Copy", !capabilities.is_copyable(), Box::new(Copy))
    .menu_with_disabled("Paste", !editable, Box::new(Paste))
    .separator()
    .menu("Select All", Box::new(SelectAll))
}
