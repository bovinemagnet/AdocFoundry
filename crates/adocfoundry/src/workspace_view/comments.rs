//! The Comments panel (#762, COMMENTS 3/4): a sidebar tab listing the active page's comments, as
//! the engine holds them (alone, in the page's `.afd` file; in a session, as the host told
//! them). It works nothing out: every change is a `comments.*` command, and what the user may
//! do to each comment is the engine's `Allowed`.
//!
//! Open comments come first, in the order of their text; resolved ones are folded under a
//! heading, and detached ones (their text gone) are listed with the text they were on. Choosing
//! a comment selects its text in the editor. Add Comment opens a box for a new comment on the
//! selection; Reply and Edit open one under the comment.

use adoc_engine::{Comment, CommentId, CommentRequest, CommentsState};
use gpui::AnyElement;

use super::*;
use crate::editor::bindings::{AddComment, NextComment, PreviousComment};

/// What the panel is writing: a new comment on a range, a reply, or an edit, with its box.
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
}

impl Draft {
    fn input(&self) -> &Entity<gpui_component::input::TextareaState> {
        match self {
            Self::New { input, .. } | Self::Reply { input, .. } | Self::Edit { input, .. } => input,
        }
    }

    fn document(&self) -> DocumentId {
        match self {
            Self::New { document, .. }
            | Self::Reply { document, .. }
            | Self::Edit { document, .. } => *document,
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

impl WorkspaceView {
    /// Add Comment: a box in the Comments panel for a comment on the active editor's selection.
    pub(super) fn on_add_comment(
        &mut self,
        _: &AddComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        let (document, selection) = {
            let editor = editor.read(cx);
            (editor.document(), editor.selection(cx))
        };
        if selection.is_empty() {
            window.push_notification("Select the text to comment on first.", cx);
            return;
        }
        if let CommentsState::Unusable(reason) | CommentsState::ReadOnly(reason) =
            self.app.read(cx).engine.comments_state(document)
        {
            window.push_notification(reason, cx);
            return;
        }
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
        if text.trim().is_empty() {
            self.comments.draft = Some(draft);
            return;
        }
        let (command, request) = match draft {
            Draft::New {
                document, range, ..
            } => (
                ids::COMMENTS_ADD,
                CommentRequest::Add {
                    id: document,
                    range,
                    text,
                },
            ),
            Draft::Reply {
                document, comment, ..
            } => (
                ids::COMMENTS_REPLY,
                CommentRequest::Reply {
                    id: document,
                    comment,
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
                    id: document,
                    comment,
                    reply,
                    text,
                },
            ),
        };
        self.invoke(command, CommandArgs::Comment(request), window, cx);
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

    /// The box being written in, with Comment (or Save) and Cancel.
    fn render_draft(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let label = match draft {
            Draft::New { .. } => "Comment",
            Draft::Reply { .. } => "Reply",
            Draft::Edit { .. } => "Save",
        };
        v_flex()
            .gap_1()
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
        let label = format!(
            "Comment by {} on \u{201c}{}\u{201d}",
            comment.author.name,
            shown_quote(&comment.quote)
        );
        let button = |name: &str, title: &str| {
            let selector = format!("comment-{name}-{key}");
            Button::new(SharedString::from(selector.clone()))
                .label(title.to_owned())
                .small()
                .ghost()
                .debug_selector(move || selector.clone())
        };
        let mut buttons = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
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
        if comment.can.resolve {
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
        if comment.can.edit {
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
        let mut card = v_flex()
            .id(SharedString::from(format!("comment-{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("comment-{key}")
            })
            .p_2()
            .gap_1()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
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
                    }),
            )
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
            card = card.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Resolved by {}", resolution.by)),
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
            Some(Draft::Reply { comment, .. } | Draft::Edit { comment, .. }) => *comment == id,
            _ => false,
        };
        if open_here {
            if let Some(draft) = &self.comments.draft {
                card = card.child(self.render_draft(draft, cx));
            }
        }
        // Read aloud as who wrote it and on what, so the colour in the editor is never all
        // that says where a comment is.
        div()
            .debug_selector(move || label.clone())
            .child(card)
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
        if let Some(draft @ Draft::New { .. }) = &self.comments.draft {
            panel = panel
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("New comment"),
                )
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

    /// The status bar's word on a comment at the caret, if there is one.
    pub(super) fn comment_at_caret(&self, cx: &App) -> Option<String> {
        let editor = self.active_editor()?.read(cx);
        let document = editor.document();
        let engine = &self.app.read(cx).engine;
        let id = engine.comment_at(document, editor.cursor(cx))?;
        let comment = engine
            .comments(document)
            .into_iter()
            .find(|comment| comment.id == id)?;
        Some(format!("Comment by {}", comment.author.name))
    }
}
