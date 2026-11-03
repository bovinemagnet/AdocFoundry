//! New Antora Project (#294): the dialog that makes a project through the engine's
//! `project.new`, and the welcome shown in place of an editor while no tab is open in a
//! workspace with no Antora component.
//!
//! Every field is read into one `NewProject` on each render and checked by
//! `Engine::check_new_project`, whose first problem shows under the field it names; the files
//! listed beside the fields are `adoc_engine::planned_tree` of the same request.

use std::cell::RefCell;

use adoc_engine::{planned_tree, NewProject, NewProjectField, Welcome};

use super::*;
use crate::editor::bindings::NewProject as NewProject_;

/// The dialog's fields, kept between renders.
struct Fields {
    location: Entity<InputState>,
    in_subfolder: Rc<Cell<bool>>,
    subfolder: Entity<InputState>,
    name: Entity<InputState>,
    title: Entity<InputState>,
    version: Entity<InputState>,
    template: Entity<SelectState<Vec<String>>>,
    git: Rc<Cell<bool>>,
    readme: Rc<Cell<bool>>,
    playbook: Rc<Cell<bool>>,
    /// The last location asked whether it is in a repository, and the answer.
    in_repository: Rc<RefCell<(String, bool)>>,
}

impl Fields {
    /// The request the fields make, and whether its folder is in a repository already.
    fn request(&self, engine: &adoc_engine::Engine, cx: &App) -> (NewProject, bool) {
        let location = self.location.read(cx).value().trim().to_owned();
        let root = PathBuf::from(&location);
        let in_repository = {
            let mut known = self.in_repository.borrow_mut();
            if known.0 != location {
                *known = (location.clone(), engine.in_repository(&root));
            }
            known.1
        };
        let subfolder = self.subfolder.read(cx).value().trim().to_owned();
        let version = self.version.read(cx).value().trim().to_owned();
        let request = NewProject {
            subfolder: self.in_subfolder.get().then_some(subfolder),
            title: self.title.read(cx).value().trim().to_owned(),
            version: (!version.is_empty()).then_some(version),
            readme: self.readme.get(),
            playbook: self.playbook.get(),
            git_init: self.git.get() && !in_repository,
            ..NewProject::new(root, self.name.read(cx).value().trim())
        };
        (request, in_repository)
    }
}

/// `text` under a field, as its problem.
fn problem_text(field: &'static str, text: String, cx: &App) -> gpui::AnyElement {
    div()
        .debug_selector(move || format!("new-project-problem-{field}"))
        .text_xs()
        .text_color(cx.theme().danger)
        .child(text)
        .into_any_element()
}

fn label(text: &'static str) -> gpui::Div {
    div().text_xs().child(text)
}

impl WorkspaceView {
    /// New Antora Project: the folder (chosen or typed, with an optional subfolder), the
    /// component's name, title and version, the template, and whether to make a repository and
    /// a README, each checked as it is typed, with the files it makes listed; Create runs
    /// `project.new`, opens the index page beside the preview and offers to undo it.
    pub(super) fn on_new_project(
        &mut self,
        _: &NewProject_,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.app.read(cx).engine.new_project_request();
        let text = |value: String, placeholder: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let fields = Rc::new(Fields {
            location: text(
                start.root.to_string_lossy().into_owned(),
                "Folder",
                window,
                cx,
            ),
            in_subfolder: Rc::new(Cell::new(start.subfolder.is_some())),
            subfolder: text(
                start.subfolder.clone().unwrap_or_else(|| "docs".to_owned()),
                "Subfolder",
                window,
                cx,
            ),
            name: text(start.name.clone(), "Component name", window, cx),
            title: text(String::new(), "Title (the name, title-cased)", window, cx),
            version: text(String::new(), "latest (unversioned)", window, cx),
            template: cx.new(|cx| {
                SelectState::new(
                    vec!["Minimal".to_owned()],
                    Some(gpui_component::IndexPath::new(0)),
                    window,
                    cx,
                )
            }),
            git: Rc::new(Cell::new(true)),
            readme: Rc::new(Cell::new(start.readme)),
            playbook: Rc::new(Cell::new(start.playbook)),
            in_repository: Rc::new(RefCell::new((String::new(), false))),
        });
        let first = fields.name.clone();
        let workspace = cx.weak_entity();
        let app = self.app.clone();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let engine = &app.read(cx).engine;
            let (request, in_repository) = fields.request(engine, cx);
            let checked = engine.check_new_project(&request);
            let under = |field: NewProjectField, name: &'static str, cx: &App| match &checked {
                Err(problem) if problem.field == field => {
                    Some(problem_text(name, problem.message.clone(), cx))
                }
                _ => None,
            };
            let root = request.root.clone();
            let mut tree = v_flex()
                .debug_selector(|| "new-project-tree".to_owned())
                .gap_0p5()
                .text_xs()
                .font_family("monospace");
            if !root.as_os_str().is_empty() {
                for (number, path) in planned_tree(&request).iter().enumerate() {
                    let shown = path
                        .strip_prefix(&root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    tree = tree.child(
                        div()
                            .debug_selector(move || format!("new-project-file-{number}"))
                            .child(shown),
                    );
                }
            }
            let choose = {
                let (location, workspace) = (fields.location.clone(), workspace.clone());
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let paths = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Choose".into()),
                    });
                    let (location, workspace) = (location.clone(), workspace.clone());
                    window
                        .spawn(cx, async move |cx| {
                            let folder = paths.await.ok()?.ok()??.into_iter().next()?;
                            cx.update(|window, cx| {
                                location.update(cx, |state, cx| {
                                    state.set_value(folder.to_string_lossy(), window, cx);
                                });
                                workspace.update(cx, |_, cx| cx.notify()).ok()
                            })
                            .ok()
                        })
                        .detach();
                }
            };
            let toggle = |id: &'static str, text: &'static str, cell: &Rc<Cell<bool>>| {
                let cell = cell.clone();
                div().debug_selector(move || id.to_owned()).child(
                    Checkbox::new(id).label(text).checked(cell.get()).on_click(
                        move |checked, window, _| {
                            cell.set(*checked);
                            window.refresh();
                        },
                    ),
                )
            };
            let form = v_flex()
                .gap_2()
                .w(px(420.))
                .child(label("Location"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&fields.location)))
                        .child(
                            Button::new("new-project-choose")
                                .label("Choose…")
                                .on_click(choose),
                        ),
                )
                .children(under(NewProjectField::Location, "location", cx))
                .child(toggle(
                    "new-project-in-subfolder",
                    "Create inside a subfolder",
                    &fields.in_subfolder,
                ))
                .when(fields.in_subfolder.get(), |form| {
                    form.child(Input::new(&fields.subfolder))
                })
                .children(under(NewProjectField::Subfolder, "subfolder", cx))
                .child(label("Component"))
                .child(Input::new(&fields.name))
                .children(under(NewProjectField::Name, "name", cx))
                .child(Input::new(&fields.title))
                .child(Input::new(&fields.version))
                .children(under(NewProjectField::Version, "version", cx))
                .child(label("Template"))
                .child(Select::new(&fields.template))
                .when(!in_repository, |form| {
                    form.child(toggle(
                        "new-project-git",
                        "Initialise a Git repository",
                        &fields.git,
                    ))
                })
                .children(under(NewProjectField::Git, "git", cx))
                .child(toggle("new-project-readme", "Add a README", &fields.readme))
                .child(toggle(
                    "new-project-playbook",
                    "Add a local playbook and build setup",
                    &fields.playbook,
                ));
            let made = match &checked {
                Ok(component) => div()
                    .debug_selector(|| "new-project-component".to_owned())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("antora.yml in {}", component.display())),
                Err(_) => div(),
            };
            let (fields, workspace) = (fields.clone(), workspace.clone());
            dialog
                .title("New Antora Project")
                .width(px(820.))
                .child(
                    h_flex().items_start().gap_4().child(form).child(
                        v_flex()
                            .flex_1()
                            .gap_2()
                            .child(label("Files"))
                            .child(tree)
                            .child(made),
                    ),
                )
                .confirm()
                .ok_text("Create")
                .on_ok(move |_, window, cx| {
                    let Some(workspace) = workspace.upgrade() else {
                        return true;
                    };
                    let (request, _) = fields.request(&workspace.read(cx).app.read(cx).engine, cx);
                    // Nothing is made until every field is right; the dialog stays open.
                    if workspace
                        .read(cx)
                        .app
                        .read(cx)
                        .engine
                        .check_new_project(&request)
                        .is_err()
                    {
                        return false;
                    }
                    workspace.update(cx, |workspace, cx| {
                        workspace.make_project(request, window, cx)
                    })
                })
        });
        first.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Runs `project.new` for `request`: shows the index page beside the preview, in the Files
    /// panel and the Navigation panel, and says what was made, with Undo. False when refused.
    fn make_project(
        &mut self,
        request: NewProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let files = planned_tree(&request).len();
        let Some(CommandOutcome::Opened(index)) = self.invoke(
            ids::PROJECT_NEW,
            CommandArgs::NewProject(request.clone()),
            window,
            cx,
        ) else {
            return false;
        };
        if !self.app.read(cx).engine.preview_layout().visible {
            self.invoke(ids::VIEW_TOGGLE_PREVIEW, CommandArgs::None, window, cx);
        }
        if let Some(path) = self
            .app
            .read(cx)
            .engine
            .document(index)
            .map(|document| document.path.clone())
        {
            self.files.update(cx, |files, cx| files.reveal(&path, cx));
        }
        self.sidebar_tab = SidebarTab::Navigation;
        let app = self.app.clone();
        window.push_notification(
            Notification::new()
                .message(format!("Created {} with {files} files.", request.name))
                .action(move |_, _, _| {
                    let app = app.clone();
                    Button::new("new-project-undo")
                        .label("Undo")
                        .debug_selector(|| "new-project-undo".to_owned())
                        .on_click(move |_, window, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.invoke(ids::EDIT_UNDO, CommandArgs::Document(index), cx)
                            });
                            window.refresh();
                        })
                }),
            cx,
        );
        cx.notify();
        true
    }

    /// What to show in place of an editor while no tab is open: the welcome, while the
    /// workspace holds no Antora component, else a pointer to the sidebar.
    pub(super) fn render_no_tabs(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let engine = &self.app.read(cx).engine;
        let Some(welcome) = engine.welcome() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Open a folder or a page from the sidebar")
                .child(
                    Button::new("open-folder")
                        .primary()
                        .label("Open Folder…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_open_folder(&OpenFolder, window, cx);
                        })),
                )
                .into_any_element();
        };
        let (message, open) = match &welcome {
            Welcome::NoFolder => ("Open a folder to begin", "Open Folder…"),
            Welcome::NoDocumentation(_) => (
                "This folder has no documentation yet",
                "Open a Different Folder…",
            ),
            Welcome::NotAntora(_) => (
                "These pages are not in an Antora component",
                "Open a Different Folder…",
            ),
        };
        let current = engine.roots().to_vec();
        let recent: Vec<PathBuf> = engine
            .recent_folders()
            .into_iter()
            .filter(|folder| !current.contains(folder))
            .take(5)
            .collect();
        let mut recents = v_flex().gap_1().items_center();
        if !recent.is_empty() {
            recents = recents.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Recent folders"),
            );
        }
        for (number, folder) in recent.into_iter().enumerate() {
            let shown = folder.to_string_lossy().into_owned();
            recents = recents.child(
                Button::new(("welcome-recent", number))
                    .ghost()
                    .small()
                    .label(shown)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_folder(folder.clone(), window, cx);
                    })),
            );
        }
        v_flex()
            .debug_selector(|| "welcome".to_owned())
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .debug_selector(move || match welcome {
                        Welcome::NoFolder => "welcome-no-folder".to_owned(),
                        Welcome::NoDocumentation(_) => "welcome-no-documentation".to_owned(),
                        Welcome::NotAntora(_) => "welcome-not-antora".to_owned(),
                    })
                    .text_lg()
                    .child(message),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("welcome-new-project")
                            .debug_selector(|| "welcome-new-project".to_owned())
                            .primary()
                            .label("New Antora Project…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_new_project(&NewProject_, window, cx);
                            })),
                    )
                    .child(
                        Button::new("welcome-open-folder")
                            .debug_selector(|| "welcome-open-folder".to_owned())
                            .label(open)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_open_folder(&OpenFolder, window, cx);
                            })),
                    ),
            )
            .child(recents)
            .into_any_element()
    }
}
