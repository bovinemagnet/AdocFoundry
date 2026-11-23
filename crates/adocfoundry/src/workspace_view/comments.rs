//! The Comments panel (#762, COMMENTS 3/4): a sidebar tab listing the active page's comments, as
//! the engine holds them (alone, in the page's `.afd` file; in a session, as the host told
//! them). It works nothing out: every change is a `comments.*` command, and what the user may
//! do to each comment is the engine's `Allowed`.
//!
//! Open comments come first, in the order of their text; resolved ones are folded under a
//! heading, and detached ones (their text gone) are listed with the text they were on. Choosing
//! a comment selects its text in the editor. Add Comment opens a box for a new comment on the
//! selection; Reply and Edit open one under the comment.
//!
//! Suggest Change (#763, SUGGEST 3/4) opens a box with the selected text to change and a note.
//! A suggestion's card shows the change, the part accepting would remove struck through and the
//! part it would add underlined, and offers Accept and Reject as the engine allows; decided ones
//! fold with resolved comments.

use adoc_engine::{
    Comment, CommentId, CommentRequest, CommentsState, SeenSuggestion, SuggestionState,
};
use gpui::{AnyElement, HighlightStyle, StrikethroughStyle, StyledText, UnderlineStyle};

use super::*;
use crate::editor::bindings::{AddComment, NextComment, PreviousComment, SuggestChange};

/// What the panel is writing: a new comment on a range, a reply, an edit, or a suggested change,
/// with its boxes.
pub(super) enum Draft {
    New {
        document: DocumentId,
        range: SourceRange,
        input: Entity<gpui_component::input::TextareaState>,
    },
    Reply {
        document: DocumentId,
        comment: CommentId,
        input: Entity<gpui_component::input::TextareaState>,
    },
    Edit {
        document: DocumentId,
        comment: CommentId,
        reply: Option<CommentId>,
        input: Entity<gpui_component::input::TextareaState>,
    },
    /// A change to the text of `range`, which held `original` when the box opened:
    /// `replacement` begins as that text, `input` is the note. `range` follows the page's
    /// edits while the box is open (`follow_edits`); `version` is the page's version it is in.
    Suggest {
        document: DocumentId,
        range: SourceRange,
        version: u64,
        original: String,
        replacement: Entity<gpui_component::input::TextareaState>,
        input: Entity<gpui_component::input::TextareaState>,
    },
    /// A new replacement and note for an undecided suggestion.
    EditSuggestion {
        document: DocumentId,
        comment: CommentId,
        replacement: Entity<gpui_component::input::TextareaState>,
        input: Entity<gpui_component::input::TextareaState>,
    },
}

impl Draft {
    fn input(&self) -> &Entity<gpui_component::input::TextareaState> {
        match self {
            Self::New { input, .. }
            | Self::Reply { input, .. }
            | Self::Edit { input, .. }
            | Self::Suggest { input, .. }
            | Self::EditSuggestion { input, .. } => input,
        }
    }

    /// The replacement's box, for a suggested change.
    fn replacement(&self) -> Option<&Entity<gpui_component::input::TextareaState>> {
        match self {
            Self::Suggest { replacement, .. } | Self::EditSuggestion { replacement, .. } => {
                Some(replacement)
            }
            Self::New { .. } | Self::Reply { .. } | Self::Edit { .. } => None,
        }
    }

    fn document(&self) -> DocumentId {
        match self {
            Self::New { document, .. }
            | Self::Reply { document, .. }
            | Self::Edit { document, .. }
            | Self::Suggest { document, .. }
            | Self::EditSuggestion { document, .. } => *document,
        }
    }
}

/// The panel's own state: the box being written in, and whether resolved comments are shown.
#[derive(Default)]
pub(super) struct CommentsPanel {
    pub(super) draft: Option<Draft>,
    pub(super) show_resolved: bool,
}

/// When a comment was written, as a reader would say it: today's time, else its date, in UTC.
fn when(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let day = seconds / 86_400;
    let (hour, minute) = (seconds % 86_400 / 3_600, seconds % 3_600 / 60);
    if day == now / 86_400 {
        format!("{hour:02}:{minute:02} UTC")
    } else {
        // Days to a civil date, as the engine writes its times.
        let days = day as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let of_era = days - era * 146_097;
        let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
        let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * of_year + 2) / 153;
        let day_of_month = of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        format!("{year}-{month:02}-{day_of_month:02}")
    }
}

/// The quoted text as the panel shows it: one line, at most 80 characters.
fn shown_quote(quote: &str) -> String {
    let line: String = quote.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line
    }
}

/// What a suggestion's card reads aloud: who suggests what, as the change accepting would make.
fn suggestion_label(author: &str, removed: &str, inserted: &str) -> String {
    let (deletes, inserts) = (inserted.is_empty(), removed.is_empty());
    let (removed, inserted) = (shown_quote(removed), shown_quote(inserted));
    if deletes {
        format!("Suggestion by {author}: delete \u{2018}{removed}\u{2019}")
    } else if inserts {
        format!("Suggestion by {author}: insert \u{2018}{inserted}\u{2019}")
    } else {
        format!(
            "Suggestion by {author}: replace \u{2018}{removed}\u{2019} with \u{2018}{inserted}\u{2019}"
        )
    }
}

/// The frame of comment `key`'s card: a group that assistive technology reads by `label`, who
/// wrote it and on what (or, for a suggestion, the change in words), so neither the colour in
/// the editor nor a suggestion's strike and underline is all that says what it is.
fn comment_card(key: &str, label: String) -> gpui::Stateful<gpui::Div> {
    let selector = format!("comment-{key}");
    v_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .role(gpui::Role::Group)
        .aria_label(label)
}

/// The part of `text` between `prefix` bytes from its start and `suffix` from its end, or all of
/// it when that is not a part of it.
fn middle(text: &str, prefix: usize, suffix: usize) -> std::ops::Range<usize> {
    let end = text.len().saturating_sub(suffix);
    if prefix <= end && text.is_char_boundary(prefix) && text.is_char_boundary(end) {
        prefix..end
    } else {
        0..text.len()
    }
}

impl WorkspaceView {
    /// The active editor's document and selection, when a comment can be made on it; else says
    /// why not, `empty` being what to say for an empty selection.
    fn selection_to_comment_on(
        &self,
        empty: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(DocumentId, std::ops::Range<usize>, String)> {
        let editor = self.active_editor()?.read(cx);
        let (document, selection) = (editor.document(), editor.selection(cx));
        if selection.is_empty() {
            window.push_notification(empty, cx);
            return None;
        }
        let selected = editor.widget_text(cx).get(selection.clone())?.to_owned();
        if let CommentsState::Unusable(reason) | CommentsState::ReadOnly(reason) =
            self.app.read(cx).engine.comments_state(document)
        {
            window.push_notification(reason, cx);
            return None;
        }
        Some((document, selection, selected))
    }

    /// Add Comment: a box in the Comments panel for a comment on the active editor's selection.
    pub(super) fn on_add_comment(
        &mut self,
        _: &AddComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((document, selection, _)) =
            self.selection_to_comment_on("Select the text to comment on first.", window, cx)
        else {
            return;
        };
        let input = cx.new(|cx| {
            gpui_component::input::TextareaState::new(window, cx).placeholder("Write a comment")
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.comments.draft = Some(Draft::New {
            document,
            range: SourceRange::new(selection.start, selection.end),
            input,
        });
        self.sidebar_tab = SidebarTab::Comments;
        cx.notify();
    }

    /// Suggest Change: a box in the Comments panel for a change to the active editor's
    /// selection, its replacement begun as the selected text, all selected so typing replaces it.
    pub(super) fn on_suggest_change(
        &mut self,
        _: &SuggestChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((document, selection, selected)) =
            self.selection_to_comment_on("Select the text to change first.", window, cx)
        else {
            return;
        };
        let range = SourceRange::new(selection.start, selection.end);
        let original = selected.clone();
        let version = self
            .app
            .read(cx)
            .engine
            .document(document)
            .map_or(0, |open| open.version);
        self.open_suggestion_box(
            |replacement, input| Draft::Suggest {
                document,
                range,
                version,
                original,
                replacement,
                input,
            },
            &selected,
            "",
            window,
            cx,
        );
        self.sidebar_tab = SidebarTab::Comments;
    }

    /// Keeps the Suggest box on its text: its range moves with every edit made to the page
    /// while it is open, as comments' ranges do, so only a change to the text itself refuses
    /// the suggestion.
    pub(super) fn follow_edits(&mut self, event: &EngineEvent) {
        let EngineEvent::DocumentChanged {
            id,
            version: after,
            edits,
            ..
        } = event
        else {
            return;
        };
        if let Some(Draft::Suggest {
            document,
            range,
            version,
            ..
        }) = &mut self.comments.draft
        {
            if document == id && after > version {
                *range = adoc_engine::map_range(*range, edits);
                *version = *after;
            }
        }
    }

    /// Next Comment and Previous Comment: select the open comment after or before the caret.
    pub(super) fn on_next_comment(
        &mut self,
        _: &NextComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_comment(ids::COMMENTS_NEXT, window, cx);
    }

    pub(super) fn on_previous_comment(
        &mut self,
        _: &PreviousComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_comment(ids::COMMENTS_PREVIOUS, window, cx);
    }

    fn step_comment(
        &mut self,
        command: adoc_engine::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let (id, offset) = {
            let editor = editor.read(cx);
            (editor.document(), editor.cursor(cx))
        };
        if let Some(CommandOutcome::Selection(range)) = self.invoke(
            command,
            CommandArgs::Comment(CommentRequest::Step { id, offset }),
            window,
            cx,
        ) {
            editor.update(cx, |editor, cx| editor.select_range(range, window, cx));
        }
    }

    /// Selects comment `range` in the active editor.
    fn show_comment(&mut self, range: SourceRange, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.select_range(range, window, cx));
        }
    }

    /// Sends what the open box holds: a new comment, a reply or an edit.
    fn submit_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.comments.draft.take() else {
            return;
        };
        let text = draft.input().read(cx).value().to_string();
        let replacement = draft
            .replacement()
            .map(|replacement| replacement.read(cx).value().to_string());
        // A suggestion's note may be empty; a comment's text may not.
        let blank_allowed = match &draft {
            Draft::Suggest { .. } | Draft::EditSuggestion { .. } => true,
            Draft::Edit {
                document,
                comment,
                reply: None,
                ..
            } => self
                .app
                .read(cx)
                .engine
                .comments(*document)
                .iter()
                .any(|shown| shown.id == *comment && shown.suggestion.is_some()),
            Draft::New { .. } | Draft::Reply { .. } | Draft::Edit { .. } => false,
        };
        if !blank_allowed && text.trim().is_empty() {
            self.comments.draft = Some(draft);
            return;
        }
        let (command, request) = match &draft {
            Draft::New {
                document, range, ..
            } => (
                ids::COMMENTS_ADD,
                CommentRequest::Add {
                    id: *document,
                    range: *range,
                    text,
                },
            ),
            Draft::Reply {
                document, comment, ..
            } => (
                ids::COMMENTS_REPLY,
                CommentRequest::Reply {
                    id: *document,
                    comment: *comment,
                    text,
                },
            ),
            Draft::Edit {
                document,
                comment,
                reply,
                ..
            } => (
                ids::COMMENTS_EDIT,
                CommentRequest::Edit {
                    id: *document,
                    comment: *comment,
                    reply: *reply,
                    text,
                },
            ),
            Draft::Suggest {
                document,
                range,
                original,
                ..
            } => (
                ids::COMMENTS_SUGGEST,
                CommentRequest::Suggest {
                    id: *document,
                    range: *range,
                    // Refused if the range no longer holds the text the box was opened on.
                    original: Some(original.clone()),
                    replacement: replacement.unwrap_or_default(),
                    text,
                },
            ),
            Draft::EditSuggestion {
                document, comment, ..
            } => (
                ids::COMMENTS_EDIT,
                CommentRequest::EditSuggestion {
                    id: *document,
                    comment: *comment,
                    replacement: replacement.unwrap_or_default(),
                    text,
                },
            ),
        };
        let suggests = draft.replacement().is_some();
        if self
            .invoke(command, CommandArgs::Comment(request), window, cx)
            .is_none()
            && suggests
        {
            // Refused (the window says why): the replacement typed is kept to be put right.
            self.comments.draft = Some(draft);
            cx.notify();
            return;
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// Back to the page once a box is sent or put away.
    fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            let focus = gpui::Focusable::focus_handle(editor.read(cx), cx);
            window.focus(&focus, cx);
        }
    }

    fn open_box(
        &mut self,
        make: impl FnOnce(Entity<gpui_component::input::TextareaState>) -> Draft,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        let input = cx.new(|cx| {
            let mut input =
                gpui_component::input::TextareaState::new(window, cx).placeholder("Write a reply");
            input.set_value(text, window, cx);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.comments.draft = Some(make(input));
        cx.notify();
    }

    /// Opens the boxes of a suggested change: its replacement, holding `replacement` all
    /// selected and focused, and its note, holding `note`.
    fn open_suggestion_box(
        &mut self,
        make: impl FnOnce(
            Entity<gpui_component::input::TextareaState>,
            Entity<gpui_component::input::TextareaState>,
        ) -> Draft,
        replacement: &str,
        note: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (replacement, note) = (replacement.to_owned(), note.to_owned());
        let replacement = cx.new(|cx| {
            let mut input = gpui_component::input::TextareaState::new(window, cx)
                .placeholder("Replace with (empty to delete)");
            input.set_value(replacement, window, cx);
            input
        });
        let input = cx.new(|cx| {
            let mut input = gpui_component::input::TextareaState::new(window, cx)
                .placeholder("Add a note (optional)");
            input.set_value(note, window, cx);
            input
        });
        replacement.update(cx, |replacement, cx| {
            replacement.focus(window, cx);
            replacement.select_all(window, cx);
        });
        self.comments.draft = Some(make(replacement, input));
        cx.notify();
    }

    /// The box being written in, with Comment (or Save) and Cancel.
    fn render_draft(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let label = match draft {
            Draft::New { .. } => "Comment",
            Draft::Reply { .. } => "Reply",
            Draft::Suggest { .. } => "Suggest",
            Draft::Edit { .. } | Draft::EditSuggestion { .. } => "Save",
        };
        v_flex()
            .gap_1()
            .when_some(draft.replacement(), |boxes, replacement| {
                boxes.child(
                    div()
                        .debug_selector(|| "suggestion-replacement".to_owned())
                        .h(px(48.))
                        .child(
                            gpui_component::input::Textarea::new(replacement)
                                .aria_label("Replace with")
                                .h(px(48.)),
                        ),
                )
            })
            .child(
                div()
                    .debug_selector(|| "comment-box".to_owned())
                    .h(px(72.))
                    .child(gpui_component::input::Textarea::new(draft.input()).h(px(72.))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("comment-submit")
                            .label(label)
                            .small()
                            .primary()
                            .debug_selector(|| "comment-submit".to_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_draft(window, cx);
                            })),
                    )
                    .child(
                        Button::new("comment-cancel")
                            .label("Cancel")
                            .small()
                            .debug_selector(|| "comment-cancel".to_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.comments.draft = None;
                                this.focus_editor(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    /// One comment, its replies, its buttons and any box open under it.
    fn render_comment(
        &self,
        document: DocumentId,
        comment: &Comment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let id = comment.id;
        let key = format!("{id}");
        let range = comment.range;
        let detached = comment.detached;
        let resolved = comment.resolved.is_some();
        let suggestion = comment.suggestion.as_ref();
        let decided = suggestion.is_some_and(|suggestion| suggestion.state.decided());
        // The parts accepting would remove and add: while pending, what is left of the range
        // and the replacement once their common start and end are taken off.
        let parts = suggestion.map(|suggestion| {
            let prefix = suggestion.removed.start.saturating_sub(range.start);
            let suffix = range.end.saturating_sub(suggestion.removed.end);
            (
                middle(&suggestion.original, prefix, suffix),
                middle(&suggestion.replacement, prefix, suffix),
            )
        });
        let label = match (suggestion, &parts) {
            (Some(suggestion), Some((removed, _))) => suggestion_label(
                &comment.author.name,
                &suggestion.original[removed.clone()],
                &suggestion.inserted,
            ),
            _ => format!(
                "Comment by {} on \u{201c}{}\u{201d}",
                comment.author.name,
                shown_quote(&comment.quote)
            ),
        };
        let button = |name: &str, title: &str| {
            let selector = format!("comment-{name}-{key}");
            Button::new(SharedString::from(selector.clone()))
                .label(title.to_owned())
                .small()
                .ghost()
                .debug_selector(move || selector.clone())
        };
        // A suggestion's Accept and Reject come first.
        let mut decisions = Vec::new();
        if let Some(suggestion) = suggestion {
            if comment.can.accept && suggestion.state == SuggestionState::Pending {
                // The change this card shows: the engine refuses it if the suggestion is no
                // longer that.
                let seen = SeenSuggestion {
                    original: suggestion.original.clone(),
                    replacement: suggestion.replacement.clone(),
                };
                decisions.push(button("accept", "Accept").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.invoke(
                            ids::COMMENTS_ACCEPT,
                            CommandArgs::Comment(CommentRequest::Accept {
                                id: document,
                                comment: id,
                                seen: Some(seen.clone()),
                            }),
                            window,
                            cx,
                        );
                    },
                )));
            }
            if comment.can.reject {
                decisions.push(button("reject", "Reject").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.invoke(
                            ids::COMMENTS_REJECT,
                            CommandArgs::Comment(CommentRequest::Reject {
                                id: document,
                                comment: id,
                            }),
                            window,
                            cx,
                        );
                    },
                )));
            }
        }
        let mut buttons = h_flex().gap_1().flex_wrap().children(decisions).child(
            button("reply", "Reply").on_click(cx.listener(move |this, _, window, cx| {
                this.open_box(
                    |input| Draft::Reply {
                        document,
                        comment: id,
                        input,
                    },
                    "",
                    window,
                    cx,
                );
            })),
        );
        // A suggestion is resolved by accepting or rejecting it, and reopened once decided.
        if comment.can.resolve && suggestion.is_none() {
            let (command, resolve, title) = if resolved {
                (ids::COMMENTS_REOPEN, false, "Reopen")
            } else {
                (ids::COMMENTS_RESOLVE, true, "Resolve")
            };
            buttons = buttons.child(button("resolve", title).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.invoke(
                        command,
                        CommandArgs::Comment(CommentRequest::Resolve {
                            id: document,
                            comment: id,
                            resolved: resolve,
                        }),
                        window,
                        cx,
                    );
                },
            )));
        }
        if comment.can.edit && suggestion.is_some_and(|_| !decided) {
            // Its replacement and note; a decided one's note alone, below.
            let (replacement, text) = (
                suggestion.map_or_else(String::new, |suggestion| suggestion.replacement.clone()),
                comment.text.clone(),
            );
            buttons = buttons.child(button("edit", "Edit").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.open_suggestion_box(
                        |replacement, input| Draft::EditSuggestion {
                            document,
                            comment: id,
                            replacement,
                            input,
                        },
                        &replacement,
                        &text,
                        window,
                        cx,
                    );
                },
            )));
        } else if comment.can.edit {
            let text = comment.text.clone();
            buttons = buttons.child(button("edit", "Edit").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.open_box(
                        |input| Draft::Edit {
                            document,
                            comment: id,
                            reply: None,
                            input,
                        },
                        &text,
                        window,
                        cx,
                    );
                },
            )));
        }
        if comment.can.delete {
            buttons = buttons.child(button("delete", "Delete").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.invoke(
                        ids::COMMENTS_DELETE,
                        CommandArgs::Comment(CommentRequest::Delete {
                            id: document,
                            comment: id,
                            reply: None,
                        }),
                        window,
                        cx,
                    );
                },
            )));
        }
        if comment.can.resolve && decided {
            buttons = buttons.child(button("reopen", "Reopen").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.invoke(
                        ids::COMMENTS_REOPEN,
                        CommandArgs::Comment(CommentRequest::Resolve {
                            id: document,
                            comment: id,
                            resolved: false,
                        }),
                        window,
                        cx,
                    );
                },
            )));
        }
        // A suggestion shows its change where a comment shows the text it is on.
        let place = match suggestion.zip(parts) {
            Some((suggestion, (removed, inserted))) => {
                self.render_change(&key, suggestion, removed, inserted, !detached, range, cx)
            }
            None => div()
                .id(SharedString::from(format!("comment-quote-{key}")))
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .when(detached, |quote| quote.italic())
                .child(if detached {
                    format!("Was on \u{201c}{}\u{201d}", shown_quote(&comment.quote))
                } else {
                    format!("\u{201c}{}\u{201d}", shown_quote(&comment.quote))
                })
                .when(!detached, |quote| {
                    quote.on_click(cx.listener(move |this, _, window, cx| {
                        this.show_comment(range, window, cx);
                    }))
                })
                .into_any_element(),
        };
        let mut card = comment_card(&key, label.clone())
            .p_2()
            .gap_1()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(place)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(comment.author.name.clone()),
                    )
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{}{}",
                        when(comment.created),
                        if comment.edited.is_some() {
                            ", edited"
                        } else {
                            ""
                        }
                    ))),
            )
            .child(div().child(comment.text.clone()));
        if let Some(resolution) = &comment.resolved {
            let how = match suggestion.map(|suggestion| suggestion.state) {
                Some(SuggestionState::Accepted) => "Accepted",
                Some(SuggestionState::Rejected) => "Rejected",
                _ => "Resolved",
            };
            card = card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{how} by {}", resolution.by)),
            );
        }
        for reply in &comment.replies {
            let reply_id = reply.id;
            let mut row = v_flex()
                .pl_3()
                .border_l_2()
                .border_color(cx.theme().border)
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(reply.author.name.clone()),
                        )
                        .child(div().text_xs().text_color(muted).child(when(reply.created))),
                )
                .child(div().child(reply.text.clone()));
            let mut actions = h_flex().gap_1();
            if reply.can.edit {
                let text = reply.text.clone();
                actions =
                    actions.child(button(&format!("edit-reply-{reply_id}"), "Edit").on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_box(
                                |input| Draft::Edit {
                                    document,
                                    comment: id,
                                    reply: Some(reply_id),
                                    input,
                                },
                                &text,
                                window,
                                cx,
                            );
                        }),
                    ));
            }
            if reply.can.delete {
                actions = actions.child(
                    button(&format!("delete-reply-{reply_id}"), "Delete").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.invoke(
                                ids::COMMENTS_DELETE,
                                CommandArgs::Comment(CommentRequest::Delete {
                                    id: document,
                                    comment: id,
                                    reply: Some(reply_id),
                                }),
                                window,
                                cx,
                            );
                        },
                    )),
                );
            }
            row = row.child(actions);
            card = card.child(row);
        }
        card = card.child(buttons);
        let open_here = match &self.comments.draft {
            Some(
                Draft::Reply { comment, .. }
                | Draft::Edit { comment, .. }
                | Draft::EditSuggestion { comment, .. },
            ) => *comment == id,
            _ => false,
        };
        if open_here {
            if let Some(draft) = &self.comments.draft {
                card = card.child(self.render_draft(draft, cx));
            }
        }
        // The card is read aloud by its label (`comment_card`); tests find it by the same.
        div()
            .debug_selector(move || label.clone())
            .child(card)
            .into_any_element()
    }

    /// A suggestion's change: its original with the part accepting would remove struck through,
    /// then its replacement with the part it would add underlined; and, while outdated, why it
    /// cannot be accepted. Choosing it selects its text, as a comment's quote does.
    #[allow(clippy::too_many_arguments)]
    fn render_change(
        &self,
        key: &str,
        suggestion: &adoc_engine::Suggestion,
        removed: std::ops::Range<usize>,
        inserted: std::ops::Range<usize>,
        attached: bool,
        range: SourceRange,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let struck = HighlightStyle {
            strikethrough: Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..HighlightStyle::default()
        };
        let underlined = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            ..HighlightStyle::default()
        };
        v_flex()
            .id(SharedString::from(format!("comment-quote-{key}")))
            .gap_0p5()
            .when(attached, |change| {
                change
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_comment(range, window, cx);
                    }))
            })
            .child(
                div().text_color(cx.theme().muted_foreground).child(
                    StyledText::new(suggestion.original.clone())
                        .with_highlights((!removed.is_empty()).then_some((removed, struck))),
                ),
            )
            .when(!suggestion.replacement.is_empty(), |change| {
                change.child(
                    div().child(
                        StyledText::new(suggestion.replacement.clone()).with_highlights(
                            (!inserted.is_empty()).then_some((inserted, underlined)),
                        ),
                    ),
                )
            })
            .when(suggestion.state == SuggestionState::Outdated, |change| {
                change.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child("The text has changed since this was suggested."),
                )
            })
            .into_any_element()
    }

    /// The Comments panel: the active page's comments, a box for a new one, and why a page's
    /// comments cannot be used when they cannot.
    pub(super) fn render_comments(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut panel = v_flex()
            .id("comments-panel")
            .debug_selector(|| "comments-panel".to_owned())
            .size_full()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .text_sm();
        let Some(document) = self.active_document(cx) else {
            return panel
                .child(div().text_color(muted).child("No page is open."))
                .into_any_element();
        };
        // A box for another page's comment is put away when that page is no longer shown.
        if self
            .comments
            .draft
            .as_ref()
            .is_some_and(|draft| draft.document() != document)
        {
            self.comments.draft = None;
        }
        let engine = &self.app.read(cx).engine;
        match engine.comments_state(document) {
            CommentsState::Usable => {}
            CommentsState::Unusable(reason) | CommentsState::ReadOnly(reason) => {
                panel = panel.child(
                    div()
                        .debug_selector(|| "comments-state".to_owned())
                        .text_color(cx.theme().warning)
                        .child(reason),
                );
            }
        }
        let comments = engine.comments(document);
        if let Some(draft @ (Draft::New { .. } | Draft::Suggest { .. })) = &self.comments.draft {
            let heading = if draft.replacement().is_some() {
                "Suggested change"
            } else {
                "New comment"
            };
            panel = panel
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(heading))
                .child(self.render_draft(draft, cx));
        }
        let (open, rest): (Vec<&Comment>, Vec<&Comment>) = comments
            .iter()
            .partition(|comment| !comment.detached && comment.resolved.is_none());
        let (detached, resolved): (Vec<&Comment>, Vec<&Comment>) =
            rest.into_iter().partition(|comment| comment.detached);
        if comments.is_empty() && self.comments.draft.is_none() {
            panel = panel.child(
                div()
                    .text_color(muted)
                    .child("No comments on this page. Select some text and choose Add Comment."),
            );
        }
        for comment in open {
            panel = panel.child(self.render_comment(document, comment, cx));
        }
        if !resolved.is_empty() {
            let show = self.comments.show_resolved;
            panel = panel.child(
                Button::new("comments-resolved")
                    .label(format!(
                        "{} Resolved ({})",
                        if show { "▾" } else { "▸" },
                        resolved.len()
                    ))
                    .small()
                    .ghost()
                    .debug_selector(|| "comments-resolved".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.comments.show_resolved = !this.comments.show_resolved;
                        cx.notify();
                    })),
            );
            if show {
                for comment in resolved {
                    panel = panel.child(self.render_comment(document, comment, cx));
                }
            }
        }
        if !detached.is_empty() {
            panel = panel.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Detached ({})", detached.len())),
            );
            for comment in detached {
                panel = panel.child(self.render_comment(document, comment, cx));
            }
        }
        panel.into_any_element()
    }

    /// Whether the sidebar offers the Comments tab: while it is chosen, a box is open, or the
    /// active page has comments.
    pub(super) fn offers_comments(&self, cx: &App) -> bool {
        self.sidebar_tab == SidebarTab::Comments
            || self.comments.draft.is_some()
            || self
                .active_document(cx)
                .is_some_and(|document| !self.app.read(cx).engine.comments(document).is_empty())
    }

    /// What the open suggestion box's replacement holds.
    #[cfg(test)]
    pub fn suggestion_replacement_for_test(&self, cx: &App) -> Option<String> {
        let replacement = self.comments.draft.as_ref()?.replacement()?;
        Some(replacement.read(cx).value().to_string())
    }

    /// The status bar's word on the comment or suggestion at the caret.
    #[cfg(test)]
    pub fn status_comment_for_test(&self, cx: &App) -> Option<String> {
        self.comment_at_caret(cx).map(|(_, word)| word)
    }

    /// The status bar's word on a comment or suggestion at the caret, if there is one, with
    /// its selector: `status-suggestion` saying the change as its card reads it aloud, since
    /// the source editor does not draw the text it would insert; else `status-comment`.
    pub(super) fn comment_at_caret(&self, cx: &App) -> Option<(&'static str, String)> {
        let editor = self.active_editor()?.read(cx);
        let document = editor.document();
        let engine = &self.app.read(cx).engine;
        let id = engine.comment_at(document, editor.cursor(cx))?;
        let comment = engine
            .comments(document)
            .into_iter()
            .find(|comment| comment.id == id)?;
        let Some(suggestion) = &comment.suggestion else {
            return Some((
                "status-comment",
                format!("Comment by {}", comment.author.name),
            ));
        };
        // The part of the original accepting would remove, as the card works it out.
        let prefix = suggestion.removed.start.saturating_sub(comment.range.start);
        let suffix = comment.range.end.saturating_sub(suggestion.removed.end);
        let removed = middle(&suggestion.original, prefix, suffix);
        Some((
            "status-suggestion",
            suggestion_label(
                &comment.author.name,
                &suggestion.original[removed],
                &suggestion.inserted,
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use gpui::Element as _;

    use super::*;

    /// A card is a group that assistive technology reads by its label: who wrote it, and the
    /// change it suggests in words, since its strike and underline are only seen.
    #[test]
    fn a_card_is_read_aloud_by_its_label() {
        let label = suggestion_label("Ada", "word", "term");
        assert_eq!(
            label,
            "Suggestion by Ada: replace \u{2018}word\u{2019} with \u{2018}term\u{2019}"
        );
        let card = comment_card("7", label.clone());
        assert_eq!(card.a11y_role(), Some(gpui::Role::Group));
        let mut node = gpui::accesskit::Node::new(gpui::Role::Group);
        card.write_a11y_info(&mut node);
        assert_eq!(node.label(), Some(label.as_str()));
        assert_eq!(
            suggestion_label("Ada", "word", ""),
            "Suggestion by Ada: delete \u{2018}word\u{2019}"
        );
        assert_eq!(
            suggestion_label("Ada", "", "term"),
            "Suggestion by Ada: insert \u{2018}term\u{2019}"
        );
    }
}
