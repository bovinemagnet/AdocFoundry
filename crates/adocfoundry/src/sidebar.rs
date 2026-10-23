//! Sidebar trees built from engine state: the Antora navigation, the module and family browser,
//! the workspace diagnostics, the files changed since the last commit, and the outline of the
//! active document.
//!
//! Every leaf's id encodes where clicking it leads, so one handler serves all of them.

use std::path::{Path, PathBuf};

use adoc_core::{LineIndex, SourceRange};
use adoc_engine::{DocumentId, DocumentSymbol, Engine, GitChange, NavItem, WorkspaceState};
use gpui_component::tree::TreeItem;

/// Where a tree item leads when clicked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub path: PathBuf,
    pub range: SourceRange,
}

/// Encodes a target as a tree item id, unique within one tree.
pub fn target_id(path: &Path, range: SourceRange, salt: usize) -> String {
    format!("{}|{}|{}|{}", range.start, range.end, salt, path.display())
}

/// Decodes an id made by [`target_id`]; folder ids do not decode.
pub fn parse_target(id: &str) -> Option<Target> {
    let mut parts = id.splitn(4, '|');
    let start = parts.next()?.parse().ok()?;
    let end = parts.next()?.parse().ok()?;
    let _salt = parts.next()?;
    let path = PathBuf::from(parts.next()?);
    Some(Target {
        path,
        range: SourceRange::new(start, end),
    })
}

fn folder(id: impl Into<String>, label: impl Into<String>, children: Vec<TreeItem>) -> TreeItem {
    TreeItem::new(format!("folder:{}", id.into()), label.into())
        .children(children)
        .expanded(true)
}

fn component_label(name: &str, title: Option<&str>, version: Option<&str>) -> String {
    let title = title.unwrap_or(name);
    match version {
        Some(version) => format!("{title} {version}"),
        None => title.to_owned(),
    }
}

/// The page a navigation entry links to, as a file to open.
fn nav_target(workspace: &WorkspaceState, item: &NavItem) -> Option<PathBuf> {
    let id = item.target.as_ref()?;
    let coordinate = adoc_antora_coordinate(id)?;
    workspace
        .antora
        .resolve(&coordinate)
        .map(|resource| resource.source_path.clone())
}

fn adoc_antora_coordinate(
    id: &adoc_engine::AntoraResourceId,
) -> Option<adoc_engine::AntoraCoordinate> {
    Some(adoc_engine::AntoraCoordinate {
        component: id.component.clone()?,
        version: id.version.clone(),
        module: id.module.clone()?,
        family: id.family?,
        relative_path: PathBuf::from(&id.path),
    })
}

fn nav_items(workspace: &WorkspaceState, items: &[NavItem], salt: &mut usize) -> Vec<TreeItem> {
    items
        .iter()
        .map(|item| {
            *salt += 1;
            let children = nav_items(workspace, &item.children, salt);
            // A linked entry opens its page; anything else opens its line in the nav file.
            let (path, range) = match nav_target(workspace, item) {
                Some(path) => (path, SourceRange::new(0, 0)),
                None => (item.source.path.clone(), item.source.range),
            };
            let tree_item = TreeItem::new(target_id(&path, range, *salt), item.title.clone());
            if children.is_empty() {
                tree_item
            } else {
                tree_item.children(children).expanded(true)
            }
        })
        .collect()
}

/// The navigation of every component version, falling back to its pages when it declares none.
pub fn navigation(engine: &Engine) -> Vec<TreeItem> {
    let workspace = engine.workspace();
    let mut salt = 0;
    let mut components: Vec<_> = workspace.antora.components().collect();
    components.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| adoc_engine::compare_component_versions(right, left))
    });
    components
        .into_iter()
        .map(|component| {
            let label = component_label(
                &component.name,
                component.title.as_deref(),
                component.version.as_deref(),
            );
            let id = format!(
                "{}@{}",
                component.name,
                component.version.as_deref().unwrap_or("~")
            );
            let trees = engine
                .nav_trees(&component.name, component.version.as_deref())
                .unwrap_or_default();
            let children = if trees.is_empty() {
                pages(
                    workspace,
                    &component.name,
                    component.version.as_deref(),
                    &mut salt,
                )
            } else {
                trees
                    .iter()
                    .flat_map(|tree| nav_items(workspace, &tree.items, &mut salt))
                    .collect()
            };
            folder(id, label, children)
        })
        .collect()
}

fn pages(
    workspace: &WorkspaceState,
    component: &str,
    version: Option<&str>,
    salt: &mut usize,
) -> Vec<TreeItem> {
    workspace
        .antora
        .modules_of(component, version)
        .map(|module| {
            let children = workspace
                .antora
                .resources_in(
                    component,
                    version,
                    &module.name,
                    adoc_engine::ResourceFamily::Page,
                )
                .map(|resource| {
                    *salt += 1;
                    TreeItem::new(
                        target_id(&resource.source_path, SourceRange::new(0, 0), *salt),
                        resource.coordinate.relative_path.display().to_string(),
                    )
                })
                .collect();
            folder(
                format!("{component}@{}:{}", version.unwrap_or("~"), module.name),
                module.name.clone(),
                children,
            )
        })
        .collect()
}

/// Every module of every component version, with its families and their resources.
pub fn families(engine: &Engine) -> Vec<TreeItem> {
    let workspace = engine.workspace();
    let mut salt = 0;
    let mut components: Vec<_> = workspace.antora.components().collect();
    components.sort_by(|left, right| left.name.cmp(&right.name));
    components
        .into_iter()
        .map(|component| {
            let version = component.version.as_deref();
            let modules = workspace
                .antora
                .modules_of(&component.name, version)
                .map(|module| {
                    let families = adoc_engine::ResourceFamily::ALL
                        .into_iter()
                        .filter_map(|family| {
                            let resources: Vec<TreeItem> = workspace
                                .antora
                                .resources_in(&component.name, version, &module.name, family)
                                .map(|resource| {
                                    salt += 1;
                                    TreeItem::new(
                                        target_id(
                                            &resource.source_path,
                                            SourceRange::new(0, 0),
                                            salt,
                                        ),
                                        resource.coordinate.relative_path.display().to_string(),
                                    )
                                })
                                .collect();
                            (!resources.is_empty()).then(|| {
                                folder(
                                    format!("{}:{}:{family}", component.name, module.name),
                                    format!("{family} ({})", resources.len()),
                                    resources,
                                )
                            })
                        })
                        .collect();
                    folder(
                        format!(
                            "{}@{}:{}",
                            component.name,
                            version.unwrap_or("~"),
                            module.name
                        ),
                        module.name.clone(),
                        families,
                    )
                })
                .collect();
            folder(
                format!("{}@{}", component.name, version.unwrap_or("~")),
                component_label(&component.name, component.title.as_deref(), version),
                modules,
            )
        })
        .collect()
}

/// Workspace diagnostics grouped by file, each opening its location.
pub fn problems(engine: &Engine, roots: &[PathBuf]) -> (Vec<TreeItem>, usize) {
    let mut count = 0;
    let mut salt = 0;
    let items = engine
        .workspace_diagnostics()
        .into_iter()
        .map(|(path, diagnostics)| {
            let text = engine
                .document_for_path(&path)
                .and_then(|id| engine.document(id))
                .map(|document| document.text.to_string())
                .or_else(|| std::fs::read_to_string(&path).ok())
                .unwrap_or_default();
            let lines = LineIndex::new(&text);
            let children = diagnostics
                .iter()
                .map(|diagnostic| {
                    count += 1;
                    salt += 1;
                    let (line, column) = lines.line_column(diagnostic.range.start);
                    TreeItem::new(
                        target_id(&path, diagnostic.range, salt),
                        format!("{}:{} {}", line + 1, column + 1, diagnostic.message),
                    )
                })
                .collect();
            folder(
                path.display().to_string(),
                display_path(&path, roots),
                children,
            )
        })
        .collect();
    (items, count)
}

/// The Antora family directories whose files the Changes panel lists whatever their type.
const FAMILY_DIRECTORIES: &[&str] = &["images", "attachments", "examples", "partials"];

/// The changed files under the workspace roots that a writer edits: AsciiDoc pages and the
/// files of Antora's resource families, grouped by repository when there is more than one. A
/// leaf opens its file at the first change; a deleted file's leaf leads nowhere.
pub fn changes(engine: &Engine, roots: &[PathBuf]) -> Vec<TreeItem> {
    let git = engine.git();
    let repositories = git.repositories();
    let rows = |workdir: Option<&Path>| -> Vec<TreeItem> {
        git.statuses()
            .filter(|(path, status)| {
                roots.iter().any(|root| path.starts_with(root))
                    && workdir.is_none_or(|workdir| path.starts_with(workdir))
                    && status.worktree != GitChange::Ignored
                    && is_writer_file(path)
            })
            .map(|(path, status)| {
                let hunks = engine
                    .document_for_path(path)
                    .map_or(0, |id| git.hunks_for(id).len());
                let mut label = format!("{} {}", short_status(status), display_path(path, roots));
                if hunks > 0 {
                    label.push_str(&format!(" ({hunks})"));
                }
                let deleted = matches!(status.worktree, GitChange::Deleted)
                    || matches!(status.index, GitChange::Deleted);
                let id = if deleted {
                    format!("deleted:{}", path.display())
                } else {
                    change_id(path)
                };
                TreeItem::new(id, label)
            })
            .collect()
    };
    if repositories.len() <= 1 {
        return rows(None);
    }
    repositories
        .iter()
        .map(|repository| {
            let name = repository.workdir.file_name().map_or_else(
                || repository.workdir.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            folder(
                repository.workdir.display().to_string(),
                name,
                rows(Some(&repository.workdir)),
            )
        })
        .collect()
}

/// The id of a Changes leaf, which opens `path` at its first change.
pub fn change_id(path: &Path) -> String {
    format!("change:{}", path.display())
}

/// The path a Changes leaf opens, if `id` is one.
pub fn parse_change(id: &str) -> Option<PathBuf> {
    id.strip_prefix("change:").map(PathBuf::from)
}

fn is_writer_file(path: &Path) -> bool {
    adoc_index::is_asciidoc_path(path)
        || path.components().any(|component| {
            FAMILY_DIRECTORIES
                .iter()
                .any(|family| component.as_os_str() == *family)
        })
}

/// The two letters `git status --short` shows for a file.
fn short_status(status: &adoc_engine::FileStatus) -> String {
    let letter = |change: &GitChange| match change {
        GitChange::Unmodified => ' ',
        GitChange::Added => 'A',
        GitChange::Modified => 'M',
        GitChange::Deleted => 'D',
        GitChange::Renamed(_) => 'R',
        GitChange::Untracked => '?',
        GitChange::Ignored => '!',
        GitChange::Conflicted => 'U',
    };
    match status.worktree {
        GitChange::Untracked => "??".to_owned(),
        _ => format!("{}{}", letter(&status.index), letter(&status.worktree)),
    }
}

/// The outline of document `id`.
pub fn outline(engine: &Engine, id: DocumentId) -> Vec<TreeItem> {
    let Some(path) = engine.document(id).map(|document| document.path.clone()) else {
        return Vec::new();
    };
    let mut salt = 0;
    symbols(&engine.outline(id), &path, &mut salt)
}

fn symbols(symbols: &[DocumentSymbol], path: &Path, salt: &mut usize) -> Vec<TreeItem> {
    symbols
        .iter()
        .map(|symbol| {
            *salt += 1;
            let item = TreeItem::new(
                target_id(path, symbol.selection_range, *salt),
                symbol.name.clone(),
            );
            let children = self::symbols(&symbol.children, path, salt);
            if children.is_empty() {
                item
            } else {
                item.children(children).expanded(true)
            }
        })
        .collect()
}

/// `path` relative to the workspace root that contains it.
pub fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ids_round_trip() {
        let path = Path::new("/docs/a|b/page.adoc");
        let id = target_id(path, SourceRange::new(3, 9), 7);
        assert_eq!(
            parse_target(&id),
            Some(Target {
                path: path.to_path_buf(),
                range: SourceRange::new(3, 9),
            })
        );
        assert_eq!(parse_target("folder:demo@1.0"), None);
    }
}
