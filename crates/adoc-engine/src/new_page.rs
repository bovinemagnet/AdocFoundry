//! New Page: `page.new` writes `pages/<slug>.adoc` in a module with the page's title, opens
//! it, and, when asked, adds `* xref:<slug>.adoc[]` to the module's `nav.adoc` after the
//! active page's entry. The file and the entry are one undo group: undoing it takes the entry
//! out and deletes the file, as long as nobody has changed the file since. `page.new_partial`
//! makes the empty file an include names, undone the same way. `file.new` makes an empty file
//! of any kind in a folder of the workspace, and `nav.add_page` lists an existing page at the
//! end of its module's navigation file.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_antora::{AntoraCoordinate, NavItem, ResourceFamily};
use adoc_index::{is_asciidoc_path, normalize_path};

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, TextEdit, UndoGroup,
};

/// What `page.new` makes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewPage {
    /// The root of the module the page goes in (`…/modules/<name>`), as
    /// [`Engine::new_page_modules`] lists it.
    pub module: PathBuf,
    /// The page's title, `= Title`; its file name is made from it.
    pub title: String,
    /// Whether to add the page to the module's navigation file.
    pub add_to_nav: bool,
    /// The page's path in the module's `pages` folder, when it is not named from the title.
    pub file: Option<PathBuf>,
}

/// A module a new page can go in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageModule {
    pub root: PathBuf,
    /// `version@component:module`, or `component:module` for an unversioned component.
    pub label: String,
    /// The module's navigation file, if it has one.
    pub nav: Option<PathBuf>,
    /// Whether the active page is in this module.
    pub active: bool,
}

/// A file a group made, with what it was made with.
#[derive(Clone, Debug)]
pub(crate) struct CreatedFile {
    path: PathBuf,
    content: Arc<[u8]>,
}

impl CreatedFile {
    pub(crate) fn new(path: PathBuf, text: String) -> Self {
        Self::binary(path, text.into_bytes().into())
    }

    pub(crate) fn binary(path: PathBuf, content: Arc<[u8]>) -> Self {
        Self { path, content }
    }
}

/// What a new page makes: its file and text, and the entry its navigation file gains.
pub(crate) struct NewPagePlan {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    pub(crate) nav: Option<(PathBuf, TextEdit)>,
}

pub(crate) fn register_new_page(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::PAGE_NEW,
            "New Page…",
            CommandCategory::Workspace,
            Some("cmd-n"),
            CommandContext::Antora,
        ),
        page_new,
    );
    register(
        registry,
        spec(
            ids::PAGE_NEW_PARTIAL,
            "New Partial",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        page_new_partial,
    );
    register(
        registry,
        spec(
            ids::FILE_NEW,
            "New File…",
            CommandCategory::Workspace,
            None,
            CommandContext::Always,
        ),
        |engine, args| {
            let CommandArgs::Path(path) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::FILE_NEW,
                    expected: "the path of the file to make",
                });
            };
            engine.new_file(&path).map(CommandOutcome::Opened)
        },
    );
    register(
        registry,
        spec(
            ids::NAV_ADD_PAGE,
            "Add to Navigation",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        |engine, args| {
            let CommandArgs::Path(path) = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::NAV_ADD_PAGE,
                    expected: "the page to add",
                });
            };
            engine.add_to_nav(&path).map(|()| CommandOutcome::Done)
        },
    );
}

fn page_new_partial(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Path(path) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::PAGE_NEW_PARTIAL,
            expected: "the path of the file to make",
        });
    };
    engine.new_partial(&path).map(CommandOutcome::Opened)
}

fn page_new(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::NewPage(request) = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::PAGE_NEW,
            expected: "a module, a title and whether to add it to the navigation",
        });
    };
    engine.new_page(&request).map(CommandOutcome::Opened)
}

impl Engine {
    /// The modules of every component version, by component, newest version first, then
    /// module, the one holding the active page marked.
    #[must_use]
    pub fn new_page_modules(&self) -> Vec<PageModule> {
        let active = self
            .active
            .and_then(|id| self.document(id))
            .and_then(|document| self.workspace.antora.context_for_path(&document.path));
        let mut components: Vec<_> = self.workspace.antora.components().collect();
        components.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| adoc_antora::compare_component_versions(right, left))
        });
        components
            .into_iter()
            .flat_map(|component| {
                self.workspace
                    .antora
                    .modules_of(&component.name, component.version.as_deref())
            })
            .map(|module| PageModule {
                root: module.root.clone(),
                label: match &module.version {
                    Some(version) => format!("{version}@{}:{}", module.component, module.name),
                    None => format!("{}:{}", module.component, module.name),
                },
                nav: module.nav.clone(),
                active: active.as_ref().is_some_and(|context| {
                    context.component == module.component
                        && context.version == module.version
                        && context.module == module.name
                }),
            })
            .collect()
    }

    /// Makes the page `request` describes, opens it and, when asked, adds it to its module's
    /// navigation, as one undo group. Refuses a title with nothing to name the file after, a
    /// file that exists already, and a navigation entry for a module without a navigation
    /// file, before changing anything.
    pub fn new_page(&mut self, request: &NewPage) -> Result<crate::DocumentId, EngineError> {
        let after = self
            .active
            .and_then(|id| self.document(id))
            .and_then(|document| self.workspace.antora.resource_for_path(&document.path))
            .map(|resource| resource.coordinate.clone())
            .filter(|coordinate| coordinate.family == ResourceFamily::Page);
        let plan = self.plan_new_page(request, after.as_ref())?;
        let nav_edit = match plan.nav {
            Some((nav, edit)) => Some((self.open(&nav)?, edit)),
            None => None,
        };
        let NewPagePlan { path, text, .. } = plan;
        let (page, group) = self.create_and_open(&path, &text)?;
        let mut documents = vec![page];
        if let Some((nav, edit)) = nav_edit {
            if let Err(error) = self.apply_edits(nav, vec![edit], UndoMode::Group(group)) {
                self.discard_group(group);
                let _ = self.close(page);
                let _ = fs::remove_file(&path);
                self.catalog_changed_at(&path);
                return Err(error);
            }
            documents.push(nav);
        }
        self.group_documents.insert(group, documents);
        self.group_created
            .insert(group, vec![CreatedFile::new(path, text)]);
        self.set_active(page);
        Ok(page)
    }

    /// What `page.new` makes for `request`, made nowhere: the page's path and text, refused
    /// when it exists or the module is not one, and its navigation entry, after the entries of
    /// the page at `after` or else at the navigation's end.
    pub(crate) fn plan_new_page(
        &self,
        request: &NewPage,
        after: Option<&AntoraCoordinate>,
    ) -> Result<NewPagePlan, EngineError> {
        // One line, however the title was typed.
        let title = request
            .title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let file = match &request.file {
            Some(file) => {
                let plain = file
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)));
                if !plain || !is_asciidoc_path(file) || title.is_empty() {
                    return Err(EngineError::InvalidRequest(format!(
                        "{} is not a page in the module's pages folder, with a title",
                        file.display()
                    )));
                }
                file.clone()
            }
            None => {
                let slug = slug(&title);
                if slug.is_empty() {
                    return Err(EngineError::InvalidRequest(
                        "the page needs a title with a letter or digit to name its file after"
                            .to_owned(),
                    ));
                }
                PathBuf::from(format!("{slug}.adoc"))
            }
        };
        let root = normalize_path(&request.module);
        let module = self
            .workspace
            .antora
            .modules()
            .find(|module| module.root == root)
            .cloned()
            .ok_or_else(|| {
                EngineError::InvalidRequest(format!("{} is not a module", root.display()))
            })?;
        let path = normalize_path(&module.root.join("pages").join(&file));
        if path.exists() || self.document_for_path(&path).is_some() {
            return Err(EngineError::InvalidRequest(format!(
                "{} already exists",
                path.display()
            )));
        }
        let nav = match (request.add_to_nav, &module.nav) {
            (false, _) => None,
            (true, Some(nav)) => Some(nav.clone()),
            (true, None) => {
                return Err(EngineError::InvalidRequest(format!(
                    "the module {} has no navigation file",
                    module.name
                )))
            }
        };

        let entry = match &nav {
            Some(nav) => Some((
                nav.clone(),
                self.nav_entry(nav, after, &file.to_string_lossy().replace('\\', "/"))?,
            )),
            None => None,
        };
        Ok(NewPagePlan {
            path,
            text: format!("= {title}\n"),
            nav: entry,
        })
    }

    /// Makes an empty file at `path`, the partial an include names, and opens it, as one undo
    /// group. Refuses a file outside the workspace's roots, one that is not AsciiDoc and one
    /// that exists already, before changing anything.
    pub fn new_partial(&mut self, path: &Path) -> Result<crate::DocumentId, EngineError> {
        let path = normalize_path(path);
        let inside = self.roots().iter().any(|root| path.starts_with(root));
        if !inside
            || !is_asciidoc_path(&path)
            || path.exists()
            || self.document_for_path(&path).is_some()
        {
            return Err(EngineError::InvalidRequest(format!(
                "{} is not a new AsciiDoc file in the workspace",
                path.display()
            )));
        }
        let (partial, group) = self.create_and_open(&path, "")?;
        self.group_documents.insert(group, vec![partial]);
        self.group_created
            .insert(group, vec![CreatedFile::new(path, String::new())]);
        self.set_active(partial);
        Ok(partial)
    }

    /// Makes an empty file at `path` and opens it, as one undo step that takes the file away
    /// again. Refuses a path whose folder does not exist, is outside the workspace or reached
    /// through a link, and a file that exists already.
    pub fn new_file(&mut self, path: &Path) -> Result<crate::DocumentId, EngineError> {
        let path = normalize_path(path);
        let folder = path
            .parent()
            .filter(|folder| folder.is_dir())
            .ok_or_else(|| {
                EngineError::InvalidRequest(format!("{} has no folder to go in", path.display()))
            })?;
        self.listable(folder)?;
        if fs::symlink_metadata(&path).is_ok() || self.document_for_path(&path).is_some() {
            return Err(EngineError::InvalidRequest(format!(
                "{} already exists",
                path.display()
            )));
        }
        let (file, group) = self.create_and_open(&path, "")?;
        self.group_documents.insert(group, vec![file]);
        self.group_created
            .insert(group, vec![CreatedFile::new(path.clone(), String::new())]);
        self.set_active(file);
        self.directories_changed(&[path]);
        Ok(file)
    }

    /// Whether `page` can be added to its module's navigation: a page of a module with a
    /// navigation file that no navigation of its component version lists yet.
    #[must_use]
    pub fn can_add_to_nav(&self, page: &Path) -> bool {
        self.nav_addition(page).is_ok()
    }

    /// Lists `page` at the end of its module's navigation file, as one undo step.
    pub fn add_to_nav(&mut self, page: &Path) -> Result<(), EngineError> {
        let (nav, edit) = self.nav_addition(page)?;
        let id = self.open(&nav)?;
        let group = self.new_group();
        self.apply_edits(id, vec![edit], UndoMode::Group(group))?;
        Ok(())
    }

    /// The navigation file `page` would be added to and the edit adding it.
    fn nav_addition(&self, page: &Path) -> Result<(PathBuf, TextEdit), EngineError> {
        let page = normalize_path(page);
        let coordinate = self
            .workspace
            .antora
            .resource_for_path(&page)
            .filter(|_| is_asciidoc_path(&page))
            .map(|resource| &resource.coordinate)
            .filter(|coordinate| coordinate.family == ResourceFamily::Page)
            .ok_or_else(|| {
                EngineError::InvalidRequest(format!(
                    "{} is not a page of a component",
                    page.display()
                ))
            })?;
        let nav = self
            .workspace
            .antora
            .modules()
            .find(|module| {
                module.component == coordinate.component
                    && module.version == coordinate.version
                    && module.name == coordinate.module
            })
            .and_then(|module| module.nav.clone())
            .ok_or_else(|| {
                EngineError::InvalidRequest(format!(
                    "the module {} has no navigation file",
                    coordinate.module
                ))
            })?;
        let listed = self
            .nav_trees(&coordinate.component, coordinate.version.as_deref())
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))?
            .iter()
            .any(|tree| find_entry(&tree.items, coordinate).is_some());
        if listed {
            return Err(EngineError::InvalidRequest(format!(
                "{} is in the navigation already",
                page.display()
            )));
        }
        let target = coordinate
            .relative_path
            .to_string_lossy()
            .replace('\\', "/");
        let edit = self.nav_entry(&nav, None, &target)?;
        Ok((nav, edit))
    }

    /// Writes `text` to a new file at `path` and opens it in a new undo group, taking the file
    /// away again if it cannot be opened.
    fn create_and_open(
        &mut self,
        path: &Path,
        text: &str,
    ) -> Result<(crate::DocumentId, UndoGroup), EngineError> {
        create_file(path, text)?;
        self.catalog_changed_at(path);
        let group = self.new_group();
        let opened = match self.open(path) {
            Ok(opened) => opened,
            Err(error) => {
                let _ = fs::remove_file(path);
                self.catalog_changed_at(path);
                return Err(error);
            }
        };
        self.push_marker(opened, group);
        Ok((opened, group))
    }

    /// The edit adding an entry for `target` to the navigation file `nav`: after the entry for
    /// the page at `after` and the entries beneath it, at its level, or else at the end.
    fn nav_entry(
        &self,
        nav: &Path,
        after: Option<&AntoraCoordinate>,
        target: &str,
    ) -> Result<TextEdit, EngineError> {
        let text = self.current_text(nav).unwrap_or_default();
        let trees = self
            .nav_trees_of(&[nav.to_path_buf()])
            .map_err(|error| EngineError::InvalidRequest(error.to_string()))?;
        let entry =
            after.and_then(|page| trees.iter().find_map(|tree| find_entry(&tree.items, page)));
        let edit = match entry {
            Some(entry) => {
                let line = text
                    .get(entry.source.range.start..entry.source.range.end)
                    .unwrap_or_default();
                let level = line
                    .trim_start()
                    .chars()
                    .take_while(|&character| character == '*')
                    .count()
                    .max(1);
                let end = subtree_end(entry);
                TextEdit::new(
                    adoc_core::SourceRange::new(end, end),
                    format!("\n{} xref:{target}[]", "*".repeat(level)),
                )
            }
            None => {
                let end = text.len();
                let before = if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                TextEdit::new(
                    adoc_core::SourceRange::new(end, end),
                    format!("{before}* xref:{target}[]\n"),
                )
            }
        };
        Ok(edit)
    }

    /// Takes back (with `undone`) or makes again the files `group` made. A file is deleted
    /// only while it holds what the group wrote and its page has no unsaved edits; a file
    /// made again is opened, leaving the active tab as it was.
    pub(crate) fn replay_creations(&mut self, group: UndoGroup, undone: bool) {
        let Some(created) = self.group_created.get(&group).cloned() else {
            return;
        };
        for file in created {
            let open = self.document_for_path(&file.path);
            if undone {
                let edited = open
                    .and_then(|id| self.document(id))
                    .is_some_and(|document| document.dirty);
                let unchanged = fs::read(&file.path).is_ok_and(|bytes| *bytes == *file.content);
                if edited || !unchanged || fs::remove_file(&file.path).is_err() {
                    continue;
                }
                if let Some(id) = open {
                    let _ = self.close(id);
                }
                self.workspace.remove_document(&file.path);
                self.graph_file_changed(&file.path);
                self.catalog_changed_at(&file.path);
            } else if !file.path.exists() && create_file(&file.path, &file.content).is_ok() {
                self.catalog_changed_at(&file.path);
                let active = self.active_item();
                // A module's `.gitkeep` is made again, not opened.
                if is_asciidoc_path(&file.path) && self.open(&file.path).is_ok() {
                    if let Some(active) = active {
                        self.activate(active);
                    }
                }
            }
        }
    }

    /// Re-reads the descriptor of the component holding `path`, a page made or deleted.
    pub(crate) fn catalog_changed_at(&mut self, path: &Path) {
        let descriptors: Vec<PathBuf> = self
            .workspace
            .antora
            .components()
            .filter(|component| path.starts_with(&component.root))
            .map(|component| component.root.join("antora.yml"))
            .collect();
        for descriptor in descriptors {
            self.reload_descriptor(&descriptor);
        }
    }
}

/// Writes `content` to a new file at `path`, refusing one that exists.
pub(crate) fn create_file(path: &Path, content: impl AsRef<[u8]>) -> Result<(), EngineError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| EngineError::io(parent, error))?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(content.as_ref()))
        .map_err(|error| EngineError::io(path, error))
}

/// The first entry linking to `page` itself, not a fragment of it, else the first linking to
/// it at all.
fn find_entry<'a>(items: &'a [NavItem], page: &AntoraCoordinate) -> Option<&'a NavItem> {
    let mut entries = Vec::new();
    collect_entries(items, page, &mut entries);
    entries
        .iter()
        .find(|entry| entry.fragment.is_none())
        .or_else(|| entries.first())
        .copied()
}

fn collect_entries<'a>(items: &'a [NavItem], page: &AntoraCoordinate, into: &mut Vec<&'a NavItem>) {
    for item in items {
        let links = item.target.as_ref().is_some_and(|target| {
            target.component.as_deref() == Some(page.component.as_str())
                && target.version == page.version
                && target.module.as_deref() == Some(page.module.as_str())
                && Path::new(&target.path) == page.relative_path
        });
        if links {
            into.push(item);
        }
        collect_entries(&item.children, page, into);
    }
}

/// The end of the line of `entry`'s last descendant, or of its own line.
fn subtree_end(entry: &NavItem) -> usize {
    entry
        .children
        .last()
        .map_or(entry.source.range.end, subtree_end)
}

/// A file name for a page called `title`: its letters and digits, lower case, with a hyphen
/// for each run of anything else. Apostrophes join the letters around them.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if character.is_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if !matches!(character, '\'' | '’') && !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs_keep_letters_and_digits_and_hyphenate_the_rest() {
        assert_eq!(slug("Release Notes"), "release-notes");
        assert_eq!(slug("  Getting Started: Part 2 "), "getting-started-part-2");
        assert_eq!(slug("What's New"), "whats-new");
        assert_eq!(slug("Café — Ordering"), "café-ordering");
        assert_eq!(slug("C++ & Rust"), "c-rust");
        assert_eq!(slug("?!"), "");
        assert_eq!(slug(""), "");
    }
}
