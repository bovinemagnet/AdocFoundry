//! The Files panel: the folders and files beneath each workspace root as they are on disk,
//! listed by the engine a folder at a time as the user opens them.
//!
//! Rows are keyed by root and path, never by position, and the panel keeps which folders are
//! open. A file opens through the workspace: AsciiDoc and other text as a (preview) tab, images,
//! PDFs, fonts and archives in the system's own application, and nothing is run by selecting it.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
};

use adoc_engine::{commands::ids, CommandArgs, DirectoryError, EngineEvent, EntryKind, Listing};
use gpui::{
    actions, div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
};
use gpui_component::{
    h_flex,
    list::ListItem,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    tooltip::Tooltip,
    tree::{tree, TreeEvent, TreeItem, TreeState},
    v_flex, ActiveTheme as _, IconName, WindowExt as _,
};

use crate::{app::AppState, workspace_view::WorkspaceView};

actions!(adocfoundry, [OpenFileEntry]);

/// The key context of the Files panel, where Space opens the selected file.
pub const FILES_CONTEXT: &str = "FilesTree";

/// Files a click opens in the system's own viewer rather than as text: images and PDFs, which
/// viewers only display. Anything else that is not text (fonts, archives, programs) opens only
/// through Open Externally, so selecting a file can never run it.
const EXTERNAL: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "tif", "tiff", "heic", "pdf",
];

/// Whether `path` opens in the system's own application rather than as text.
pub fn opens_externally(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .is_some_and(|extension| EXTERNAL.contains(&extension.as_str()))
}

#[cfg(test)]
thread_local! {
    /// What the Files view handed to the system to open, in tests, whose platform opens nothing.
    pub static OPENED_EXTERNALLY: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens `path` in the system's own application for it.
fn open_externally(path: &Path, cx: &App) {
    #[cfg(test)]
    {
        let _ = cx;
        OPENED_EXTERNALLY.with(|opened| opened.borrow_mut().push(path.to_path_buf()));
    }
    #[cfg(not(test))]
    cx.open_with_system(path);
}

/// What a row stands for.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RowKind {
    Root,
    Directory,
    File,
    Link {
        to_directory: bool,
    },
    /// Loading, empty, or a reason the folder above could not be listed.
    Note,
}

#[derive(Clone, Debug)]
struct Row {
    root: PathBuf,
    path: PathBuf,
    kind: RowKind,
    excluded: bool,
}

/// The platform's name for showing a file in its file manager.
fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Reveal in File Explorer"
    } else {
        "Reveal in File Manager"
    }
}

/// A row's id: its root and path, which name it however the tree changes.
fn row_id(root: &Path, path: &Path) -> SharedString {
    format!("{}\u{1f}{}", root.display(), path.display()).into()
}

pub struct FilesView {
    app: Entity<AppState>,
    workspace: WeakEntity<WorkspaceView>,
    tree: Entity<TreeState>,
    /// Shared with the rows' renderers and menus, so a frame does not copy it.
    rows: Rc<HashMap<SharedString, Row>>,
    /// The open folders, roots included.
    expanded: BTreeSet<PathBuf>,
    /// Shown though excluded, for a file revealed beneath an excluded folder.
    forced: BTreeSet<PathBuf>,
    /// A file to select once the folders above it are listed.
    pending_reveal: Option<PathBuf>,
    /// The roots the tree was built for, to notice a new workspace.
    roots: Vec<PathBuf>,
    /// The next rebuild selects the active document's row, if it is shown: another tab became
    /// active.
    follow_active: bool,
    _subscriptions: Vec<Subscription>,
}

impl FilesView {
    pub fn new(
        app: Entity<AppState>,
        workspace: WeakEntity<WorkspaceView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.subscribe(&app, |this, _, event: &EngineEvent, cx| match event {
                EngineEvent::DirectoryListed(_) | EngineEvent::WorkspaceIndexed { .. } => {
                    this.rebuild(cx);
                }
                EngineEvent::NavigationChanged => {
                    this.follow_active = true;
                    this.rebuild(cx);
                }
                _ => {}
            }),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, cx| {
                let (id, open) = match event {
                    TreeEvent::Expanded(id) => (id, true),
                    TreeEvent::Collapsed(id) => (id, false),
                };
                let Some(row) = this.rows.get(id).cloned() else {
                    return;
                };
                if open {
                    this.expanded.insert(row.path.clone());
                    this.request(&row.path, cx);
                } else {
                    this.expanded.remove(&row.path);
                }
                this.rebuild(cx);
            }),
        ];
        let mut view = Self {
            app,
            workspace,
            tree,
            rows: Rc::default(),
            expanded: BTreeSet::new(),
            forced: BTreeSet::new(),
            pending_reveal: None,
            roots: Vec::new(),
            follow_active: false,
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view
    }

    fn request(&self, dir: &Path, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let _ = app.engine.request_directory(dir);
            app.drain(cx);
        });
    }

    /// Rebuilds the rows from the engine's listings, keeping the selection on the same path,
    /// or the nearest folder above it still shown.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let roots = self.app.read(cx).engine.roots().to_vec();
        if roots != self.roots {
            // A new workspace: its roots open, everything else starts closed.
            self.roots = roots.clone();
            self.expanded = roots.iter().cloned().collect();
            self.forced.clear();
            for root in &roots {
                self.request(root, cx);
            }
        }
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .and_then(|item| self.rows.get(&item.id))
            .map(|row| row.path.clone());
        let active = {
            let engine = &self.app.read(cx).engine;
            engine
                .active()
                .and_then(|id| engine.document(id))
                .map(|document| document.path.clone())
        };
        let mut rows = HashMap::new();
        let mut wanted = Vec::new();
        let items: Vec<TreeItem> = roots
            .iter()
            .map(|root| {
                let label = root_label(root, &roots);
                let id = row_id(root, root);
                rows.insert(
                    id.clone(),
                    Row {
                        root: root.clone(),
                        path: root.clone(),
                        kind: RowKind::Root,
                        excluded: false,
                    },
                );
                let children = self.children(root, root, &mut rows, &mut wanted, cx);
                TreeItem::new(id, label)
                    .children(children)
                    .expanded(self.expanded.contains(root))
            })
            .collect();
        self.rows = Rc::new(rows);
        for dir in wanted {
            self.request(&dir, cx);
        }
        let reveal = self.pending_reveal.clone();
        let rows = &self.rows;
        let exact = |path: &Path| {
            rows.iter()
                .find(|(_, row)| row.path == path && row.kind != RowKind::Note)
                .map(|(id, _)| id.clone())
        };
        // The row itself, or the nearest folder above it still shown.
        let shown = |path: &Path| path.ancestors().find_map(exact);
        let follow = std::mem::take(&mut self.follow_active);
        let active_row = active.as_deref().and_then(exact);
        let target_id = match (&reveal, follow, &active_row) {
            (Some(reveal), _, _) => shown(reveal),
            (None, true, Some(active)) => Some(active.clone()),
            _ => selected.as_deref().and_then(shown).or(active_row),
        };
        let revealed = reveal
            .as_deref()
            .zip(target_id.as_ref())
            .is_some_and(|(path, id)| self.rows.get(id).is_some_and(|row| row.path == path));
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let index = target_id.as_ref().and_then(|id| tree.index_of(id));
            tree.set_selected_index(index, cx);
            if revealed {
                if let Some(index) = index {
                    tree.scroll_to_item(index, gpui::ScrollStrategy::Center);
                }
            }
        });
        if revealed {
            self.pending_reveal = None;
        }
        cx.notify();
    }

    /// The rows beneath `dir`: its entries once listed, else a note saying why there are none.
    fn children(
        &self,
        root: &Path,
        dir: &Path,
        rows: &mut HashMap<SharedString, Row>,
        wanted: &mut Vec<PathBuf>,
        cx: &App,
    ) -> Vec<TreeItem> {
        let engine = &self.app.read(cx).engine;
        let show_excluded = engine.show_excluded();
        let note = |text: String, rows: &mut HashMap<SharedString, Row>| {
            let id: SharedString = format!("{}\u{1f}note", row_id(root, dir)).into();
            rows.insert(
                id.clone(),
                Row {
                    root: root.to_path_buf(),
                    path: dir.to_path_buf(),
                    kind: RowKind::Note,
                    excluded: false,
                },
            );
            vec![TreeItem::new(id, text).disabled(true)]
        };
        let entries = match engine.directory_with_excluded(dir) {
            None => {
                if self.expanded.contains(dir) {
                    wanted.push(dir.to_path_buf());
                }
                return note("Loading…".to_owned(), rows);
            }
            Some(Listing::Loading) => return note("Loading…".to_owned(), rows),
            Some(Listing::Failed(DirectoryError::PermissionDenied)) => {
                return note("Permission denied — right-click to retry".to_owned(), rows)
            }
            Some(Listing::Failed(DirectoryError::Unavailable(reason))) => {
                return note(format!("Unavailable: {reason}"), rows)
            }
            Some(Listing::Loaded(entries)) => entries,
        };
        let visible: Vec<_> = entries
            .into_iter()
            .filter(|entry| !entry.excluded || show_excluded || self.forced.contains(&entry.path))
            .collect();
        if visible.is_empty() {
            return note("Empty folder".to_owned(), rows);
        }
        visible
            .into_iter()
            .map(|entry| {
                let id = row_id(root, &entry.path);
                let kind = match entry.kind {
                    EntryKind::Directory => RowKind::Directory,
                    EntryKind::File => RowKind::File,
                    EntryKind::Link { to_directory } => RowKind::Link { to_directory },
                };
                let label = match (&kind, entry.excluded) {
                    (RowKind::Link { .. }, _) => format!("{} →", entry.name),
                    (_, true) => format!("{} (excluded)", entry.name),
                    _ => entry.name.clone(),
                };
                rows.insert(
                    id.clone(),
                    Row {
                        root: root.to_path_buf(),
                        path: entry.path.clone(),
                        kind: kind.clone(),
                        excluded: entry.excluded,
                    },
                );
                let item = TreeItem::new(id, label);
                if kind == RowKind::Directory {
                    item.children(self.children(root, &entry.path, rows, wanted, cx))
                        .expanded(self.expanded.contains(&entry.path))
                } else {
                    item
                }
            })
            .collect()
    }

    /// Opens the file of row `id`: in a preview tab, or kept open with `keep`; in the system's
    /// application when it is not text.
    fn activate(
        &mut self,
        id: &SharedString,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.get(id).cloned() else {
            return;
        };
        match row.kind {
            RowKind::File
            | RowKind::Link {
                to_directory: false,
            } => {}
            _ => return,
        }
        // A link is opened as text at most: where it leads is not the system's to open.
        if row.kind == RowKind::File && opens_externally(&row.path) {
            open_externally(&row.path, cx);
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace.open_at(
                &row.path,
                adoc_core::SourceRange::default(),
                adoc_engine::OpenDisposition::Preview,
                window,
                cx,
            );
        });
        if keep {
            let path = row.path.clone();
            self.app.update(cx, |app, cx| {
                if let Some(id) = app.engine.document_for_path(&path) {
                    let _ = app.invoke(ids::DOCUMENT_KEEP_OPEN, CommandArgs::Document(id), cx);
                }
            });
        }
    }

    fn on_open_entry(&mut self, _: &OpenFileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone())
        else {
            return;
        };
        self.activate(&id, false, window, cx);
    }

    /// Shows `path` in the tree: opens the folders above it, listing them as needed, and
    /// selects it once listed. False when it is outside the workspace's roots.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let Some(root) = self
            .roots
            .iter()
            .find(|root| path.starts_with(root))
            .cloned()
        else {
            return false;
        };
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.starts_with(&root) {
                break;
            }
            self.expanded.insert(ancestor.to_path_buf());
            self.forced.insert(ancestor.to_path_buf());
            self.request(ancestor, cx);
        }
        self.forced.insert(path.to_path_buf());
        self.pending_reveal = Some(path.to_path_buf());
        self.rebuild(cx);
        true
    }

    /// Lists every listed folder again, keeping what is open and selected.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            app.engine.refresh_directories();
            app.drain(cx);
        });
    }

    /// Closes every folder, the roots too.
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.forced.clear();
        self.rebuild(cx);
    }

    pub fn toggle_excluded(&mut self, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let show = !app.engine.show_excluded();
            app.engine.set_show_excluded(show);
            app.drain(cx);
        });
        self.rebuild(cx);
    }

    /// The selected row's path relative to its root, with the root's name, for the footer.
    fn selected_path(&self, cx: &App) -> Option<String> {
        let item = self.tree.read(cx).selected_item()?;
        let row = self.rows.get(&item.id)?;
        let root = row.root.file_name()?.to_string_lossy();
        let relative = row.path.strip_prefix(&row.root).ok()?;
        Some(if relative.as_os_str().is_empty() {
            root.into_owned()
        } else {
            format!("{root}/{}", relative.display())
        })
    }

    #[cfg(test)]
    pub fn rows_for_test(&self, cx: &App) -> Vec<String> {
        let state = self.tree.read(cx);
        (0..)
            .map_while(|ix| state.entry(ix))
            .map(|entry| format!("{}{}", "  ".repeat(entry.depth()), entry.item().label))
            .collect()
    }

    #[cfg(test)]
    pub fn selected_for_test(&self, cx: &App) -> Option<PathBuf> {
        let item = self.tree.read(cx).selected_item()?;
        self.rows.get(&item.id).map(|row| row.path.clone())
    }
}

/// A root's name: its folder's, with the folder above when another root has the same name.
fn root_label(root: &Path, roots: &[PathBuf]) -> String {
    let name = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let own = name(root);
    let alike = roots.iter().filter(|other| name(other) == own).count() > 1;
    match root.parent().filter(|_| alike) {
        Some(parent) => format!("{own} — {}", parent.display()),
        None => own,
    }
}

/// The menu of a Files row, or of the panel's space when `row` is `None`.
fn files_menu(
    menu: PopupMenu,
    row: Option<Row>,
    files: WeakEntity<FilesView>,
    show_excluded: bool,
) -> PopupMenu {
    let mut menu = menu;
    if let Some(row) = row.filter(|row| row.kind != RowKind::Note) {
        let path = row.path.clone();
        let relative = row
            .path
            .strip_prefix(&row.root)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let id = row_id(&row.root, &row.path);
        if matches!(
            row.kind,
            RowKind::File
                | RowKind::Link {
                    to_directory: false
                }
        ) {
            let files = files.clone();
            let id = id.clone();
            menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, window, cx| {
                let _ = files.update(cx, |files, cx| files.activate(&id, false, window, cx));
            }));
            let external = path.clone();
            menu = menu.item(
                PopupMenuItem::new("Open Externally")
                    .on_click(move |_, _, cx| open_externally(&external, cx)),
            );
        }
        let full = path.display().to_string();
        let relative = relative.display().to_string();
        let shown = path.clone();
        menu = menu
            .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
            }))
            .item(
                PopupMenuItem::new("Copy Relative Path").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(relative.clone()));
                }),
            )
            .item(
                PopupMenuItem::new(reveal_label()).on_click(move |_, _, cx| {
                    cx.reveal_path(&shown);
                }),
            )
            .separator();
        if matches!(row.kind, RowKind::Directory | RowKind::Root) {
            let files = files.clone();
            let dir = row.path.clone();
            menu = menu.item(
                PopupMenuItem::new("Refresh Folder").on_click(move |_, _, cx| {
                    let _ = files.update(cx, |files, cx| {
                        files.app.update(cx, |app, cx| {
                            let _ = app.engine.refresh_directory(&dir);
                            app.drain(cx);
                        });
                    });
                }),
            );
        }
    }
    let refresh = files.clone();
    let collapse = files.clone();
    menu.item(PopupMenuItem::new("Refresh").on_click(move |_, _, cx| {
        let _ = refresh.update(cx, |files, cx| files.refresh(cx));
    }))
    .item(
        PopupMenuItem::new("Collapse All").on_click(move |_, _, cx| {
            let _ = collapse.update(cx, |files, cx| files.collapse_all(cx));
        }),
    )
    .item(
        PopupMenuItem::new("Show Excluded Files")
            .checked(show_excluded)
            .on_click(move |_, _, cx| {
                let _ = files.update(cx, |files, cx| files.toggle_excluded(cx));
            }),
    )
}

impl Render for FilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = cx.weak_entity();
        let rows = self.rows.clone();
        let show_excluded = self.app.read(cx).engine.show_excluded();
        let footer = self.selected_path(cx);
        let row_menu = {
            let files = files.clone();
            let rows = rows.clone();
            move |_: usize,
                  entry: &gpui_component::tree::TreeEntry,
                  menu: PopupMenu,
                  _: &mut Window,
                  _: &mut Context<TreeState>| {
                files_menu(
                    menu,
                    rows.get(&entry.item().id).cloned(),
                    files.clone(),
                    show_excluded,
                )
            }
        };
        let panel_menu = {
            let files = files.clone();
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                files_menu(menu, None, files.clone(), show_excluded)
            }
        };
        let list = tree(&self.tree, move |index, entry, _selected, _window, cx| {
            let item = entry.item().clone();
            let row = rows.get(&item.id).cloned();
            let kind = row.as_ref().map(|row| row.kind.clone());
            let icon = match (&kind, entry.is_expanded()) {
                (Some(RowKind::Root | RowKind::Directory), true) => IconName::FolderOpen,
                (Some(RowKind::Root | RowKind::Directory), false) => IconName::Folder,
                (Some(RowKind::Link { to_directory: true }), _) => IconName::Folder,
                _ => IconName::File,
            };
            let muted =
                matches!(kind, Some(RowKind::Note)) || row.as_ref().is_some_and(|row| row.excluded);
            let full: SharedString = row
                .as_ref()
                .map(|row| match row.kind {
                    RowKind::Link { .. } => {
                        format!("{} (a link, not followed)", row.path.display())
                    }
                    _ => row.path.display().to_string(),
                })
                .unwrap_or_default()
                .into();
            let files = files.clone();
            let id = item.id.clone();
            ListItem::new(index)
                .w_full()
                .rounded(cx.theme().radius)
                .py_0p5()
                .px_2()
                .pl(px(14.) * entry.depth() + px(8.))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("files-row-{index}")))
                        .debug_selector(move || format!("files-row-{index}"))
                        .gap_2()
                        .w_full()
                        .min_w_0()
                        .text_sm()
                        .when(muted, |row| row.text_color(cx.theme().muted_foreground))
                        .when(kind != Some(RowKind::Note), |row| row.child(icon))
                        .child(div().truncate().child(item.label.clone()))
                        .when(!full.is_empty(), |row| {
                            row.tooltip(move |window, cx| {
                                Tooltip::new(full.clone()).build(window, cx)
                            })
                        }),
                )
                .on_click(move |event, window, cx| {
                    let keep = event.click_count() > 1;
                    let _ = files.update(cx, |files, cx| files.activate(&id, keep, window, cx));
                })
        })
        .context_menu(row_menu);
        v_flex()
            .size_full()
            .child(
                div()
                    .id("files-panel")
                    .key_context(FILES_CONTEXT)
                    .on_action(cx.listener(Self::on_open_entry))
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .context_menu(panel_menu),
            )
            .when_some(footer, |panel, footer| {
                panel.child(
                    div()
                        .debug_selector(|| "files-footer".to_owned())
                        .px_2()
                        .py_1()
                        .text_xs()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(footer),
                )
            })
    }
}

/// Space opens the selected file, as a click does.
pub fn bind(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "space",
        OpenFileEntry,
        Some(FILES_CONTEXT),
    )]);
}

/// Tells the user why `path` is not in the Files tree.
pub fn outside_roots(path: &Path, window: &mut Window, cx: &mut App) {
    window.push_notification(
        format!(
            "{} is outside the workspace folders, so the Files view does not show it.",
            path.display()
        ),
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_of_the_same_name_are_told_apart() {
        let roots = [
            PathBuf::from("/work/a/docs"),
            PathBuf::from("/work/b/docs"),
            PathBuf::from("/work/site"),
        ];
        let labels: Vec<String> = roots.iter().map(|root| root_label(root, &roots)).collect();
        assert_eq!(labels, ["docs — /work/a", "docs — /work/b", "site"]);
        assert_ne!(row_id(&roots[0], &roots[0]), row_id(&roots[1], &roots[1]));
    }

    #[test]
    fn only_non_text_kinds_open_externally() {
        for external in ["logo.PNG", "guide.pdf", "diagram.svg"] {
            assert!(opens_externally(Path::new(external)), "{external}");
        }
        // Anything that could run or install itself stays out, with fonts and archives.
        for explicit in [
            "font.ttf",
            "site.zip",
            "tool.exe",
            "app.jar",
            "lib.dylib",
            "setup.msi",
        ] {
            assert!(!opens_externally(Path::new(explicit)), "{explicit}");
        }
        for text in [
            "build.sh",
            "theme.yml",
            "README.md",
            "page.adoc",
            "Makefile",
        ] {
            assert!(!opens_externally(Path::new(text)), "{text}");
        }
    }
}
