//! Delete File (#100): the confirmation lists what removing the file would break, from Inspect
//! Impact's analysis (`impact.inspect`, which the Impact panel then holds too), before the
//! engine's `file.delete` moves it aside; the notification after offers Undo, which is
//! `file.undo_delete`, as Edit › Undo Delete is. File › Restore Deleted File… (#434) lists the
//! deletions the engine keeps for the workspace's files, from this session or earlier ones,
//! and puts back the one chosen through `file.restore_deleted`.

use std::{rc::Rc, time::SystemTime};

use adoc_engine::{
    impact::{Impact, ImpactKind},
    ChangeSource, InspectionState, KeptDeletion,
};
use gpui_component::{button::ButtonVariant, command::CommandItem};

use super::*;
use crate::{
    editor::{
        bindings::{DeleteFile, RestoreDeletedFile, UndoDelete},
        blame::relative_time,
    },
    history_view::seconds,
};

/// A kept deletion's row in Restore Deleted File…: where it was and when it went, and whether
/// a file is at its path now, which restoring refuses to replace.
fn kept_label(kept: &KeptDeletion, roots: &[PathBuf], now: i64) -> String {
    let path = sidebar::display_path(&kept.path, roots);
    let when = relative_time(seconds(kept.deleted), now);
    if kept.occupied {
        format!("{path} — deleted {when}; a file is there now")
    } else {
        format!("{path} — deleted {when}")
    }
}

/// How many of the things it would break the confirmation lists.
const LISTED: usize = 8;

/// What deleting a file would break, as one line of the confirmation; `None` for the file's
/// own rendering.
fn breakage(impact: &Impact) -> Option<String> {
    Some(match impact.kind {
        ImpactKind::Render if impact.via.len() < 2 => return None,
        ImpactKind::Render => format!("Included in {}", impact.path),
        ImpactKind::ReferenceIntegrity => {
            let from = impact
                .source
                .as_ref()
                .map_or(impact.path.as_str(), |source| source.path.as_str());
            match &impact.anchor {
                Some(anchor) => format!("Linked to #{anchor} from {from}"),
                None => format!("Linked from {from}"),
            }
        }
        ImpactKind::Navigation => {
            let from = impact
                .source
                .as_ref()
                .map_or(impact.path.as_str(), |source| source.path.as_str());
            format!("Listed in {from}")
        }
        ImpactKind::Publication => format!(
            "Published by {}",
            impact.publication.as_deref().unwrap_or(&impact.path)
        ),
    })
}

/// What the confirmation says about the analysis of `path`.
fn consequences(engine: &adoc_engine::Engine, path: &Path) -> Vec<String> {
    let inspection = engine
        .impact_inspection()
        .filter(|inspection| inspection.source == ChangeSource::Removal(vec![path.to_owned()]));
    let Some(inspection) = inspection else {
        return vec!["Working out what depends on it…".to_owned()];
    };
    match &inspection.state {
        InspectionState::Running => vec!["Working out what depends on it…".to_owned()],
        InspectionState::Failed(error) => {
            vec![format!(
                "What depends on it could not be worked out: {error}"
            )]
        }
        InspectionState::Ready(report) => {
            let mut lines: Vec<String> = report.impacts.iter().filter_map(breakage).collect();
            lines.dedup();
            if lines.is_empty() {
                return vec!["Nothing in the workspace depends on it.".to_owned()];
            }
            let more = lines.len().saturating_sub(LISTED);
            lines.truncate(LISTED);
            if more > 0 {
                lines.push(format!("…and {more} more, listed in the Impact panel."));
            }
            lines
        }
    }
}

impl WorkspaceView {
    /// What the confirmation for deleting `path` lists just now.
    #[cfg(test)]
    pub(crate) fn delete_consequences_for_test(&self, path: &Path, cx: &App) -> Vec<String> {
        consequences(&self.app.read(cx).engine, path)
    }

    /// File › Delete File…: the active tab's file.
    pub(super) fn on_delete_file(
        &mut self,
        _: &DeleteFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        let path = match engine.active_item() {
            Some(TabItem::Document(id)) => engine
                .document(id)
                .filter(|document| !document.untitled)
                .map(|document| document.path.clone()),
            Some(TabItem::Image(id)) => engine.image(id).map(|image| image.path.clone()),
            None => None,
        };
        if let Some(path) = path {
            self.confirm_delete(&path, window, cx);
        }
    }

    pub(super) fn on_undo_delete(
        &mut self,
        _: &UndoDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke(ids::FILE_UNDO_DELETE, CommandArgs::None, window, cx);
    }

    /// File › Restore Deleted File…: the deletions kept for the workspace's files, newest
    /// first; the one chosen is put back, and opened if it is a page.
    pub(super) fn on_restore_deleted_file(
        &mut self,
        _: &RestoreDeletedFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = &self.app.read(cx).engine;
        if !engine.command_available(ids::FILE_RESTORE_DELETED) {
            return;
        }
        let kept = engine.kept_deletions();
        if kept.is_empty() {
            window.push_notification("No deleted files of this workspace are kept.", cx);
            return;
        }
        let roots = engine.roots().to_vec();
        let now = seconds(SystemTime::now());
        let items: Vec<CommandItem> = kept
            .iter()
            .map(|kept| {
                let path = sidebar::display_path(&kept.path, &roots);
                CommandItem::new()
                    .label(kept_label(kept, &roots, now))
                    .keywords([SharedString::from(path)])
            })
            .collect();
        let kept = Rc::new(kept);
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus = state.read(cx).focus_handle(cx);
        let workspace = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            window.focus(&focus, cx);
            dialog.close_button(false).p_0().child(
                Command::new(&state)
                    .bordered(false)
                    .placeholder("Restore a deleted file")
                    .max_h(px(420.))
                    .items(items.clone())
                    .on_confirm({
                        let (kept, workspace) = (kept.clone(), workspace.clone());
                        move |index, window, cx| {
                            let Some(kept) = kept.get(index.row) else {
                                return;
                            };
                            let args = CommandArgs::Custom(
                                serde_json::json!({ "deletion": kept.id.clone() }),
                            );
                            window.close_dialog(cx);
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.invoke(ids::FILE_RESTORE_DELETED, args, window, cx);
                            });
                        }
                    }),
            )
        });
    }

    /// Asks before deleting the file at `path`, listing what removing it would break, worked
    /// out while the question is shown.
    pub(crate) fn confirm_delete(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asciidoc = adoc_engine::is_asciidoc_path(path);
        if asciidoc {
            let _ = self.app.update(cx, |app, cx| {
                app.invoke(
                    ids::IMPACT_INSPECT,
                    CommandArgs::Path(path.to_path_buf()),
                    cx,
                )
            });
        }
        let unsaved = {
            let engine = &self.app.read(cx).engine;
            engine
                .document_for_path(path)
                .and_then(|id| engine.document(id))
                .is_some_and(|document| document.dirty)
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let (app, workspace, path) = (self.app.clone(), cx.weak_entity(), path.to_path_buf());
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let lines: Vec<String> = if asciidoc {
                consequences(&app.read(cx).engine, &path)
            } else {
                vec![
                    "What uses a file that is not AsciiDoc, such as an image, is not traced."
                        .to_owned(),
                ]
            };
            let (workspace, deleted) = (workspace.clone(), path.clone());
            dialog
                .title(format!("Delete {name}?"))
                .child(
                    v_flex()
                        .gap_2()
                        .debug_selector(|| "delete-confirm".to_owned())
                        .child(
                            "It is moved aside, and Edit › Undo Delete puts it back. Removing it \
                             would affect:",
                        )
                        .child(
                            v_flex()
                                .p_2()
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .text_sm()
                                .debug_selector(|| "delete-consequences".to_owned())
                                .children(lines.into_iter().map(|line| div().child(line))),
                        )
                        .when(unsaved, |content| {
                            content.child("Its unsaved edits are discarded.")
                        }),
                )
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let deleted = deleted.clone();
                    let _ = workspace.update(cx, |this, cx| this.delete_now(&deleted, window, cx));
                    true
                })
        });
    }

    /// Deletes the file at `path`, offering Undo after.
    fn delete_now(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .invoke(
                ids::FILE_DELETE,
                CommandArgs::Path(path.to_path_buf()),
                window,
                cx,
            )
            .is_none()
        {
            return;
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let workspace = cx.weak_entity();
        window.push_notification(
            Notification::new()
                .message(format!("Deleted {name}."))
                .action(move |_, _, cx| {
                    let (workspace, notification) = (workspace.clone(), cx.entity());
                    Button::new("undo-delete")
                        .small()
                        .label("Undo")
                        .debug_selector(|| "undo-delete".to_owned())
                        .on_click(move |_, window, cx| {
                            let _ = workspace.update(cx, |this, cx| {
                                this.invoke(ids::FILE_UNDO_DELETE, CommandArgs::None, window, cx);
                            });
                            notification.update(cx, |notification, cx| {
                                notification.dismiss(window, cx);
                            });
                        })
                })
                .autohide(true),
            cx,
        );
    }
}
