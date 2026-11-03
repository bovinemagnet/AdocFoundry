//! Antora-aware creation (MCP PRD §7.6): `create_page` and `create_partial`, adapters to the
//! engine commands the app's New Page and New Partial invoke (`page.new`, `page.new_partial`),
//! so the page's place, name, navigation entry and undo are the app's. Nothing here writes a
//! file. Both apply at once and need `--allow-write`; the reviewed path for a change of several
//! files is `preview_changes` and `apply_changes`, whose `create_page` and `create_partial`
//! operations stage the same plans. `create_module` invokes New Module's `module.new`, and
//! `create_project` New Antora Project's `project.new` (#292), in a folder inside the roots
//! only, so it never adds a root.

use adoc_engine::{commands::ids, text_hash, CommandArgs, CommandOutcome, NewPage};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    changes::{module_root, plain},
    responses::{ErrorCode, Failure},
    schemas,
    server::{Answer, Server, State},
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, JsonSchema)]
struct CreatePageArgs {
    component: String,
    /// `~` for the versionless version; needed when the component has several.
    version: Option<String>,
    module: String,
    /// The page's title, `= Title`; its file is named from it unless `file_name` is given.
    title: String,
    /// The page's path in the module's `pages` folder.
    file_name: Option<String>,
    /// Add an entry for the page at the end of the module's navigation file.
    #[serde(default)]
    add_to_navigation: bool,
    /// The SHA-256 (hex) the navigation file must hold now, as a preview or
    /// `get_page_context` saw it.
    expected_nav_hash: Option<String>,
    /// Write the page and the navigation file to disk; true by default. The page's file is
    /// made in any case, as New Page makes it.
    save: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct CreateModuleArgs {
    component: String,
    /// `~` for the versionless version; needed when the component has several.
    version: Option<String>,
    /// Lower-case letters, digits, `.`, `_` and `-`; `ROOT` only when there is none.
    name: String,
    /// The families besides `pages`: `partials`, `examples`, `images`, `attachments`;
    /// partials and images without it.
    families: Option<Vec<String>>,
    /// Make `nav.adoc` and list it in `antora.yml`'s `nav:`; true by default.
    navigation: Option<bool>,
    /// Make `pages/index.adoc` with this title; named from the module without it. Pass
    /// `index: false` for no page.
    index_title: Option<String>,
    /// Make an index page; true by default.
    index: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct CreateProjectArgs {
    /// The folder to make the project in, inside a workspace root: empty, or holding no
    /// Antora component when `subfolder` is given.
    path: String,
    /// Make the component in this new folder inside `path` (`docs` in a code repository).
    subfolder: Option<String>,
    /// The component's `name`: lower-case letters, digits, `.`, `_` and `-`.
    name: String,
    /// The component's `title`; the name, title-cased, without it.
    title: Option<String>,
    /// The component's `version`; unversioned (`~`) without it.
    version: Option<String>,
    /// The `ROOT` module's families besides `pages`: `partials`, `examples`, `images`,
    /// `attachments`; partials and images without it.
    families: Option<Vec<String>>,
    /// The index page's title; the component's title without it.
    index_title: Option<String>,
    /// Make a `README.adoc`; true by default.
    readme: Option<bool>,
    /// Make a `.gitignore`; true by default.
    gitignore: Option<bool>,
    /// Make a local `antora-playbook.yml` and a `package.json` with the Antora CLI beside
    /// `antora.yml`, and a commented `.adocfoundry.toml` in `path` when it has none; true by
    /// default. Nothing is installed or built.
    playbook: Option<bool>,
    /// Initialise an empty Git repository in `path` when it is in none; false by default.
    #[serde(default)]
    git_init: bool,
}

#[derive(Deserialize, JsonSchema)]
struct CreatePartialArgs {
    component: String,
    /// `~` for the versionless version; needed when the component has several.
    version: Option<String>,
    module: String,
    /// The partial's path in the module's `partials` folder.
    file_name: String,
    /// What the partial holds; empty without it.
    #[serde(default)]
    text: String,
}

/// Saves each of `paths` that is open, and the pages that could not be.
fn save_all(state: &mut State, paths: &[std::path::PathBuf]) -> Vec<Value> {
    let mut failed = Vec::new();
    for path in paths {
        let Some(id) = state.engine.document_for_path(path) else {
            continue;
        };
        if let Err(error) = state
            .engine
            .invoke(ids::DOCUMENT_SAVE, CommandArgs::Document(id))
        {
            failed.push(json!({ "path": state.shown(path), "message": error.to_string() }));
        }
    }
    failed
}

#[tool_router(router = creation_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Make a page in a component version's module, as the app's New Page does (`page.new`): named from its title, with an entry at the end of the module's navigation file when `add_to_navigation`, as one undo step. Applies at once; to review it first, use `preview_changes` with a `create_page` operation.",
        output_schema = schemas::create_page(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_page(&self, Parameters(args): Parameters<CreatePageArgs>) -> Answer {
        self.run(move |state| {
            let root = module_root(state, &args.component, args.version.as_deref(), &args.module)?;
            let file = args.file_name.as_deref().map(plain).transpose()?;
            let request = NewPage {
                module: root,
                title: args.title.clone(),
                add_to_nav: args.add_to_navigation,
                file,
            };
            // Worked out first, without changing anything: the page's path and the navigation
            // entry the command will make, checked against the roots and the expected hash.
            let staged = state.engine.stage_new_page(&request).map_err(|error| {
                let message = error.to_string();
                let code = if message.ends_with("already exists") {
                    ErrorCode::Conflict
                } else {
                    ErrorCode::InvalidArgument
                };
                Failure::new(code, message)
            })?;
            for path in staged.paths() {
                state.confine(&path.to_string_lossy())?;
            }
            let nav = staged.edits.first().map(|file| file.path.clone());
            if let Some(expected) = &args.expected_nav_hash {
                let now = nav
                    .as_deref()
                    .and_then(|nav| state.engine.current_text(nav))
                    .map(|text| text_hash(&text));
                if now.as_ref() != Some(expected) {
                    return Err(Failure::new(
                        ErrorCode::Conflict,
                        "the navigation file does not hold the text `expected_nav_hash` names",
                    ));
                }
            }
            let CommandOutcome::Opened(page) = state
                .engine
                .invoke(ids::PAGE_NEW, CommandArgs::NewPage(request))
                .map_err(Failure::from)?
            else {
                return Err(Failure::internal("New Page opened no page"));
            };
            let path = state
                .engine
                .document(page)
                .map(|document| document.path.clone())
                .ok_or_else(|| Failure::internal("the new page is not open"))?;
            let mut changed = vec![path.clone()];
            changed.extend(nav.clone());
            let save = args.save.unwrap_or(true);
            let failed = if save { save_all(state, &changed) } else { Vec::new() };
            if !failed.is_empty() {
                return Err(Failure::new(
                    ErrorCode::InternalError,
                    "the page was made, but a file could not be saved; it stays in the server's buffers",
                )
                .with_details(json!({ "applied": true, "failed": failed })));
            }
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "path": state.shown(&path),
                "resource_id": state.engine.resource_id(&path),
                "navigation_file": nav.as_deref().map(|nav| state.shown(nav)),
                "changed": changed.iter().map(|path| state.shown(path)).collect::<Vec<_>>(),
                "saved": save,
                "workspace_revision": state.engine.workspace_revision(),
            }))
        })
        .await
    }

    #[tool(
        description = "Make a module in a component version, as the app's New Module does (`module.new`): its families (each empty one kept by a `.gitkeep`), an index page and a navigation file listed in `antora.yml`, as one undo step. `antora.yml` gains one line, written as its other entries are; a `nav:` that is not a plain list is refused.",
        output_schema = schemas::create_module(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_module(&self, Parameters(args): Parameters<CreateModuleArgs>) -> Answer {
        self.run(move |state| {
            let catalog = &state.engine.workspace().antora;
            let version = match args.version.as_deref() {
                Some("~") => None,
                Some(version) => Some(version.to_owned()),
                None => match catalog.versions_of(&args.component).as_slice() {
                    [] => {
                        return Err(Failure::not_found(format!(
                            "no component `{}` in the workspace",
                            args.component
                        )))
                    }
                    [only] => only.version.clone(),
                    _ => {
                        return Err(Failure::invalid(format!(
                            "`{}` has several versions; give the `version`",
                            args.component
                        )))
                    }
                },
            };
            let descriptor = catalog
                .component(&args.component, version.as_deref())
                .ok_or_else(|| {
                    Failure::not_found(format!(
                        "no component `{}` at version `{}`",
                        args.component,
                        version.as_deref().unwrap_or("~")
                    ))
                })?;
            let root = state.confine(&descriptor.root.to_string_lossy())?;
            let mut families = vec![adoc_engine::ResourceFamily::Page];
            match &args.families {
                None => families.extend(adoc_engine::NewModule::DEFAULT_FAMILIES),
                Some(names) => {
                    for name in names {
                        let family = adoc_engine::ResourceFamily::ALL
                            .into_iter()
                            .find(|family| family.directory() == name || family.to_string() == *name)
                            .ok_or_else(|| Failure::invalid(format!("unknown family `{name}`")))?;
                        families.push(family);
                    }
                }
            }
            let folder = state
                .engine
                .check_new_module(&root, &args.name)
                .map_err(|problem| {
                    let code = if problem.contains("exists already") {
                        ErrorCode::Conflict
                    } else {
                        ErrorCode::InvalidArgument
                    };
                    Failure::new(code, problem)
                })?;
            state.confine(&folder.to_string_lossy())?;
            let request = adoc_engine::NewModule {
                component: root.clone(),
                name: args.name.clone(),
                families,
                nav: args.navigation.unwrap_or(true),
                index_title: args
                    .index
                    .unwrap_or(true)
                    .then(|| args.index_title.clone().unwrap_or_default()),
            };
            state
                .engine
                .invoke(ids::MODULE_NEW, CommandArgs::NewModule(request))
                .map_err(|error| match error {
                    adoc_engine::EngineError::InvalidRequest(_) => {
                        Failure::new(ErrorCode::InvalidArgument, error.to_string())
                    }
                    other => Failure::from(other),
                })?;
            let mut made = Vec::new();
            let mut folders = vec![folder.clone()];
            while let Some(folder) = folders.pop() {
                for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                    if entry.path().is_dir() {
                        folders.push(entry.path());
                    } else {
                        made.push(state.shown(&entry.path()));
                    }
                }
            }
            made.sort();
            let index = folder.join("pages/index.adoc");
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "module": args.name,
                "path": state.shown(&folder),
                "made": made,
                "descriptor": args.navigation.unwrap_or(true).then(|| state.shown(&root.join("antora.yml"))),
                "index_page": index.is_file().then(|| state.engine.resource_id(&index)).flatten(),
                "workspace_revision": state.engine.workspace_revision(),
            }))
        })
        .await
    }

    #[tool(
        description = "Make an Antora project in a folder inside the workspace, as the app's New Antora Project does (`project.new`): `antora.yml`, a `ROOT` module with an index page and a navigation file, a `.gitignore` and a README, as one undo step; the folder must be empty, or the project goes in a new `subfolder`. A Git repository is made only with `git_init`, and never inside another.",
        output_schema = schemas::create_project(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_project(&self, Parameters(args): Parameters<CreateProjectArgs>) -> Answer {
        self.run(move |state| {
            let root = state.confine(&args.path)?;
            let mut families = Vec::new();
            match &args.families {
                None => families.extend(adoc_engine::NewModule::DEFAULT_FAMILIES),
                Some(names) => {
                    for name in names {
                        let family = adoc_engine::ResourceFamily::ALL
                            .into_iter()
                            .find(|family| {
                                family.directory() == name || family.to_string() == *name
                            })
                            .ok_or_else(|| Failure::invalid(format!("unknown family `{name}`")))?;
                        families.push(family);
                    }
                }
            }
            let request = adoc_engine::NewProject {
                subfolder: args.subfolder.clone(),
                title: args.title.clone().unwrap_or_default(),
                version: args.version.clone(),
                families,
                index_title: args.index_title.clone().unwrap_or_default(),
                readme: args.readme.unwrap_or(true),
                gitignore: args.gitignore.unwrap_or(true),
                playbook: args.playbook.unwrap_or(true),
                git_init: args.git_init,
                ..adoc_engine::NewProject::new(root.clone(), &args.name)
            };
            let component = state
                .engine
                .check_new_project(&request)
                .map_err(|problem| {
                    let conflict = problem.message.contains("exists already")
                        || problem.message.contains("not empty");
                    let code = if conflict {
                        ErrorCode::Conflict
                    } else {
                        ErrorCode::InvalidArgument
                    };
                    Failure::new(code, problem.message)
                })?;
            state.confine(&component.to_string_lossy())?;
            // Listed before, as what is planned depends on what is there already.
            let mut made: Vec<String> = adoc_engine::planned_tree(&request)
                .iter()
                .map(|path| state.shown(path))
                .collect();
            let had_repository = root.join(".git").exists();
            let CommandOutcome::Opened(index) = state
                .engine
                .invoke(ids::PROJECT_NEW, CommandArgs::NewProject(request.clone()))
                .map_err(|error| match error {
                    adoc_engine::EngineError::InvalidRequest(_) => {
                        Failure::new(ErrorCode::InvalidArgument, error.to_string())
                    }
                    other => Failure::from(other),
                })?
            else {
                return Err(Failure::internal("the project has no index page"));
            };
            made.sort();
            let index_page = state
                .engine
                .document(index)
                .and_then(|document| state.engine.resource_id(&document.path.clone()));
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "component": args.name,
                "path": state.shown(&component),
                "made": made,
                "index_page": index_page,
                "repository": !had_repository && root.join(".git").exists(),
                "workspace_revision": state.engine.workspace_revision(),
            }))
        })
        .await
    }

    #[tool(
        description = "Make a partial in a component version's module, as the app's New Partial does (`page.new_partial`), with its text as a second undo step, and give the include that names it: from its own module, and from anywhere.",
        output_schema = schemas::create_partial(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_partial(&self, Parameters(args): Parameters<CreatePartialArgs>) -> Answer {
        self.run(move |state| {
            let root = module_root(state, &args.component, args.version.as_deref(), &args.module)?;
            let relative = plain(&args.file_name)?;
            let path = state.confine(&root.join("partials").join(&relative).to_string_lossy())?;
            let CommandOutcome::Opened(id) = state
                .engine
                .invoke(ids::PAGE_NEW_PARTIAL, CommandArgs::Path(path.clone()))
                .map_err(|error| match error {
                    adoc_engine::EngineError::InvalidRequest(_) => {
                        Failure::new(ErrorCode::Conflict, error.to_string())
                    }
                    other => Failure::from(other),
                })?
            else {
                return Err(Failure::internal("New Partial opened no file"));
            };
            if !args.text.is_empty() {
                state
                    .engine
                    .invoke(
                        ids::EDIT_INSERT_TEXT,
                        CommandArgs::Text {
                            id,
                            range: adoc_core::SourceRange::new(0, 0),
                            text: args.text.clone(),
                        },
                    )
                    .map_err(Failure::from)?;
                let failed = save_all(state, std::slice::from_ref(&path));
                if !failed.is_empty() {
                    return Err(Failure::new(
                        ErrorCode::InternalError,
                        "the partial was made, but its text could not be saved; it stays in the server's buffers",
                    )
                    .with_details(json!({ "applied": true, "failed": failed })));
                }
            }
            let resource_id = state.engine.resource_id(&path);
            let relative = relative.to_string_lossy().replace('\\', "/");
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "path": state.shown(&path),
                "resource_id": resource_id,
                "include": {
                    "from_its_module": format!("include::partial${relative}[]"),
                    "from_anywhere": resource_id.as_deref().map(|id| format!("include::{id}[]")),
                },
                "workspace_revision": state.engine.workspace_revision(),
            }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use adoc_engine::{Engine, Settings};

    use super::*;

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// MCP PRD §11.3 test 18: the page made through `create_page` is the app's New Page,
    /// resource id, navigation entry and all, and one undo takes back both.
    #[test]
    fn mcp_create_page_updates_nav_and_undo() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_engine::normalize_path(&directory.path().join("discovery"));
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery"),
            &root,
        );
        let mut engine = Engine::new(Settings::default().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        let server = Server::new(
            engine,
            vec![root.clone()],
            crate::server::Permissions {
                write: true,
                build: false,
            },
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let made = runtime
            .block_on(server.create_page(Parameters(CreatePageArgs {
                component: "product".into(),
                version: Some("~".into()),
                module: "ROOT".into(),
                title: "Configuring Authentication".into(),
                file_name: None,
                add_to_navigation: true,
                expected_nav_hash: None,
                save: Some(false),
            })))
            .unwrap();
        let made = made.structured_content.unwrap();
        assert_eq!(
            made["resource_id"],
            "product:ROOT:page$configuring-authentication.adoc"
        );

        let page = root.join("product/main/modules/ROOT/pages/configuring-authentication.adoc");
        let nav = root.join("product/main/modules/ROOT/nav.adoc");
        let mut state = server.state.lock().unwrap();
        let engine = &mut state.engine;
        let page_id = engine.document_for_path(&page).unwrap();
        let nav_id = engine.document_for_path(&nav).unwrap();
        assert!(engine
            .document(nav_id)
            .unwrap()
            .text_string()
            .ends_with("* xref:configuring-authentication.adoc[]\n"));
        assert!(page.exists());

        // As in the app: undoing in the page takes back the page and its entry together.
        assert!(engine.undo(page_id));
        assert!(!page.exists());
        assert!(!engine
            .document(nav_id)
            .unwrap()
            .text_string()
            .contains("configuring-authentication"));
    }
}
