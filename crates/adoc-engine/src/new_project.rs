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
    templates::{
        current_year, join_relative, substitute, Template, TemplateField, Variables,
        DEFAULT_TEMPLATE,
    },
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, ConflictKind,
    DocumentId, Engine, EngineError, UndoGroup,
};

/// What `project.new` makes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewProject {
    /// The folder to create in. Must exist and be empty, or be a non-Antora folder with
    /// `subfolder` set.
    pub root: PathBuf,
    /// Create the project in `root/<subfolder>` (e.g. `docs`) instead of `root` itself.
    pub subfolder: Option<String>,
    /// `name:` in antora.yml: `[a-z0-9][a-z0-9._-]*`; a template's components are named from
    /// it (`{{name}}`).
    pub name: String,
    /// `title:`; empty for the name, title-cased.
    pub title: String,
    /// `version:`; `None` (or `~`) writes `version: ~`.
    pub version: Option<String>,
    /// Families for each module (same rules as `NewModule.families`).
    pub families: Vec<ResourceFamily>,
    /// Title of the first component's `modules/ROOT/pages/index.adoc`; empty for its title.
    pub index_title: String,
    /// A `README.adoc` in the project's folder saying what it is.
    pub readme: bool,
    /// A `.gitignore` for `build/`, `node_modules/` and `.cache/`.
    pub gitignore: bool,
    /// Initialise a Git repository in `root`; nothing is done when `root` is in one already.
    pub git_init: bool,
    /// A local `antora-playbook.yml` and a `package.json` with the Antora CLI in the project's
    /// folder, and a commented `.adocfoundry.toml` in `root` when it has none, so the project
    /// builds with `npm install` and `npm run build`.
    pub playbook: bool,
    /// The template's id ([`Engine::templates`]): `minimal` when empty.
    pub template: String,
}

impl NewProject {
    /// A request for the component `name` in `root` with the defaults the dialog offers: the
    /// minimal template, the title from the name, versionless, pages, partials and images, a
    /// README, a `.gitignore`, a local playbook and a Git repository.
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
            playbook: true,
            template: DEFAULT_TEMPLATE.to_owned(),
        }
    }

    /// The folder the project is made in, `root` or `root/<subfolder>`: the one holding
    /// `antora.yml` for a template of one component in its folder, as `minimal` is.
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
            "" => match title_case(self.name.trim()) {
                title if title.is_empty() => "Document".to_owned(),
                title => title,
            },
            title => title.to_owned(),
        }
    }

    fn version(&self) -> Option<&str> {
        self.version
            .as_deref()
            .map(str::trim)
            .filter(|version| *version != "~")
    }

    /// What `template`'s `{{…}}` stand for in this request.
    fn variables(&self) -> Variables {
        let title = self.title();
        Variables {
            name: self.name.trim().to_owned(),
            index_title: match self.index_title.trim() {
                "" => title.clone(),
                index_title => index_title.to_owned(),
            },
            title,
            version: self.version().unwrap_or_default().to_owned(),
            year: current_year().to_string(),
        }
    }
}

/// The field of the New Project dialog a problem belongs under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewProjectField {
    Location,
    Subfolder,
    Template,
    Name,
    Version,
    Git,
}

/// Why a project cannot be made, and the field it belongs under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewProjectProblem {
    pub field: NewProjectField,
    pub message: String,
    /// What is in the way on disk, when that is the problem: a folder that is not empty or a
    /// subfolder there already.
    pub conflict: Option<ConflictKind>,
}

impl From<NewProjectProblem> for EngineError {
    fn from(problem: NewProjectProblem) -> Self {
        match problem.conflict {
            Some(kind) => Self::Conflict {
                kind,
                message: problem.message,
            },
            None => Self::InvalidRequest(problem.message),
        }
    }
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

/// What a group's project needs on undo and redo: its descriptors, read again, its playbook,
/// and the folders it made, removed again while empty.
#[derive(Clone, Debug)]
pub(crate) struct MadeProject {
    descriptors: Vec<PathBuf>,
    playbook: Option<PathBuf>,
    folders: Vec<PathBuf>,
}

/// The files and folders a project is made of.
struct Plan {
    /// Each component's folder, the one holding its `antora.yml`.
    components: Vec<PathBuf>,
    files: Vec<(PathBuf, String)>,
    /// Outermost first.
    folders: Vec<PathBuf>,
    /// The page opened once it is made.
    index: PathBuf,
    playbook: Option<PathBuf>,
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

/// A component of the plan: its folder and name.
struct PlannedComponent {
    folder: PathBuf,
    name: String,
}

/// The project's files and folders, made from `template`, the first component's descriptor
/// `descriptor` when given, and the playbook building from the repository at `repository`
/// (the folder in none when `None`).
fn plan(
    request: &NewProject,
    template: &Template,
    descriptor: Option<&str>,
    repository: Option<&Path>,
) -> Result<Plan, String> {
    let project = request.component_root();
    let root = normalize_path(&request.root);
    let variables = request.variables();
    let uses = |field| template.uses(field);
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut components = Vec::new();
    for (number, spec) in template.components.iter().enumerate() {
        let name = substitute(&spec.name, &variables);
        let title = substitute(&spec.title, &variables);
        let folder = join_relative(&project, &substitute(&spec.path, &variables))?;
        let attributes: Vec<(String, String)> = spec
            .attributes
            .iter()
            .map(|(key, value)| (key.clone(), substitute(value, &variables)))
            .collect();
        let version = request.version().filter(|_| uses(TemplateField::Version));
        let text = match descriptor.filter(|_| number == 0) {
            Some(text) => text.to_owned(),
            None => descriptor_text(&name, &title, version, &spec.modules, &attributes),
        };
        files.push((folder.join("antora.yml"), text));
        for module in &spec.modules {
            let index_title = match (module.as_str(), number) {
                ("ROOT", 0) => variables.index_title.clone(),
                ("ROOT", _) => title.clone(),
                _ => title_case(module),
            };
            let (module_files, _) = module_files(
                &folder.join("modules").join(module),
                &request.families,
                true,
                Some(&index_title),
            );
            files.extend(module_files);
        }
        components.push(PlannedComponent { folder, name });
    }
    // The template's own files, each in place of one made at the same path.
    for (path, text) in &template.files {
        let path = join_relative(&project, &substitute(path, &variables))?;
        let text = substitute(text, &variables);
        match files.iter_mut().find(|(made, _)| *made == path) {
            Some(made) => made.1 = text,
            None => files.push((path, text)),
        }
    }
    // A folder with a file of its own needs no `.gitkeep`.
    let kept: Vec<PathBuf> = files
        .iter()
        .filter(|(path, _)| !path.ends_with(".gitkeep"))
        .filter_map(|(path, _)| path.parent().map(Path::to_path_buf))
        .collect();
    files.retain(|(path, _)| {
        !path.ends_with(".gitkeep")
            || !path
                .parent()
                .is_some_and(|folder| kept.contains(&folder.to_path_buf()))
    });
    let mut playbook = None;
    if template.antora {
        let title = variables.title.clone();
        if request.gitignore && uses(TemplateField::Gitignore) {
            files.push((
                project.join(".gitignore"),
                "build/\nnode_modules/\n.cache/\n".to_owned(),
            ));
        }
        let builds = request.playbook && uses(TemplateField::Playbook);
        if request.readme && uses(TemplateField::Readme) {
            files.push((
                project.join("README.adoc"),
                readme_text(&title, &project, &components, builds),
            ));
        }
        if builds {
            let path = project.join(adoc_antora::PLAYBOOK_FILE);
            files.push((
                path.clone(),
                playbook_text(&root, &title, &project, &components, repository),
            ));
            files.push((project.join("package.json"), package_text(&components)));
            let settings = root.join(".adocfoundry.toml");
            if !settings.exists() {
                files.push((settings, SETTINGS_TEXT.to_owned()));
            }
            playbook = Some(path);
        }
    }
    let index = match &template.open {
        Some(open) => join_relative(&project, &substitute(open, &variables))?,
        None => components
            .first()
            .map(|component| component.folder.join("modules/ROOT/pages/index.adoc"))
            .ok_or("the template makes no component and names no page to open")?,
    };
    if !files.iter().any(|(path, _)| *path == index) {
        return Err(format!(
            "the template makes no {} to open",
            index.strip_prefix(&project).unwrap_or(&index).display()
        ));
    }
    // Every folder a file is made in, from the project's folder down: outermost first.
    let mut folders: Vec<PathBuf> = files
        .iter()
        .filter_map(|(path, _)| path.parent())
        .flat_map(Path::ancestors)
        .filter(|folder| folder.starts_with(&project) && *folder != root)
        .map(Path::to_path_buf)
        .collect();
    folders.sort();
    folders.dedup();
    Ok(Plan {
        components: components
            .into_iter()
            .map(|component| component.folder)
            .collect(),
        files,
        folders,
        index,
        playbook,
    })
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

/// `antora.yml` for a component of `modules`, its values quoted for YAML, with `attributes`
/// in `asciidoc.attributes`, or a commented example when there are none.
fn descriptor_text(
    name: &str,
    title: &str,
    version: Option<&str>,
    modules: &[String],
    attributes: &[(String, String)],
) -> String {
    let version = version.map_or_else(|| "~".to_owned(), yaml_quoted);
    let mut lines = vec![
        format!("name: {name}"),
        format!("title: {}", yaml_quoted(title)),
        format!("version: {version}"),
        "nav:".to_owned(),
    ];
    lines.extend(
        modules
            .iter()
            .map(|module| format!("- modules/{module}/nav.adoc")),
    );
    lines.push("asciidoc:".to_owned());
    if attributes.is_empty() {
        lines.extend([
            "  # Attributes every page of the component can use, for example:".to_owned(),
            "  #   attributes:".to_owned(),
            format!("  #     product-name: {}", title.replace(['\n', '\r'], " ")),
            "  attributes: {}".to_owned(),
        ]);
    } else {
        lines.push("  attributes:".to_owned());
        lines.extend(
            attributes
                .iter()
                .map(|(key, value)| format!("    {key}: {}", yaml_quoted(value))),
        );
    }
    lines.push(String::new());
    lines.join("\n")
}

/// The README of a project of `components` in `project`, with how to build it when `builds`.
fn readme_text(
    title: &str,
    project: &Path,
    components: &[PlannedComponent],
    builds: bool,
) -> String {
    let place = |component: &PlannedComponent| {
        let relative = component
            .folder
            .strip_prefix(project)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| format!(" in `{}`", relative.display()));
        format!("`{}`{}", component.name, relative.unwrap_or_default())
    };
    let what = match components {
        [one] => format!(
            "an Antora component named {}: its pages are in each module's `pages` folder under \
             `modules`, and `antora.yml` lists its navigation",
            place(one)
        ),
        several => format!(
            "the Antora components {}, each with its own `antora.yml`",
            several.iter().map(place).collect::<Vec<_>>().join(" and ")
        ),
    };
    let mut text = format!(
        "= {title}\n\n\
         The documentation of {title}, {what}.\n\n\
         Check it for broken references with:\n\n\
         [source,sh]\n----\nadocfoundry-cli check .\n----\n",
    );
    if builds {
        text.push_str(
            "\nBuild the site with Antora, which needs Node.js and a Git repository; the pages \
             are written to `build/site`:\n\n\
             [source,sh]\n----\nnpm install\nnpm run build\n----\n",
        );
    }
    text
}

/// The default UI bundle Antora's own documentation names, fetched when the site is built.
const UI_BUNDLE: &str = "https://gitlab.com/antora/antora-ui-default/-/jobs/artifacts/HEAD/raw/build/ui-bundle.zip?job=bundle-stable";

/// The workspace settings written beside a new project: every key commented, as an example.
const SETTINGS_TEXT: &str = "\
# AdocFoundry's settings for this folder. Every key is optional; remove the `#` to use one.

# [pdf]
# theme = \"default\"

# [quality]
# enabled = true
";

/// A playbook in `project` that builds `components` from the working tree of the repository
/// at `repository` (`root` when it is in none): Antora reads a local source from its
/// repository's root, named relative to the playbook, with each component's folder within it
/// a start path. The site starts at the first component's index page.
fn playbook_text(
    root: &Path,
    title: &str,
    project: &Path,
    components: &[PlannedComponent],
    repository: Option<&Path>,
) -> String {
    let base = repository.unwrap_or(root);
    let inside = |folder: &Path| -> Vec<String> {
        folder
            .strip_prefix(base)
            .map(|relative| {
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let depth = inside(project).len();
    let url = if depth == 0 {
        ".".to_owned()
    } else {
        vec![".."; depth].join("/")
    };
    let start = components
        .first()
        .map(|component| component.name.as_str())
        .unwrap_or_default();
    let mut lines = vec![
        "site:".to_owned(),
        format!("  title: {}", yaml_quoted(title)),
        format!("  start_page: {start}::index.adoc"),
        "content:".to_owned(),
        "  sources:".to_owned(),
    ];
    if repository.is_none() {
        lines.push(
            "  # Antora builds from a Git repository: run `git init` in this folder first."
                .to_owned(),
        );
    }
    lines.push(format!("  - url: {url}"));
    lines.push("    branches: HEAD".to_owned());
    let starts: Vec<String> = components
        .iter()
        .map(|component| inside(&component.folder).join("/"))
        .collect();
    match &starts[..] {
        [one] if one.is_empty() => {}
        [one] => lines.push(format!("    start_path: {one}")),
        several => {
            lines.push("    start_paths:".to_owned());
            lines.extend(several.iter().map(|start| format!("    - {start}")));
        }
    }
    lines.extend([
        "ui:".to_owned(),
        "  bundle:".to_owned(),
        format!("    url: {UI_BUNDLE}"),
        "    snapshot: true".to_owned(),
        "output:".to_owned(),
        "  dir: build/site".to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}

/// `package.json`, named after the first component.
fn package_text(components: &[PlannedComponent]) -> String {
    format!(
        "{{\n  \"name\": \"{name}\",\n  \"private\": true,\n  \"scripts\": {{\n    \"build\": \"antora \
         antora-playbook.yml\"\n  }},\n  \"devDependencies\": {{\n    \"antora\": \"3.2.x\"\n  }}\n}}\n",
        name = components.first().map(|component| component.name.as_str()).unwrap_or("docs"),
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
    /// shows its fields, or the folder its (first) `antora.yml` will be in when it can (the
    /// page it opens for a template with no component); checked before anything is written,
    /// and as a dialog types.
    pub fn check_new_project(&self, request: &NewProject) -> Result<PathBuf, NewProjectProblem> {
        let template = self
            .template(&request.template)
            .map_err(|message| NewProjectProblem {
                field: NewProjectField::Template,
                message,
                conflict: None,
            })?;
        self.check_new_project_with(request, &template)
    }

    fn check_new_project_with(
        &self,
        request: &NewProject,
        template: &Template,
    ) -> Result<PathBuf, NewProjectProblem> {
        use NewProjectField::{Git, Location, Name, Subfolder, Version};
        let problem = |field, message: String| {
            Err(NewProjectProblem {
                field,
                message,
                conflict: None,
            })
        };
        let in_the_way = |field, kind, message: String| {
            Err(NewProjectProblem {
                field,
                message,
                conflict: Some(kind),
            })
        };
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
                    return in_the_way(
                        Subfolder,
                        ConflictKind::Exists,
                        format!("{} exists already", component.display()),
                    );
                }
            }
            _ => match is_empty_folder(&root) {
                Err(message) => return problem(Location, message),
                Ok(false) => {
                    return in_the_way(
                        Location,
                        ConflictKind::NotEmpty,
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
        let named = template.uses(TemplateField::Name);
        if named && (request.name == "ROOT" || !valid_module_name(&request.name)) {
            return problem(
                Name,
                format!(
                    "`{}` is not a component name: use lower-case letters, digits, `.`, `_` \
                     and `-`, starting with a letter or digit",
                    request.name
                ),
            );
        }
        let variables = request.variables();
        for spec in &template.components {
            let name = substitute(&spec.name, &variables);
            if name == "ROOT" || !valid_module_name(&name) {
                return problem(
                    Name,
                    format!(
                        "the template names a component `{name}`, which is not a component name"
                    ),
                );
            }
        }
        if let Some(version) = request
            .version()
            .filter(|_| template.uses(TemplateField::Version))
        {
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
        if request.git_init && template.uses(TemplateField::Git) && !cfg!(feature = "git") {
            return problem(
                Git,
                "this build has no Git support: leave the repository out".to_owned(),
            );
        }
        let plan = plan(request, template, None, None).map_err(|message| NewProjectProblem {
            field: NewProjectField::Template,
            message,
            conflict: None,
        })?;
        Ok(plan.components.first().cloned().unwrap_or(plan.index))
    }

    /// The files `request` makes, in the order they are written: what the New Project dialog
    /// previews. Empty when its template cannot be read.
    #[must_use]
    pub fn planned_tree(&self, request: &NewProject) -> Vec<PathBuf> {
        self.template(&request.template)
            .and_then(|template| plan(request, &template, None, None))
            .map(|plan| plan.files.into_iter().map(|(path, _)| path).collect())
            .unwrap_or_default()
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
    /// (returned). Everything is checked before anything is written, and each descriptor read
    /// back after.
    pub fn new_project(&mut self, request: &NewProject) -> Result<DocumentId, EngineError> {
        self.new_project_with(request, None)
    }

    /// [`Engine::new_project`] with the first descriptor's text given.
    fn new_project_with(
        &mut self,
        request: &NewProject,
        descriptor: Option<&str>,
    ) -> Result<DocumentId, EngineError> {
        let template = self
            .template(&request.template)
            .map_err(EngineError::InvalidRequest)?;
        self.check_new_project_with(request, &template)?;
        let root = normalize_path(&request.root);
        // The repository the project is in, or the one it makes, which its playbook builds from.
        let existing = crate::git::repository_root(&root);
        let init = request.git_init && template.uses(TemplateField::Git) && existing.is_none();
        let built_from = existing.or_else(|| init.then(|| root.clone()));
        let plan = plan(request, &template, descriptor, built_from.as_deref())
            .map_err(EngineError::InvalidRequest)?;
        let repository = init.then_some(root.as_path());
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
                descriptors: plan
                    .components
                    .iter()
                    .map(|component| component.join("antora.yml"))
                    .collect(),
                playbook: plan.playbook,
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

    /// Writes the project's files, reads each descriptor back, makes the repository in
    /// `repository` when given, and adds the components to the catalog with their pages open
    /// in one new group; on failure the files written are removed and the group discarded.
    fn make_project(
        &mut self,
        plan: &Plan,
        repository: Option<&Path>,
    ) -> Result<(UndoGroup, Vec<DocumentId>, DocumentId), EngineError> {
        let mut written: Vec<PathBuf> = Vec::new();
        let result = (|| {
            for (path, text) in &plan.files {
                create_file(path, text)?;
                written.push(path.clone());
            }
            for component in &plan.components {
                let descriptor = component.join("antora.yml");
                let source = fs::read_to_string(&descriptor)
                    .map_err(|error| EngineError::io(&descriptor, error))?;
                adoc_antora::read_component_descriptor(component)
                    .map_err(|error| EngineError::InvalidRequest(error.to_string()))?;
                if let Some(problem) =
                    adoc_antora::check_component_descriptor(component, &source).first()
                {
                    return Err(EngineError::InvalidRequest(format!(
                        "{}: {}",
                        descriptor.display(),
                        problem.message
                    )));
                }
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
        for component in &plan.components {
            self.reload_descriptor(&component.join("antora.yml"));
        }
        if let Some(playbook) = &plan.playbook {
            self.adopt_playbook(playbook);
        }
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
            if *path == plan.index {
                index = Some(id);
            }
        }
        let index = index.ok_or_else(|| {
            EngineError::InvalidRequest("the project has no page to open".to_owned())
        })?;
        Ok((group, documents, index))
    }

    /// After `group`'s files are undone (`undone`) or made again: on undo its folders removed
    /// while empty; its descriptors read again either way, so the components leave or rejoin
    /// the catalog, and its playbook adopted or forgotten.
    pub(crate) fn replay_project(&mut self, group: UndoGroup, undone: bool) {
        let Some(made) = self.group_projects.get(&group).cloned() else {
            return;
        };
        if undone {
            for folder in made.folders.iter().rev() {
                let _ = fs::remove_dir(folder);
            }
        }
        for descriptor in &made.descriptors {
            self.reload_descriptor(descriptor);
        }
        if let Some(playbook) = &made.playbook {
            self.adopt_playbook(playbook);
        }
    }

    /// Makes the playbook at `path` the site's when the workspace had none, or forgets it once
    /// it is gone, so Build Site follows a project made or undone at once.
    fn adopt_playbook(&mut self, path: &Path) {
        match self.workspace.antora.playbook() {
            None if path.is_file() => {
                if let Ok(playbook) = adoc_antora::read_playbook(path) {
                    self.workspace.antora.set_playbook(Some(playbook));
                }
            }
            Some(playbook) if playbook.path == path && !path.is_file() => {
                self.workspace.antora.set_playbook(None);
            }
            _ => {}
        }
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
        let text = descriptor_text(
            &request.name,
            &request.title(),
            request.version(),
            &["ROOT".to_owned()],
            &[("page-pagination".to_owned(), String::new())],
        );
        let descriptor = adoc_antora::parse_component_descriptor(Path::new("/x"), &text).unwrap();
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
            let error = engine.new_project_with(&request, Some(broken)).unwrap_err();
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
