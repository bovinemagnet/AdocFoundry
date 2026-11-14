//! New Antora Project's templates (#295). A template is data: a `template.toml` saying what it
//! is called, which of the request's fields it uses, and its components (each with its name,
//! folder, title, modules and `asciidoc.attributes`), and the files beside it, copied into the
//! project with `{{name}}`, `{{title}}`, `{{version}}`, `{{index_title}}` and `{{year}}`
//! substituted in their contents and paths. The engine still writes every component's
//! `antora.yml`, its modules (as New Module makes them, a template's file replacing one made at
//! the same path), the playbook, `package.json`, README and `.gitignore`, so their quoting and
//! the playbook's paths are right whatever the template says.
//!
//! The built-in templates are `templates/<id>/` in this crate, embedded by `build.rs`. Others
//! are the subfolders holding a `template.toml` of `[templates] dir` in the user settings
//! (beside the settings file by default) and in a workspace's `.adocfoundry.toml` (inside the
//! workspace only), listed after the built-ins and read again on every call. A folder
//! template's links are never followed, its files must be text, and a path that would leave
//! the project is refused.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{new_module::valid_module_name, Engine};

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// The built-in templates, in the order they are offered.
const BUILTIN_ORDER: [&str; 4] = ["minimal", "product-docs", "multi-component", "single-page"];

/// The template a request names when it names none.
pub const DEFAULT_TEMPLATE: &str = "minimal";

const TEMPLATE_FILE: &str = "template.toml";

/// The most files, and bytes in one file, a folder template may hold.
const MAX_FILES: usize = 500;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A field of New Antora Project that a template may use; the dialog hides the rest, and the
/// engine ignores them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TemplateField {
    Name,
    Title,
    Version,
    IndexTitle,
    Readme,
    Gitignore,
    Playbook,
    Git,
}

impl TemplateField {
    pub const ALL: [Self; 8] = [
        Self::Name,
        Self::Title,
        Self::Version,
        Self::IndexTitle,
        Self::Readme,
        Self::Gitignore,
        Self::Playbook,
        Self::Git,
    ];

    /// The field's name in `template.toml`'s `fields`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Title => "title",
            Self::Version => "version",
            Self::IndexTitle => "index_title",
            Self::Readme => "readme",
            Self::Gitignore => "gitignore",
            Self::Playbook => "playbook",
            Self::Git => "git",
        }
    }
}

/// Where a template comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateSource {
    Builtin,
    /// A folder in the user settings' `[templates] dir`.
    User(PathBuf),
    /// A folder in the workspace settings' `[templates] dir`.
    Workspace(PathBuf),
}

/// A project template, read and checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Template {
    /// `minimal`, or `user/<folder>` or `workspace/<folder>` for a folder template.
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: TemplateSource,
    /// False for a template with no Antora component, such as `single-page`.
    pub antora: bool,
    fields: Vec<TemplateField>,
    pub(crate) components: Vec<ComponentSpec>,
    /// The page opened once the project is made, relative to the project's folder; the first
    /// component's `ROOT` index page when `None`.
    pub(crate) open: Option<String>,
    /// Each file beside `template.toml`, by its `/`-separated path, unsubstituted.
    pub(crate) files: Vec<(String, String)>,
}

impl Template {
    /// Whether the template uses `field` of the request.
    #[must_use]
    pub fn uses(&self, field: TemplateField) -> bool {
        self.fields.contains(&field)
    }
}

/// A component a template makes, its strings unsubstituted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComponentSpec {
    pub name: String,
    /// Relative to the project's folder; `.` for the folder itself.
    pub path: String,
    pub title: String,
    pub modules: Vec<String>,
    pub attributes: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    name: String,
    #[serde(default)]
    description: String,
    antora: Option<bool>,
    fields: Option<Vec<String>>,
    open: Option<String>,
    #[serde(default, rename = "component")]
    components: Vec<RawComponent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawComponent {
    name: Option<String>,
    path: Option<String>,
    title: Option<String>,
    modules: Option<Vec<String>>,
    #[serde(default)]
    attributes: toml::Table,
}

/// The values a template's `{{…}}` stand for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Variables {
    pub name: String,
    pub title: String,
    /// Empty for a versionless component.
    pub version: String,
    pub index_title: String,
    pub year: String,
}

/// `text` with each `{{variable}}` replaced by its value; any other `{{…}}` is left as it is.
pub(crate) fn substitute(text: &str, variables: &Variables) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let value = after.find("}}").and_then(|end| {
            let value = match &after[..end] {
                "name" => &variables.name,
                "title" => &variables.title,
                "version" => &variables.version,
                "index_title" => &variables.index_title,
                "year" => &variables.year,
                _ => return None,
            };
            Some((value, end))
        });
        match value {
            Some((value, end)) => {
                out.push_str(value);
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `folder` joined with the `/`-separated `relative`, refused when it would leave `folder`,
/// reach a Git repository's own folder, or name a part no file system takes; `.` is `folder`
/// itself.
pub(crate) fn join_relative(folder: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative == "." {
        return Ok(folder.to_path_buf());
    }
    let mut path = folder.to_path_buf();
    for part in relative.split('/') {
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(['\\', ':', '\0'])
            || part
                .chars()
                .any(|character| character.is_control() || ignorable(character));
        if bad {
            return Err(format!("`{relative}` is not a path inside the project"));
        }
        if is_git_folder(part) {
            return Err(format!(
                "`{relative}` reaches a Git repository's own folder, which a template never writes"
            ));
        }
        path.push(part);
    }
    Ok(path)
}

/// Whether `part` names `.git` on some file system: in any case (macOS, Windows), with trailing
/// dots or spaces (Windows drops them), or as its short name `git~1` (NTFS). Git refuses the
/// same names when it checks a tree out.
fn is_git_folder(part: &str) -> bool {
    let trimmed = part.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    trimmed == ".git"
        || trimmed.strip_prefix("git~").is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// The code points HFS+ ignores in names, as Git lists them (`is_hfs_dotgit`), so `.g\u{200c}it`
/// is `.git` there: never part of a template's path.
fn ignorable(character: char) -> bool {
    matches!(
        character,
        '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'
    )
}

/// Whether `name` can be an AsciiDoc attribute's name.
fn attribute_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
}

/// The template `template.toml` (`text`) describes, with its `files`.
fn parse(
    id: String,
    source: TemplateSource,
    text: &str,
    files: Vec<(String, String)>,
) -> Result<Template, String> {
    let raw: RawTemplate =
        toml::from_str(text).map_err(|error| format!("{TEMPLATE_FILE}: {}", error.message()))?;
    let fields = match raw.fields {
        None => TemplateField::ALL.to_vec(),
        Some(keys) => keys
            .iter()
            .map(|key| {
                TemplateField::ALL
                    .into_iter()
                    .find(|field| field.key() == key)
                    .ok_or_else(|| format!("{TEMPLATE_FILE}: `{key}` is not a field"))
            })
            .collect::<Result<_, _>>()?,
    };
    let antora = raw.antora.unwrap_or(true);
    let mut raw_components = raw.components;
    if antora && raw_components.is_empty() {
        raw_components.push(RawComponent {
            name: None,
            path: None,
            title: None,
            modules: None,
            attributes: toml::Table::new(),
        });
    }
    if !antora && !raw_components.is_empty() {
        return Err(format!(
            "{TEMPLATE_FILE}: a template with `antora = false` has no components"
        ));
    }
    let mut components = Vec::new();
    for raw in raw_components {
        let modules = raw.modules.unwrap_or_else(|| vec!["ROOT".to_owned()]);
        if !modules.iter().any(|module| module == "ROOT") {
            return Err(format!(
                "{TEMPLATE_FILE}: every component needs a ROOT module"
            ));
        }
        if let Some(bad) = modules.iter().find(|module| !valid_module_name(module)) {
            return Err(format!("{TEMPLATE_FILE}: `{bad}` is not a module name"));
        }
        let mut attributes = Vec::new();
        for (key, value) in raw.attributes {
            if !attribute_name(&key) {
                return Err(format!("{TEMPLATE_FILE}: `{key}` is not an attribute name"));
            }
            let value = value
                .as_str()
                .ok_or_else(|| format!("{TEMPLATE_FILE}: attribute `{key}` must be a string"))?;
            attributes.push((key, value.to_owned()));
        }
        components.push(ComponentSpec {
            name: raw.name.unwrap_or_else(|| "{{name}}".to_owned()),
            path: raw.path.unwrap_or_else(|| ".".to_owned()),
            title: raw.title.unwrap_or_else(|| "{{title}}".to_owned()),
            modules,
            attributes,
        });
    }
    if !antora && raw.open.is_none() {
        return Err(format!(
            "{TEMPLATE_FILE}: a template with `antora = false` names the page to `open`"
        ));
    }
    Ok(Template {
        id,
        name: raw.name,
        description: raw.description,
        source,
        antora,
        fields,
        components,
        open: raw.open,
        files,
    })
}

/// The built-in template `id`.
fn builtin(id: &str) -> Option<Template> {
    let (_, files) = BUILTIN_FILES.iter().find(|(name, _)| *name == id)?;
    let text = files
        .iter()
        .find(|(path, _)| *path == TEMPLATE_FILE)
        .map(|(_, text)| *text)?;
    let others = files
        .iter()
        .filter(|(path, _)| *path != TEMPLATE_FILE)
        .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
        .collect();
    // The built-ins are checked by `every_builtin_template_scaffolds_and_checks_clean`.
    parse(id.to_owned(), TemplateSource::Builtin, text, others).ok()
}

/// Every built-in template, in [`BUILTIN_ORDER`].
fn builtins() -> Vec<Template> {
    BUILTIN_ORDER.iter().filter_map(|id| builtin(id)).collect()
}

/// The template in `folder`, read now.
pub(crate) fn from_folder(
    folder: &Path,
    id: String,
    source: TemplateSource,
) -> Result<Template, String> {
    let text = fs::read_to_string(folder.join(TEMPLATE_FILE))
        .map_err(|error| format!("{}: {error}", folder.join(TEMPLATE_FILE).display()))?;
    let mut files = Vec::new();
    collect_files(folder, folder, &mut files)?;
    files.sort();
    parse(id, source, &text, files)
}

/// Every text file under `folder` but `template.toml` at the top, by its path relative to
/// `base`; links are not followed, and `.git` and `.DS_Store` are left out.
fn collect_files(
    base: &Path,
    folder: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(base, &path, files)?;
            continue;
        }
        if folder == base && name == TEMPLATE_FILE {
            continue;
        }
        if files.len() >= MAX_FILES {
            return Err(format!(
                "{} holds more than {MAX_FILES} files",
                base.display()
            ));
        }
        let size = entry
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?
            .len();
        if size > MAX_FILE_BYTES {
            return Err(format!("{} is larger than 1 MiB", path.display()));
        }
        let text = fs::read_to_string(&path)
            .map_err(|_| format!("{} is not a text file", path.display()))?;
        let relative = path
            .strip_prefix(base)
            .map_err(|_| format!("{} is outside the template", path.display()))?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        files.push((relative, text));
    }
    Ok(())
}

/// The templates in the subfolders of `folder`, by name, and why each one that holds a
/// `template.toml` but does not read is left out.
fn in_folder(
    folder: &Path,
    prefix: &str,
    source: fn(PathBuf) -> TemplateSource,
) -> (Vec<Template>, Vec<String>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return (Vec::new(), Vec::new());
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| path.join(TEMPLATE_FILE).is_file())
        .collect();
    folders.sort();
    let mut templates = Vec::new();
    let mut problems = Vec::new();
    for path in folders {
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        match from_folder(&path, format!("{prefix}/{name}"), source(path.clone())) {
            Ok(template) => templates.push(template),
            Err(problem) => problems.push(format!("{prefix}/{name}: {problem}")),
        }
    }
    (templates, problems)
}

/// The year now, in UTC, for `{{year}}`.
pub(crate) fn current_year() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    adoc_core::civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0)).0
}

impl Engine {
    /// The templates New Antora Project offers, read now: the built-ins, then the user's,
    /// then the workspace's.
    #[must_use]
    pub fn templates(&self) -> Vec<Template> {
        let mut templates = builtins();
        templates.extend(self.folder_templates().0);
        templates
    }

    /// Why each folder template that does not read is left out of [`Engine::templates`].
    #[must_use]
    pub fn template_problems(&self) -> Vec<String> {
        self.folder_templates().1
    }

    fn folder_templates(&self) -> (Vec<Template>, Vec<String>) {
        let mut templates = Vec::new();
        let mut problems = Vec::new();
        let folders = [
            (
                &self.settings.templates.user_dir,
                "user",
                TemplateSource::User as fn(_) -> _,
            ),
            (
                &self.settings.templates.workspace_dir,
                "workspace",
                TemplateSource::Workspace,
            ),
        ];
        for (folder, prefix, source) in folders {
            if let Some(folder) = folder {
                let (found, failed) = in_folder(folder, prefix, source);
                templates.extend(found);
                problems.extend(failed);
            }
        }
        (templates, problems)
    }

    /// The template `id` names: a built-in, a listed one, or the folder at the path `id`.
    pub fn template(&self, id: &str) -> Result<Template, String> {
        let id = if id.trim().is_empty() {
            DEFAULT_TEMPLATE
        } else {
            id.trim()
        };
        if let Some(template) = builtin(id) {
            return Ok(template);
        }
        let listed = [
            ("user/", &self.settings.templates.user_dir),
            ("workspace/", &self.settings.templates.workspace_dir),
        ];
        for (prefix, folder) in listed {
            if let (Some(name), Some(folder)) = (id.strip_prefix(prefix), folder) {
                let source = if prefix == "user/" {
                    TemplateSource::User
                } else {
                    TemplateSource::Workspace
                };
                if !name.contains(['/', '\\']) && name != ".." && name != "." {
                    let path = folder.join(name);
                    return from_folder(&path, id.to_owned(), source(path.clone()));
                }
            }
        }
        let path = Path::new(id);
        if path.is_absolute() && path.join(TEMPLATE_FILE).is_file() {
            return from_folder(
                path,
                id.to_owned(),
                TemplateSource::User(path.to_path_buf()),
            );
        }
        Err(format!("there is no template `{id}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variables() -> Variables {
        Variables {
            name: "billing".to_owned(),
            title: "Billing".to_owned(),
            version: "2.0".to_owned(),
            index_title: "Welcome".to_owned(),
            year: "2026".to_owned(),
        }
    }

    #[test]
    fn template_substitution_covers_paths_and_contents() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("house");
        fs::create_dir_all(folder.join("modules/ROOT/pages")).unwrap();
        fs::write(
            folder.join(TEMPLATE_FILE),
            "name = \"House\"\n[[component]]\nname = \"{{name}}-docs\"\npath = \"{{name}}\"\n",
        )
        .unwrap();
        fs::write(
            folder.join("modules/ROOT/pages/{{name}}-{{year}}.adoc"),
            "= {{title}} {{version}}\n{{index_title}} {{unknown}} {{ year }} {{year",
        )
        .unwrap();
        let template = from_folder(
            &folder,
            "user/house".to_owned(),
            TemplateSource::User(folder.clone()),
        )
        .unwrap();
        let (path, text) = &template.files[0];
        let vars = variables();
        assert_eq!(
            substitute(path, &vars),
            "modules/ROOT/pages/billing-2026.adoc"
        );
        assert_eq!(
            substitute(text, &vars),
            "= Billing 2.0\nWelcome {{unknown}} {{ year }} {{year"
        );
        let component = &template.components[0];
        assert_eq!(substitute(&component.name, &vars), "billing-docs");
        assert_eq!(substitute(&component.path, &vars), "billing");

        let project = Path::new("/p");
        assert_eq!(
            join_relative(project, "a/b.adoc").unwrap(),
            PathBuf::from("/p/a/b.adoc")
        );
        assert_eq!(join_relative(project, ".").unwrap(), PathBuf::from("/p"));
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "a//b",
            "C:x",
            "a\\..\\b",
            "",
        ] {
            assert!(join_relative(project, bad).is_err(), "{bad}");
        }
        // Nothing may reach a Git repository's own folder, however a file system spells it.
        for bad in [
            ".git/config",
            ".GIT/hooks/post-checkout",
            "docs/.Git/config",
            ".git./config",
            ".git /config",
            "GIT~1/config",
            "git~2",
            ".g\u{200c}it/config",
            ".git\u{feff}/config",
        ] {
            assert!(join_relative(project, bad).is_err(), "{bad:?}");
        }
        for fine in [
            ".gitignore",
            ".github/workflows/docs.yml",
            "git/notes.adoc",
            "Über.adoc",
        ] {
            assert!(join_relative(project, fine).is_ok(), "{fine}");
        }
    }

    #[test]
    fn template_toml_hides_unused_fields() {
        let single = builtin("single-page").unwrap();
        assert!(!single.antora);
        assert!(single.uses(TemplateField::Title));
        assert!(single.uses(TemplateField::Git));
        for unused in [
            TemplateField::Name,
            TemplateField::Version,
            TemplateField::IndexTitle,
            TemplateField::Readme,
            TemplateField::Playbook,
        ] {
            assert!(!single.uses(unused), "{unused:?}");
        }
        let minimal = builtin("minimal").unwrap();
        assert!(TemplateField::ALL.iter().all(|field| minimal.uses(*field)));
        let bad = parse(
            "x".to_owned(),
            TemplateSource::Builtin,
            "name = \"X\"\nfields = [\"colour\"]\n",
            Vec::new(),
        );
        assert!(bad.unwrap_err().contains("`colour` is not a field"));
    }

    #[test]
    fn builtins_read_in_order() {
        let ids: Vec<String> = builtins().into_iter().map(|template| template.id).collect();
        assert_eq!(ids, BUILTIN_ORDER);
        let product = builtin("product-docs").unwrap();
        assert_eq!(product.components[0].modules.len(), 5);
        assert!(product.components[0]
            .attributes
            .contains(&("page-pagination".to_owned(), String::new())));
        assert_eq!(builtin("multi-component").unwrap().components.len(), 2);
    }

    #[test]
    fn templates_refuse_what_they_cannot_make() {
        for (text, expected) in [
            (
                "name = \"X\"\n[[component]]\nmodules = [\"guide\"]\n",
                "ROOT",
            ),
            (
                "name = \"X\"\n[[component]]\nmodules = [\"ROOT\", \"Bad Name\"]\n",
                "module name",
            ),
            ("name = \"X\"\nantora = false\n", "`open`"),
            (
                "name = \"X\"\nantora = false\nopen = \"a.adoc\"\n[[component]]\n",
                "no components",
            ),
            (
                "name = \"X\"\n[[component]]\n[component.attributes]\n\"a b\" = \"1\"\n",
                "attribute name",
            ),
            ("name = \"X\"\nflavour = 1\n", "flavour"),
        ] {
            let error =
                parse("x".to_owned(), TemplateSource::Builtin, text, Vec::new()).unwrap_err();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn folder_templates_follow_no_links_and_hold_text_only() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("t");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join(TEMPLATE_FILE), "name = \"T\"\n").unwrap();
        fs::write(directory.path().join("secret"), "keep out").unwrap();
        std::os::unix::fs::symlink(directory.path().join("secret"), folder.join("link.adoc"))
            .unwrap();
        let template = from_folder(
            &folder,
            "user/t".to_owned(),
            TemplateSource::User(folder.clone()),
        )
        .unwrap();
        assert!(template.files.is_empty(), "{:?}", template.files);
        fs::write(folder.join("logo.png"), [0x89, 0x50, 0xff, 0xfe]).unwrap();
        let error = from_folder(
            &folder,
            "user/t".to_owned(),
            TemplateSource::User(folder.clone()),
        )
        .unwrap_err();
        assert!(error.contains("not a text file"), "{error}");
    }

    #[test]
    fn the_year_is_read_from_the_clock() {
        assert!(current_year() >= 2026);
    }
}
