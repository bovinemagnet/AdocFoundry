//! New Antora Project (#292): `project.new` makes a component in an empty folder, or in a
//! subfolder of one that is not empty (`docs` in a code repository): `antora.yml`, the `ROOT`
//! module made as New Module makes one (`module_files`), a `.gitignore` and a README, as one
//! undo step, with an empty Git repository when asked and the folder is in none.
//!
//! `antora.yml` is written from a template with its values quoted for YAML, and read back
//! through `adoc-antora` before the command succeeds; a descriptor that does not read back
//! cleanly takes back everything written. The component joins the catalog at once (a folder
//! outside every root becomes a root), so its pages resolve without a reindex.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use adoc_antora::ResourceFamily;
use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    new_module::{module_files, title_case, valid_module_name, NewModule},
    new_page::{create_file, CreatedFile},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, UndoGroup,
};

/// The project layouts `project.new` can make.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TemplateId {
    /// `antora.yml` and a `ROOT` module with an index page and a navigation file.
    #[default]
    Minimal,
}

/// What `project.new` makes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewProject {
    /// The folder to create in. Must exist and be empty, or be a non-Antora folder with
    /// `subfolder` set.
    pub root: PathBuf,
    /// Create the component in `root/<subfolder>` (e.g. `docs`) instead of `root` itself.
    pub subfolder: Option<String>,
    /// `name:` in antora.yml: `[a-z0-9][a-z0-9._-]*`.
    pub name: String,
    /// `title:`; empty for the name, title-cased.
    pub title: String,
    /// `version:`; `None` (or `~`) writes `version: ~`.
    pub version: Option<String>,
    /// Families for ROOT (same rules as `NewModule.families`).
    pub families: Vec<ResourceFamily>,
    /// Title of `modules/ROOT/pages/index.adoc`; empty for the component's title.
    pub index_title: String,
    /// A `README.adoc` beside `antora.yml` saying what the folder is.
    pub readme: bool,
    /// A `.gitignore` for `build/`, `node_modules/` and `.cache/`.
    pub gitignore: bool,
    /// Initialise a Git repository in `root`; nothing is done when `root` is in one already.
    pub git_init: bool,
    pub template: TemplateId,
}

impl NewProject {
    /// A request for the component `name` in `root` with the defaults the dialog offers: the
    /// title from the name, versionless, pages, partials and images, a README, a
    /// `.gitignore` and a Git repository.
    #[must_use]
    pub fn new(root: PathBuf, name: &str) -> Self {
        Self {
            root,
            subfolder: None,
            name: name.to_owned(),
            title: String::new(),
            version: None,
            families: NewModule::DEFAULT_FAMILIES.to_vec(),
            index_title: String::new(),
            readme: true,
            gitignore: true,
            git_init: true,
            template: TemplateId::Minimal,
        }
    }

    /// The folder that holds `antora.yml`.
    #[must_use]
    pub fn component_root(&self) -> PathBuf {
        let root = normalize_path(&self.root);
        match self.subfolder.as_deref().map(str::trim) {
            Some(subfolder) if !subfolder.is_empty() => normalize_path(&root.join(subfolder)),
            _ => root,
        }
    }

    fn title(&self) -> String {
        match self.title.trim() {
            "" => title_case(&self.name),
            title => title.to_owned(),
        }
    }

    fn version(&self) -> Option<&str> {
        self.version
            .as_deref()
            .map(str::trim)
            .filter(|version| *version != "~")
    }
}

/// The field of the New Project dialog a problem belongs under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewProjectField {
    Location,
    Subfolder,
    Name,
    Version,
    Git,
}

/// Why a project cannot be made, and the field it belongs under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewProjectProblem {
    pub field: NewProjectField,
    pub message: String,
}

impl std::fmt::Display for NewProjectProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// What the app offers while no tab is open in a workspace with no Antora component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Welcome {
    /// No folder is open.
    NoFolder,
    /// The folder holds no AsciiDoc page.
    NoDocumentation(PathBuf),
    /// The folder's pages are in no Antora component.
    NotAntora(PathBuf),
}

/// What a group's project needs on undo and redo: its descriptor, read again, and the folders
/// it made, removed again while empty.
#[derive(Clone, Debug)]
pub(crate) struct MadeProject {
    descriptor: PathBuf,
    folders: Vec<PathBuf>,
}

/// The files and folders a project is made of.
struct Plan {
    component: PathBuf,
    files: Vec<(PathBuf, String)>,
    /// Outermost first.
    folders: Vec<PathBuf>,
}

pub(crate) fn register_new_project(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::PROJECT_NEW,
            "New Antora Project…",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        project_new,
    );
}

fn project_new(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::NewProject(request) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::PROJECT_NEW,
            expected: "a folder, a component name and what to make",
        });
    };
    let index = engine.new_project(&request)?;
    Ok(CommandOutcome::Opened(index))
}

/// The files `request` makes, in the order they are written: what the New Project dialog
/// previews.
#[must_use]
pub fn planned_tree(request: &NewProject) -> Vec<PathBuf> {
    plan(request, &descriptor_text(request))
        .files
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

fn plan(request: &NewProject, descriptor: &str) -> Plan {
    let component = request.component_root();
    let title = request.title();
    let index_title = match request.index_title.trim() {
        "" => title.clone(),
        index_title => index_title.to_owned(),
    };
    let module = component.join("modules/ROOT");
    let (module_files, module_folders) =
        module_files(&module, &request.families, true, Some(&index_title));
    let mut files = vec![(component.join("antora.yml"), descriptor.to_owned())];
    files.extend(module_files);
    if request.gitignore {
        files.push((
            component.join(".gitignore"),
            "build/\nnode_modules/\n.cache/\n".to_owned(),
        ));
    }
    if request.readme {
        files.push((component.join("README.adoc"), readme_text(request, &title)));
    }
    let mut folders = Vec::new();
    if component != normalize_path(&request.root) {
        folders.push(component.clone());
    }
    folders.push(component.join("modules"));
    folders.extend(module_folders);
    Plan {
        component,
        files,
        folders,
    }
}

/// `text` as a double-quoted YAML scalar.
fn yaml_quoted(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(character)));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn descriptor_text(request: &NewProject) -> String {
    let version = request
        .version()
        .map_or_else(|| "~".to_owned(), yaml_quoted);
    let title = request.title();
    [
        format!("name: {}", request.name),
        format!("title: {}", yaml_quoted(&title)),
        format!("version: {version}"),
        "nav:".to_owned(),
        "- modules/ROOT/nav.adoc".to_owned(),
        "asciidoc:".to_owned(),
        "  # Attributes every page of the component can use, for example:".to_owned(),
        "  #   attributes:".to_owned(),
        format!("  #     product-name: {}", title.replace(['\n', '\r'], " ")),
        "  attributes: {}".to_owned(),
        String::new(),
    ]
    .join("\n")
}

fn readme_text(request: &NewProject, title: &str) -> String {
    format!(
        "= {title}\n\n\
         The documentation of {title}, an Antora component named `{name}`: its pages are in \
         `modules/ROOT/pages`, and `antora.yml` lists its navigation.\n\n\
         Check it for broken references with:\n\n\
         [source,sh]\n----\nadocfoundry-cli check .\n----\n",
        name = request.name,
    )
}

/// Whether `folder` holds nothing but a Git repository or Finder's `.DS_Store`.
fn is_empty_folder(folder: &Path) -> Result<bool, String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
        let name = entry.file_name();
        if name != ".git" && name != ".DS_Store" {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The nearest folder from `folder` upwards that holds an `antora.yml`.
fn enclosing_component(folder: &Path) -> Option<PathBuf> {
    folder
        .ancestors()
        .find(|ancestor| ancestor.join("antora.yml").is_file())
        .map(Path::to_path_buf)
}

/// `name` made into a component name: lower case, each run of other characters a `-`; empty
/// when nothing is left.
fn component_name(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_' {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

impl Engine {
    /// Why `request` cannot be made, the first problem in the order the New Project dialog
    /// shows its fields, or the folder its `antora.yml` will be in when it can; checked before
    /// anything is written, and as a dialog types.
    pub fn check_new_project(&self, request: &NewProject) -> Result<PathBuf, NewProjectProblem> {
        use NewProjectField::{Git, Location, Name, Subfolder, Version};
        let problem = |field, message: String| Err(NewProjectProblem { field, message });
        let root = normalize_path(&request.root);
        if root.as_os_str().is_empty() {
            return problem(Location, "choose a folder for the project".to_owned());
        }
        if !root.is_dir() {
            return problem(Location, format!("{} is not a folder", root.display()));
        }
        if let Some(found) = enclosing_component(&root) {
            return problem(
                Location,
                format!(
                    "{} is in the component at {}; add a module to it with New Module instead",
                    root.display(),
                    found.display()
                ),
            );
        }
        let component = request.component_root();
        match request.subfolder.as_deref().map(str::trim) {
            Some(subfolder) if !subfolder.is_empty() => {
                let plain = Path::new(subfolder)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)));
                if !plain {
                    return problem(
                        Subfolder,
                        format!(
                            "the subfolder `{subfolder}` must be a folder name inside {}",
                            root.display()
                        ),
                    );
                }
                if component.exists() {
                    return problem(Subfolder, format!("{} exists already", component.display()));
                }
            }
            _ => match is_empty_folder(&root) {
                Err(message) => return problem(Location, message),
                Ok(false) => {
                    return problem(
                        Location,
                        format!(
                            "{} is not empty: choose an empty folder, or make the project in a \
                             subfolder such as `docs`",
                            root.display()
                        ),
                    )
                }
                Ok(true) => {}
            },
        }
        if request.name == "ROOT" || !valid_module_name(&request.name) {
            return problem(
                Name,
                format!(
                    "`{}` is not a component name: use lower-case letters, digits, `.`, `_` \
                     and `-`, starting with a letter or digit",
                    request.name
                ),
            );
        }
        if let Some(version) = request.version() {
            if version.is_empty() || version.contains('/') || version.contains(char::is_whitespace)
            {
                return problem(
                    Version,
                    format!(
                        "`{version}` is not a version: leave it empty for an unversioned \
                         component, or use no spaces and no `/`"
                    ),
                );
            }
        }
        if request.git_init && !cfg!(feature = "git") {
            return problem(
                Git,
                "this build has no Git support: leave the repository out".to_owned(),
            );
        }
        Ok(component)
    }

    /// What the New Project dialog starts from: the first root (or no folder), named from it,
    /// in a `docs` subfolder when it holds files already, with a repository made when it is in
    /// none.
    #[must_use]
    pub fn new_project_request(&self) -> NewProject {
        let Some(root) = self.roots.first().cloned() else {
            return NewProject::new(PathBuf::new(), "");
        };
        let name = root
            .file_name()
            .map(|name| component_name(&name.to_string_lossy()))
            .filter(|name| valid_module_name(name) && name != "ROOT")
            .unwrap_or_default();
        let subfolder = (!is_empty_folder(&root).unwrap_or(true)).then(|| "docs".to_owned());
        NewProject {
            subfolder,
            git_init: !self.in_repository(&root),
            ..NewProject::new(root, &name)
        }
    }

    /// Whether `path` is in a Git working tree; false without Git.
    #[must_use]
    pub fn in_repository(&self, path: &Path) -> bool {
        crate::git::in_repository(path)
    }

    /// What to offer in place of an editor while no tab is open and the workspace holds no
    /// Antora component; `None` while a whole index is under way, so nothing flashes.
    #[must_use]
    pub fn welcome(&self) -> Option<Welcome> {
        let Some(root) = self.roots.first() else {
            return Some(Welcome::NoFolder);
        };
        if self.is_indexing_workspace() || self.workspace.antora.components().next().is_some() {
            return None;
        }
        Some(if self.workspace.index.files().next().is_some() {
            Welcome::NotAntora(root.clone())
        } else {
            Welcome::NoDocumentation(root.clone())
        })
    }

    /// Makes the project `request` asks for as one undo group and opens its index page
    /// (returned). Everything is checked before anything is written, and the descriptor read
    /// back after.
    pub fn new_project(&mut self, request: &NewProject) -> Result<DocumentId, EngineError> {
        self.new_project_with(request, &descriptor_text(request))
    }

    /// [`Engine::new_project`] with the descriptor's text given.
    fn new_project_with(
        &mut self,
        request: &NewProject,
        descriptor: &str,
    ) -> Result<DocumentId, EngineError> {
        self.check_new_project(request)
            .map_err(|problem| EngineError::InvalidRequest(problem.message))?;
        let root = normalize_path(&request.root);
        let plan = plan(request, descriptor);
        let descriptor = plan.component.join("antora.yml");
        let repository =
            (request.git_init && !crate::git::in_repository(&root)).then_some(root.as_path());
        let new_root = !self.roots.iter().any(|known| root.starts_with(known));

        self.applying = true;
        let made = self.make_project(&plan, repository);
        self.applying = false;
        let (group, documents, index) = match made {
            Ok(made) => made,
            Err(error) => {
                for folder in plan.folders.iter().rev() {
                    let _ = fs::remove_dir(folder);
                }
                return Err(error);
            }
        };
        self.group_documents.insert(group, documents);
        self.group_created.insert(
            group,
            plan.files
                .into_iter()
                .map(|(path, text)| CreatedFile::new(path, text))
                .collect(),
        );
        self.group_projects.insert(
            group,
            MadeProject {
                descriptor,
                folders: plan.folders,
            },
        );
        if new_root {
            let mut roots = self.roots.clone();
            roots.push(root);
            self.open_workspace(roots)?;
        }
        self.refresh_open_diagnostics(None);
        self.audit_changed_everywhere();
        let _ = self.open(&self.documents[&index].path.clone());
        self.set_active(index);
        Ok(index)
    }

    /// Writes the project's files, reads its descriptor back, makes the repository in
    /// `repository` when given, and adds the component to the catalog with its pages open in
    /// one new group; on failure the files written are removed and the
    /// group discarded.
    fn make_project(
        &mut self,
        plan: &Plan,
        repository: Option<&Path>,
    ) -> Result<(UndoGroup, Vec<DocumentId>, DocumentId), EngineError> {
        let descriptor = plan.component.join("antora.yml");
        let mut written: Vec<PathBuf> = Vec::new();
        let result = (|| {
            for (path, text) in &plan.files {
                create_file(path, text)?;
                written.push(path.clone());
            }
            let source = fs::read_to_string(&descriptor)
                .map_err(|error| EngineError::io(&descriptor, error))?;
            adoc_antora::read_component_descriptor(&plan.component)
                .map_err(|error| EngineError::InvalidRequest(error.to_string()))?;
            if let Some(problem) =
                adoc_antora::check_component_descriptor(&plan.component, &source).first()
            {
                return Err(EngineError::InvalidRequest(format!(
                    "{}: {}",
                    descriptor.display(),
                    problem.message
                )));
            }
            if let Some(repository) = repository {
                crate::git::init_repository(repository).map_err(EngineError::InvalidRequest)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for path in written.iter().rev() {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        self.reload_descriptor(&descriptor);
        let group = self.new_group();
        let mut documents = Vec::new();
        let mut index = None;
        for (path, _) in &plan.files {
            if !adoc_index::is_asciidoc_path(path) {
                continue;
            }
            let id = self.open_unshown(path)?;
            self.push_marker(id, group);
            documents.push(id);
            if path.ends_with("modules/ROOT/pages/index.adoc") {
                index = Some(id);
            }
        }
        let index = index.ok_or_else(|| {
            EngineError::InvalidRequest("the project has no index page".to_owned())
        })?;
        Ok((group, documents, index))
    }

    /// After `group`'s files are undone (`undone`) or made again: on undo its folders removed
    /// while empty; its descriptor read again either way, so the component leaves or rejoins
    /// the catalog.
    pub(crate) fn replay_project(&mut self, group: UndoGroup, undone: bool) {
        let Some(made) = self.group_projects.get(&group).cloned() else {
            return;
        };
        if undone {
            for folder in made.folders.iter().rev() {
                let _ = fs::remove_dir(folder);
            }
        }
        self.reload_descriptor(&made.descriptor);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Settings;

    #[test]
    fn descriptor_values_are_quoted_for_yaml() {
        assert_eq!(
            yaml_quoted("Billing: \"the\" \\ guide"),
            r#""Billing: \"the\" \\ guide""#
        );
        assert_eq!(yaml_quoted("a\tb"), "\"a\\u0009b\"");
        let request = NewProject {
            title: "Lines\nand: colons".to_owned(),
            version: Some("1.0".to_owned()),
            ..NewProject::new(PathBuf::from("/x"), "demo")
        };
        let descriptor =
            adoc_antora::parse_component_descriptor(Path::new("/x"), &descriptor_text(&request))
                .unwrap();
        assert_eq!(descriptor.title.as_deref(), Some("Lines\nand: colons"));
        assert_eq!(descriptor.version.as_deref(), Some("1.0"));
    }

    #[test]
    fn scaffold_failure_undoes_itself() {
        let directory = tempfile::tempdir().unwrap();
        let root = normalize_path(directory.path());
        let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(10)));
        let request = NewProject {
            git_init: false,
            ..NewProject::new(root.clone(), "demo")
        };
        for broken in [
            "name: demo\nnav: [\n",
            "title: no name\n",
            "name: demo\nflavour: x\n",
        ] {
            let error = engine.new_project_with(&request, broken).unwrap_err();
            assert!(matches!(error, EngineError::InvalidRequest(_)), "{error}");
            assert_eq!(
                fs::read_dir(&root).unwrap().count(),
                0,
                "{broken}: nothing left"
            );
            assert!(engine.workspace().antora.components().next().is_none());
            assert!(engine.workspace().descriptor_issues.is_empty());
        }
        assert!(
            engine.new_project(&request).is_ok(),
            "and the real one works"
        );
    }
}
