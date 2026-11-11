//! The Navigation, Modules and Files panels' menus (#361): New File and New Page Here, Rename,
//! Add to Navigation and Export Pages as PDF, each carried out by an engine command
//! (`file.new`, `page.new`, `page.rename`, `nav.add_page`, `export.pdf_pages`). New Page Here and
//! Rename open the row's page first, so the dialogs they share with the menu bar act on it:
//! New Page puts its entry after the open page's. Each dialog opens once the menu that asked
//! for it has closed, since closing the menu would take back the focus given to the dialog.

use gpui::WeakEntity;
use gpui_component::menu::PopupMenu;

use super::*;

impl WorkspaceView {
    /// Asks for a name, then makes the empty file in `folder` and opens it.
    pub(crate) fn new_file_in(
        &mut self,
        folder: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = folder.to_path_buf();
        cx.defer_in(window, move |this, window, cx| {
            this.ask_new_file(folder, window, cx)
        });
    }

    fn ask_new_file(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("name.adoc"));
        let first = input.clone();
        let workspace = cx.weak_entity();
        let shown = folder.display().to_string();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (input, workspace, folder) = (input.clone(), workspace.clone(), folder.clone());
            dialog
                .title("New File")
                .width(px(520.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(format!("In {shown}"))
                        .child(Input::new(&input)),
                )
                .confirm()
                .ok_text("Create")
                .on_ok(move |_, window, cx| {
                    let name = input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    let path = folder.join(name);
                    let _ = workspace.update(cx, |workspace, cx| {
                        workspace.invoke(ids::FILE_NEW, CommandArgs::Path(path), window, cx);
                    });
                    true
                })
        });
        first.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Opens `page`, then asks for a new page to follow it in its module's navigation. Asks
    /// nothing when the page does not open (the failure is reported), since New Page acts
    /// after the active page.
    pub(crate) fn new_page_after(
        &mut self,
        page: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(TabItem::Document(id)) = self.open_at(
            page,
            SourceRange::default(),
            OpenDisposition::Preview,
            window,
            cx,
        ) else {
            return;
        };
        cx.defer_in(window, move |this, window, cx| {
            if this.active_document(cx) == Some(id) {
                this.on_new_page(&NewPage, window, cx);
            }
        });
    }

    /// Opens the page at `path` and asks for its new path, as Rename Page does for the open one.
    /// Asks nothing when the page does not open (the failure is reported).
    pub(crate) fn rename_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabItem::Document(id)) = self.open_at(
            path,
            SourceRange::default(),
            OpenDisposition::Preview,
            window,
            cx,
        ) else {
            return;
        };
        cx.defer_in(window, move |this, window, cx| {
            this.rename_document(id, window, cx);
        });
    }

    /// Lists `page` at the end of its module's navigation.
    pub(crate) fn add_to_nav(&mut self, page: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let added = self.invoke(
            ids::NAV_ADD_PAGE,
            CommandArgs::Path(page.to_path_buf()),
            window,
            cx,
        );
        if added.is_some() {
            let name = page
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            window.push_notification(
                format!(
                    "Added {} to the navigation. Save the navigation file to keep it.",
                    name.unwrap_or_default()
                ),
                cx,
            );
        }
    }

    /// Asks for a folder, then exports each of `pages` to its own PDF there.
    pub(crate) fn export_pages(
        &mut self,
        pages: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let folder = folder.await.ok()?.ok()??.into_iter().next()?;
            this.update_in(cx, |this, window, cx| {
                this.invoke(
                    ids::EXPORT_PDF_PAGES,
                    CommandArgs::ExportPages { pages, folder },
                    window,
                    cx,
                );
            })
            .ok()
        })
        .detach();
    }

    /// Says how `export.pdf_pages` went, offering to open the folder.
    pub(super) fn pages_exported(
        folder: &Path,
        written: &[PathBuf],
        failed: &[(PathBuf, String)],
        window: &mut Window,
        cx: &mut App,
    ) {
        let pages = |count: usize| if count == 1 { "page" } else { "pages" };
        let mut message = format!(
            "Exported {} {} as PDF to {}.",
            written.len(),
            pages(written.len()),
            folder.display()
        );
        if let Some((page, error)) = failed.first() {
            message.push_str(&format!(
                " {} {} could not be exported: {}: {error}",
                failed.len(),
                pages(failed.len()),
                page.display()
            ));
        }
        let folder = folder.to_path_buf();
        window.push_notification(
            Notification::new().message(message).action(move |_, _, _| {
                let folder = folder.clone();
                Button::new("open-export-folder")
                    .label("Open Folder")
                    .on_click(move |_, _, cx| cx.open_with_system(&folder))
            }),
            cx,
        );
    }
}

/// The menu of a Navigation or Modules row: what acts on the page it opens, then the panel's
/// own commands. A row that opens no page (a component, a module, a navigation heading) has
/// only the panel's.
pub(super) fn sidebar_row_menu(
    menu: PopupMenu,
    panel: SidebarPanel,
    id: &str,
    workspace: &WeakEntity<WorkspaceView>,
    app: &Entity<AppState>,
    cx: &App,
) -> PopupMenu {
    let engine = &app.read(cx).engine;
    let page = sidebar::parse_target(id)
        .map(|target| target.path)
        .filter(|path| {
            engine
                .workspace()
                .antora
                .resource_for_path(path)
                .is_some_and(|resource| {
                    resource.coordinate.family == adoc_engine::ResourceFamily::Page
                        && adoc_engine::is_asciidoc_path(path)
                })
        });
    let Some(page) = page else {
        return expansion_menu(menu, panel, app, cx);
    };
    let level = workspace
        .upgrade()
        .map(|workspace| {
            let workspace = workspace.read(cx);
            let nodes = match panel {
                SidebarPanel::Navigation => &workspace.nav_nodes,
                SidebarPanel::Modules => &workspace.family_nodes,
            };
            sidebar::level_pages(nodes, id)
        })
        .unwrap_or_default();
    let can_add = engine.can_add_to_nav(&page);
    let can_delete = engine.can_delete_files();
    let act =
        |label: &'static str,
         action: fn(&mut WorkspaceView, &Path, &mut Window, &mut Context<WorkspaceView>)| {
            let (workspace, page) = (workspace.clone(), page.clone());
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let _ = workspace.update(cx, |workspace, cx| action(workspace, &page, window, cx));
            })
        };
    let mut menu = menu;
    if panel == SidebarPanel::Navigation {
        menu = menu.item(act("New Page Here…", WorkspaceView::new_page_after));
    }
    menu = menu.item(act("Rename Page…", WorkspaceView::rename_file));
    if can_add {
        menu = menu.item(act("Add to Navigation", WorkspaceView::add_to_nav));
    }
    if !level.is_empty() {
        let workspace = workspace.clone();
        menu = menu.item(PopupMenuItem::new("Export This Level as PDF…").on_click(
            move |_, window, cx| {
                let level = level.clone();
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.export_pages(level, window, cx);
                });
            },
        ));
    }
    menu = menu
        .separator()
        .item(act("Show in Graph", |workspace, page, _, cx| {
            workspace.show_in_graph(page, cx);
        }))
        .item(act("Inspect Impact", |workspace, page, _, cx| {
            workspace.inspect_impact_of(page, cx);
        }));
    if can_delete {
        menu = menu.item(act("Delete File…", WorkspaceView::confirm_delete));
    }
    expansion_menu(menu.separator(), panel, app, cx)
}
