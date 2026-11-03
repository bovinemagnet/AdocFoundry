//! New Module (#290): `module.new` makes `modules/<name>/` in a component version, with the
//! families chosen, an index page and a navigation file listed in `antora.yml`, as one undo
//! step, the way `page.new` makes a page one level down.
//!
//! The descriptor's only change is one line inserted into its `nav:` list (or the key added),
//! a text edit through the document's undo history, saved at once so the catalog, which reads
//! descriptors from disk, sees the module; undo and redo save and reload it again. The YAML is
//! never rewritten through a serialiser: a `nav:` that is not a plain block list is refused.

use std::{
    fs,
    path::{Path, PathBuf},
};

use adoc_antora::ResourceFamily;
use adoc_core::SourceRange;
use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    new_page::{create_file, CreatedFile},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, TextEdit, UndoGroup,
};

/// What `module.new` makes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewModule {
    /// The component version's root, the folder holding `antora.yml`, as
    /// [`Engine::new_module_components`] lists it.
    pub component: PathBuf,
    /// `[a-z0-9][a-z0-9._-]*`, or `ROOT` when the component version has no `ROOT` module.
    pub name: String,
    /// The families to make; `pages` is always made.
    pub families: Vec<ResourceFamily>,
    /// Make `nav.adoc` and list it in `antora.yml`'s `nav:`.
    pub nav: bool,
    /// Make `pages/index.adoc` with this title; `None` makes no page.
    pub index_title: Option<String>,
}

impl NewModule {
    /// The families made when none are chosen: pages, partials and images.
    pub const DEFAULT_FAMILIES: [ResourceFamily; 3] = [
        ResourceFamily::Page,
        ResourceFamily::Partial,
        ResourceFamily::Image,
    ];
}

/// A component version a new module can go in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleComponent {
    /// The folder holding `antora.yml`.
    pub root: PathBuf,
    /// `version@component`, or `component` for the versionless version.
    pub label: String,
    pub existing_modules: Vec<String>,
    /// Whether `antora.yml` has a `nav:` list.
    pub has_nav_list: bool,
    /// Whether the active page is in it.
    pub active: bool,
}

/// What a group's module needs on undo and redo: its descriptor, saved and reloaded, and the
/// folders it made, removed again while empty.
#[derive(Clone, Debug)]
pub(crate) struct MadeModule {
    descriptor: Option<PathBuf>,
    folders: Vec<PathBuf>,
}

pub(crate) fn register_new_module(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::MODULE_NEW,
            "New Module…",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        module_new,
    );
}

fn module_new(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::NewModule(request) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::MODULE_NEW,
            expected: "a component, a module name, families and whether to add navigation",
        });
    };
    Ok(match engine.new_module(&request)? {
        Some(index) => CommandOutcome::Opened(index),
        None => CommandOutcome::Done,
    })
}

/// Whether `name` may name a module: lower-case letters, digits, `.`, `_` and `-`, starting
/// with a letter or digit, or `ROOT`.
#[must_use]
pub fn valid_module_name(name: &str) -> bool {
    if name == "ROOT" {
        return true;
    }
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
}

/// The files a module in `folder` holds when made, with their text, and the folders made for
/// them, the module's own first: the families (`pages` always) with a `.gitkeep` while empty,
/// an index page titled `title`, and with `nav` a navigation file listing it.
pub(crate) fn module_files(
    folder: &Path,
    families: &[ResourceFamily],
    nav: bool,
    title: Option<&str>,
) -> (Vec<(PathBuf, String)>, Vec<PathBuf>) {
    let mut chosen: Vec<ResourceFamily> = vec![ResourceFamily::Page];
    for family in families {
        if !chosen.contains(family) {
            chosen.push(*family);
        }
    }
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    if let Some(title) = title {
        files.push((
            folder.join("pages/index.adoc"),
            format!("= {title}\n\nAn introduction to {title}.\n"),
        ));
    }
    if nav {
        let text = if title.is_some() {
            "* xref:index.adoc[]\n"
        } else {
            ""
        };
        files.push((folder.join("nav.adoc"), text.to_owned()));
    }
    // Git keeps no empty folder, and Antora ignores a missing family's, so an empty family
    // holds a `.gitkeep`.
    for family in &chosen {
        if *family == ResourceFamily::Page && title.is_some() {
            continue;
        }
        files.push((
            folder.join(family.directory()).join(".gitkeep"),
            String::new(),
        ));
    }
    let mut folders = vec![folder.to_path_buf()];
    folders.extend(chosen.iter().map(|family| folder.join(family.directory())));
    (files, folders)
}

/// `name` as a title: words split at `-`, `_` and `.`, each capitalised.
pub(crate) fn title_case(name: &str) -> String {
    name.split(['-', '_', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The edit adding `entry` to the `nav:` list of the descriptor `text`: a line after its last
/// item, with that item's indentation and quoting, or the key and one item at the end when
/// there is no `nav:`. Refused when `nav:` holds something other than a block list.
pub(crate) fn nav_list_edit(text: &str, entry: &str) -> Result<TextEdit, String> {
    let lines: Vec<(usize, &str)> = text
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            let at = *offset;
            *offset += line.len();
            Some((at, line))
        })
        .collect();
    let key = lines.iter().position(|(_, line)| {
        let line = line.trim_end();
        line == "nav:" || line.starts_with("nav:") && !line.starts_with("nav:-")
    });
    let Some(key) = key else {
        let before = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let end = text.len();
        return Ok(TextEdit::new(
            SourceRange::new(end, end),
            format!("{before}nav:\n- {entry}\n"),
        ));
    };
    let after_key = lines[key].1.trim_end()["nav:".len()..].trim();
    if !after_key.is_empty() && !after_key.starts_with('#') {
        return Err(format!(
            "antora.yml's `nav:` is `{after_key}`, not a block list of navigation files; add \
             the module's navigation file to it by hand"
        ));
    }
    // The list's items: lines starting `-` (indented or not) until the next key, skipping
    // blank lines and comments.
    let mut last_item: Option<usize> = None;
    for (number, (_, line)) in lines.iter().enumerate().skip(key + 1) {
        let trimmed = line.trim_start();
        if trimmed.trim_end().is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.trim_end() == "-" {
            last_item = Some(number);
            continue;
        }
        if line.starts_with([' ', '\t']) && last_item.is_some() {
            return Err(
                "antora.yml's `nav:` list has an item over several lines; add the module's \
                 navigation file to it by hand"
                    .to_owned(),
            );
        }
        break;
    }
    let (at, template) = match last_item {
        Some(number) => {
            let (start, line) = lines[number];
            (start + line.len(), Some(line))
        }
        None => {
            let (start, line) = lines[key];
            (start + line.len(), None)
        }
    };
    let (indent, quote) = match template {
        Some(line) => {
            let indent = &line[..line.len() - line.trim_start().len()];
            let value = line.trim_start()[1..].trim();
            let quote = value
                .chars()
                .next()
                .filter(|first| matches!(first, '\'' | '"'))
                .map(String::from)
                .unwrap_or_default();
            (indent.to_owned(), quote)
        }
        None => (String::new(), String::new()),
    };
    let ends_line = text[..at].ends_with('\n') || at == 0;
    let before = if ends_line { "" } else { "\n" };
    Ok(TextEdit::new(
        SourceRange::new(at, at),
        format!("{before}{indent}- {quote}{entry}{quote}\n"),
    ))
}

impl Engine {
    /// The component versions a module can be made in, by name and then latest version first.
    #[must_use]
    pub fn new_module_components(&self) -> Vec<ModuleComponent> {
        let active = self
            .active
            .and_then(|id| self.document(id))
            .map(|document| document.path.clone());
        let mut components: Vec<_> = self.workspace.antora.components().collect();
        components.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| adoc_antora::compare_component_versions(right, left))
        });
        components
            .into_iter()
            .map(|component| ModuleComponent {
                root: normalize_path(&component.root),
                label: match &component.version {
                    Some(version) => format!("{version}@{}", component.name),
                    None => component.name.clone(),
                },
                existing_modules: self
                    .workspace
                    .antora
                    .modules_of(&component.name, component.version.as_deref())
                    .map(|module| module.name.clone())
                    .collect(),
                has_nav_list: !component.nav.is_empty(),
                active: active
                    .as_deref()
                    .is_some_and(|path| path.starts_with(&component.root)),
            })
            .collect()
    }

    /// Why `name` cannot be a new module of the component version at `component`, or the
    /// module's folder when it can; checked before anything is written, and as a dialog types.
    pub fn check_new_module(&self, component: &Path, name: &str) -> Result<PathBuf, String> {
        let root = normalize_path(component);
        let descriptor = self
            .workspace
            .antora
            .components()
            .find(|descriptor| normalize_path(&descriptor.root) == root)
            .ok_or_else(|| format!("{} is not a component version", root.display()))?;
        if !valid_module_name(name) {
            return Err(format!(
                "`{name}` is not a module name: use lower-case letters, digits, `.`, `_` and \
                 `-`, starting with a letter or digit"
            ));
        }
        let existing: Vec<String> = self
            .workspace
            .antora
            .modules_of(&descriptor.name, descriptor.version.as_deref())
            .map(|module| module.name.clone())
            .collect();
        if existing.iter().any(|module| module == name) {
            return Err(format!("the module `{name}` exists already"));
        }
        let folder = root.join("modules").join(name);
        if folder.exists() {
            return Err(format!("{} exists already", folder.display()));
        }
        Ok(folder)
    }

    /// Makes the module `request` asks for as one undo group, and opens its index page when it
    /// has one (returned). Everything is checked before anything is written.
    pub fn new_module(&mut self, request: &NewModule) -> Result<Option<DocumentId>, EngineError> {
        let root = normalize_path(&request.component);
        let folder = self
            .check_new_module(&root, &request.name)
            .map_err(EngineError::InvalidRequest)?;
        let descriptor = root.join("antora.yml");
        let descriptor_edit = if request.nav {
            let text = self.current_text(&descriptor).ok_or_else(|| {
                EngineError::InvalidRequest(format!("{} cannot be read", descriptor.display()))
            })?;
            Some(
                nav_list_edit(&text, &format!("modules/{}/nav.adoc", request.name))
                    .map_err(EngineError::InvalidRequest)?,
            )
        } else {
            None
        };

        let title = request.index_title.as_deref().map(str::trim).map(|title| {
            if title.is_empty() {
                title_case(&request.name)
            } else {
                title.to_owned()
            }
        });
        let (files, folders) =
            module_files(&folder, &request.families, request.nav, title.as_deref());

        self.applying = true;
        let made = self.make_module(&root, &descriptor, descriptor_edit, &files);
        self.applying = false;
        let (group, documents, index) = match made {
            Ok(made) => made,
            Err(error) => {
                for folder in folders.iter().rev() {
                    let _ = fs::remove_dir(folder);
                }
                self.catalog_changed_at(&descriptor);
                return Err(error);
            }
        };
        self.group_documents.insert(group, documents);
        self.group_created.insert(
            group,
            files
                .into_iter()
                .map(|(path, text)| CreatedFile::new(path, text))
                .collect(),
        );
        self.group_modules.insert(
            group,
            MadeModule {
                descriptor: request.nav.then(|| descriptor.clone()),
                folders,
            },
        );
        self.refresh_open_diagnostics(None);
        self.audit_changed_everywhere();
        if let Some(index) = index {
            let _ = self.open(&self.documents[&index].path.clone());
            self.set_active(index);
        }
        Ok(index)
    }

    /// Writes the module's files and the descriptor's line in one new group; on failure the
    /// files written are removed and the group discarded.
    fn make_module(
        &mut self,
        root: &Path,
        descriptor: &Path,
        descriptor_edit: Option<TextEdit>,
        files: &[(PathBuf, String)],
    ) -> Result<(UndoGroup, Vec<DocumentId>, Option<DocumentId>), EngineError> {
        let group = self.new_group();
        let mut written: Vec<PathBuf> = Vec::new();
        let result = (|| {
            for (path, text) in files {
                create_file(path, text)?;
                written.push(path.clone());
            }
            if let Some(edit) = descriptor_edit {
                let id = self.open_unshown(descriptor)?;
                self.apply_edits(id, vec![edit], UndoMode::Group(group))?;
                self.save(id)?;
            }
            self.catalog_changed_at(&root.join("antora.yml"));
            Ok(())
        })();
        if let Err(error) = result {
            self.discard_group(group);
            for path in written.iter().rev() {
                let _ = fs::remove_file(path);
            }
            if let Some(id) = self.document_for_path(descriptor) {
                let _ = self.save(id);
            }
            return Err(error);
        }
        let mut documents: Vec<DocumentId> =
            self.document_for_path(descriptor).into_iter().collect();
        let mut index = None;
        for (path, _) in files {
            if !adoc_index::is_asciidoc_path(path) {
                continue;
            }
            let id = self.open_unshown(path)?;
            self.push_marker(id, group);
            documents.push(id);
            if path.ends_with("pages/index.adoc") {
                index = Some(id);
            }
        }
        Ok((group, documents, index))
    }

    /// After `group`'s edits and files are undone (`undone`) or redone: its descriptor saved
    /// and reloaded, and on undo its folders removed while empty.
    pub(crate) fn replay_module(&mut self, group: UndoGroup, undone: bool) {
        let Some(made) = self.group_modules.get(&group).cloned() else {
            return;
        };
        if let Some(descriptor) = &made.descriptor {
            if let Some(id) = self.document_for_path(descriptor) {
                let _ = self.save(id);
            }
        }
        if undone {
            for folder in made.folders.iter().rev() {
                let _ = fs::remove_dir(folder);
            }
        }
        if let Some(descriptor) = made.descriptor.clone().or_else(|| {
            made.folders.first().and_then(|folder| {
                folder
                    .parent()?
                    .parent()
                    .map(|root| root.join("antora.yml"))
            })
        }) {
            self.catalog_changed_at(&descriptor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(text: &str) -> Result<String, String> {
        let edit = nav_list_edit(text, "modules/security/nav.adoc")?;
        let mut out = text.to_owned();
        out.replace_range(edit.range.start..edit.range.end, &edit.replacement);
        Ok(out)
    }

    #[test]
    fn module_names_follow_antora() {
        for good in ["security", "admin-guide", "v2.api", "a_b", "0", "ROOT"] {
            assert!(valid_module_name(good), "{good}");
        }
        for bad in ["", "Security", "-x", ".x", "a b", "a/b", "Root", "ünïcode"] {
            assert!(!valid_module_name(bad), "{bad}");
        }
        assert_eq!(title_case("admin-guide_v2"), "Admin Guide V2");
    }

    #[test]
    fn a_navigation_file_joins_the_list_as_its_items_are_written() {
        assert_eq!(
            edited("name: demo\nnav:\n  - modules/ROOT/nav.adoc\nasciidoc: {}\n").unwrap(),
            "name: demo\nnav:\n  - modules/ROOT/nav.adoc\n  - modules/security/nav.adoc\nasciidoc: {}\n"
        );
        assert_eq!(
            edited("nav:\n- 'modules/ROOT/nav.adoc'\n").unwrap(),
            "nav:\n- 'modules/ROOT/nav.adoc'\n- 'modules/security/nav.adoc'\n"
        );
        assert_eq!(
            edited("nav:\n  - \"a.adoc\"\n\n  # a comment\n  - \"b.adoc\"").unwrap(),
            "nav:\n  - \"a.adoc\"\n\n  # a comment\n  - \"b.adoc\"\n  - \"modules/security/nav.adoc\"\n"
        );
        assert_eq!(
            edited("name: demo\nversion: ~\n").unwrap(),
            "name: demo\nversion: ~\nnav:\n- modules/security/nav.adoc\n"
        );
        assert_eq!(
            edited("name: demo\nnav:\nversion: ~\n").unwrap(),
            "name: demo\nnav:\n- modules/security/nav.adoc\nversion: ~\n"
        );
        assert!(edited("nav: [modules/ROOT/nav.adoc]\n").is_err());
        assert!(edited("nav: modules/ROOT/nav.adoc\n").is_err());
    }
}
