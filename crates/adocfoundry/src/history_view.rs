//! File › Recover Unsaved Changes… and File › Local History…: the unsaved text that sessions
//! which ended left behind, and the active page's saved versions. Each list sits beside the
//! lines the chosen entry changes, with the buttons that take it back.

use std::time::{SystemTime, UNIX_EPOCH};

use adoc_engine::{commands::ids, CommandArgs, DiffLine, DocumentId, Recoverable, SavedVersion};
use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Stateful,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, WindowExt as _,
};

use crate::{app::AppState, editor::blame::relative_time, sidebar, workspace_view::WorkspaceView};

/// Offers `items`, unsaved text left by sessions that ended, to recover or discard.
pub fn show_recovery(
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    items: Vec<Recoverable>,
    window: &mut Window,
    cx: &mut App,
) {
    let list = cx.new(|_| RecoveryList {
        app,
        workspace,
        items,
        selected: 0,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Recover Unsaved Changes")
            .w(px(880.))
            .child(list.clone())
    });
}

/// Offers `versions` of `document`'s page, newest first, to compare and restore.
pub fn show_history(
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    document: DocumentId,
    versions: Vec<SavedVersion>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = app
        .read(cx)
        .engine
        .document(document)
        .and_then(|document| document.path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let list = cx.new(|_| HistoryList {
        app,
        workspace,
        document,
        versions,
        selected: 0,
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(format!("Local History of {name}"))
            .w(px(880.))
            .child(list.clone())
    });
}

struct RecoveryList {
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    items: Vec<Recoverable>,
    selected: usize,
}

impl RecoveryList {
    fn name(&self, item: &Recoverable, cx: &App) -> String {
        if item.untitled {
            format!("{} (untitled)", item.path.display())
        } else {
            sidebar::display_path(&item.path, self.app.read(cx).engine.roots())
        }
    }

    /// Recovers or discards the chosen entry, and closes the dialog once none is left.
    fn take(&mut self, recover: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        let command = if recover {
            ids::DOCUMENT_RECOVER
        } else {
            ids::DOCUMENT_DISCARD_RECOVERY
        };
        let name = self.name(&item, cx);
        let done = self.workspace.upgrade().is_some_and(|workspace| {
            workspace.update(cx, |workspace, cx| {
                workspace
                    .invoke(command, CommandArgs::Path(item.record.clone()), window, cx)
                    .is_some()
            })
        });
        if !done {
            return;
        }
        if recover {
            window.push_notification(
                format!("Recovered {name}: save it to keep the changes."),
                cx,
            );
        }
        self.items.remove(self.selected);
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        if self.items.is_empty() {
            window.close_dialog(cx);
        }
        cx.notify();
    }
}

impl Render for RecoveryList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = seconds(SystemTime::now());
        let rows: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let detail = format!("Edited {}", relative_time(seconds(item.written), now));
                row(
                    "recovery-item",
                    index,
                    self.name(item, cx),
                    detail,
                    index == self.selected,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = index;
                    cx.notify();
                }))
            })
            .collect();
        let diff = self
            .items
            .get(self.selected)
            .map(|item| self.app.read(cx).engine.recovery_diff(item))
            .unwrap_or_default();
        v_flex()
            .gap_3()
            .child(
                "AdocFoundry closed before these changes were saved. Recover a page to open it \
                 with the changes, unsaved, or discard them.",
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(v_flex().w(px(260.)).gap_1().children(rows))
                    .child(diff_view(
                        "recovery-diff",
                        "Changes to the file on disk",
                        &diff,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("recovery-discard")
                            .label("Discard")
                            .debug_selector(|| "recovery-discard".to_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(false, window, cx);
                            })),
                    )
                    .child(
                        Button::new("recovery-recover")
                            .primary()
                            .label("Recover")
                            .debug_selector(|| "recovery-recover".to_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.take(true, window, cx);
                            })),
                    ),
            )
    }
}

struct HistoryList {
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    document: DocumentId,
    versions: Vec<SavedVersion>,
    selected: usize,
}

impl HistoryList {
    /// Puts the chosen version in the page, as one undo step, and closes the dialog.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(version) = self.versions.get(self.selected).cloned() else {
            return;
        };
        let args = CommandArgs::Custom(serde_json::json!({
            "document": self.document.get(),
            "version": version.id,
        }));
        let restored = self.workspace.upgrade().is_some_and(|workspace| {
            workspace.update(cx, |workspace, cx| {
                workspace
                    .invoke(ids::DOCUMENT_RESTORE_VERSION, args, window, cx)
                    .is_some()
            })
        });
        if restored {
            window.close_dialog(cx);
            let when = relative_time(seconds(version.saved), seconds(SystemTime::now()));
            window.push_notification(
                format!("Restored the version saved {when}: save the page to keep it."),
                cx,
            );
        }
    }
}

impl Render for HistoryList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = seconds(SystemTime::now());
        let rows: Vec<_> = self
            .versions
            .iter()
            .enumerate()
            .map(|(index, version)| {
                let title = format!("Saved {}", relative_time(seconds(version.saved), now));
                row(
                    "history-item",
                    index,
                    title,
                    size(version.size),
                    index == self.selected,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = index;
                    cx.notify();
                }))
            })
            .collect();
        let diff = self.versions.get(self.selected).map(|version| {
            self.app
                .read(cx)
                .engine
                .version_diff(self.document, &version.id)
        });
        let diff = match diff {
            Some(Ok(diff)) => diff,
            Some(Err(error)) => vec![DiffLine::Same(error.to_string())],
            None => Vec::new(),
        };
        v_flex()
            .gap_3()
            .child(
                "The versions of this page as it was saved. Restoring one puts it in the page \
                 as one undo step, unsaved.",
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        v_flex()
                            .id("history-items")
                            .w(px(220.))
                            .max_h(px(380.))
                            .overflow_y_scroll()
                            .gap_1()
                            .children(rows),
                    )
                    .child(diff_view(
                        "history-diff",
                        "Changes made since this version",
                        &diff,
                        cx,
                    )),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("history-restore")
                        .primary()
                        .label("Restore")
                        .debug_selector(|| "history-restore".to_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))),
                ),
            )
    }
}

/// A selectable entry of a list: a title over a detail.
fn row(
    prefix: &'static str,
    index: usize,
    title: String,
    detail: String,
    selected: bool,
    cx: &App,
) -> Stateful<gpui::Div> {
    let theme = cx.theme();
    div()
        .id((prefix, index))
        .debug_selector(move || format!("{prefix}-{index}"))
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .cursor_pointer()
        .when(selected, |row| row.bg(theme.accent))
        .child(div().text_sm().child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(detail),
        )
}

/// The lines of a difference under `caption`, removed lines red and added lines green, with
/// the unchanged lines between changes counted.
fn diff_view(
    id: &'static str,
    caption: &'static str,
    lines: &[DiffLine],
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let line = |text: String| {
        // Blank lines keep their height.
        div().child(if text.is_empty() {
            " ".to_owned()
        } else {
            text
        })
    };
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(caption),
        )
        .child(
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .h(px(360.))
                .overflow_y_scroll()
                .p_2()
                .bg(theme.muted)
                .rounded(theme.radius)
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .when(lines.is_empty(), |view| view.child("No differences."))
                .children(lines.iter().map(|diff| match diff {
                    DiffLine::Same(text) => line(format!("  {text}")),
                    DiffLine::Removed(text) => {
                        line(format!("- {text}")).bg(theme.danger.opacity(0.18))
                    }
                    DiffLine::Added(text) => {
                        line(format!("+ {text}")).bg(theme.success.opacity(0.18))
                    }
                    DiffLine::Skipped(count) => {
                        let plural = if *count == 1 { "" } else { "s" };
                        line(format!("⋯ {count} unchanged line{plural}"))
                            .text_color(theme.muted_foreground)
                    }
                })),
        )
}

/// `2.1 KB`, or `640 bytes`.
fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else {
        #[allow(clippy::cast_precision_loss)]
        let kilobytes = bytes as f64 / 1024.0;
        format!("{kilobytes:.1} KB")
    }
}

fn seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
    })
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn sizes_are_given_in_bytes_or_kilobytes() {
        assert_eq!(size(640), "640 bytes");
        assert_eq!(size(2150), "2.1 KB");
    }
}
