//! Antora versions and Git branches: which branch publishes each component version, the
//! version a page belongs to, and the same page in another version.
//!
//! The table of branches comes from the playbook's content sources and each matching branch's
//! `antora.yml`, read from the repository on a worker thread; nothing is fetched or checked out.

use std::{
    path::{Path, PathBuf},
    thread,
};

use adoc_antora::{
    parse_component_descriptor, AntoraCoordinate, BranchVersions, PublishedVersions,
};
use adoc_git::{Head, Repository};
use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError,
};

impl Engine {
    /// The component version of the page at `path`, as `component@version`, with `latest` for an
    /// unversioned component; `None` outside an Antora module.
    #[must_use]
    pub fn version_badge(&self, path: &Path) -> Option<String> {
        let context = self.workspace.antora.context_for_path(path)?;
        let version = context.version.as_deref().unwrap_or("latest");
        Some(format!("{}@{version}", context.component))
    }

    /// The other versions of open document `id`'s component that have the same resource in the
    /// workspace, latest first, with the file each would open: `(version, path)`, the version
    /// `None` for an unversioned component.
    #[must_use]
    pub fn other_versions(&self, id: DocumentId) -> Vec<(Option<String>, PathBuf)> {
        let Some(coordinate) = self.coordinate_of(id) else {
            return Vec::new();
        };
        let antora = &self.workspace.antora;
        antora
            .versions_of(&coordinate.component)
            .into_iter()
            .filter(|descriptor| descriptor.version != coordinate.version)
            .filter_map(|descriptor| {
                let there = AntoraCoordinate {
                    version: descriptor.version.clone(),
                    ..coordinate.clone()
                };
                let resource = antora.resolve(&there)?;
                Some((descriptor.version.clone(), resource.source_path.clone()))
            })
            .collect()
    }

    fn coordinate_of(&self, id: DocumentId) -> Option<AntoraCoordinate> {
        let path = &self.documents.get(&id)?.path;
        self.workspace
            .antora
            .resources()
            .find(|resource| resource.source_path == *path)
            .map(|resource| resource.coordinate.clone())
    }

    /// Recomputes which branch publishes each version on a worker thread, when the workspace
    /// has a playbook, a repository and a branch checked out; otherwise forgets it.
    pub(crate) fn refresh_branch_versions(&mut self) {
        self.git.versions_generation += 1;
        let generation = self.git.versions_generation;
        let found = self
            .workspace
            .antora
            .playbook()
            .cloned()
            .and_then(|playbook| {
                let directory = playbook.path.parent()?.to_path_buf();
                let repository = self.git.repository_for(&directory)?;
                let Head::Branch(current) = &repository.head else {
                    return None;
                };
                Some((
                    playbook,
                    directory,
                    repository.workdir.clone(),
                    current.clone(),
                ))
            });
        let Some((playbook, directory, workdir, current)) = found else {
            self.apply_branch_versions(None);
            return;
        };
        let remote = self.settings.git.remote.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let versions = Repository::discover(&workdir).ok().flatten().map(|repo| {
                let published =
                    published_versions(&repo, &playbook, &directory, &workdir, &current, &remote);
                BranchVersions { current, published }
            });
            let _ = sender.send(WorkerMessage::BranchVersions {
                generation,
                versions,
            });
        });
    }

    pub(crate) fn branch_versions_found(
        &mut self,
        generation: u64,
        versions: Option<BranchVersions>,
    ) {
        if generation == self.git.versions_generation {
            self.apply_branch_versions(versions);
        }
    }

    fn apply_branch_versions(&mut self, versions: Option<BranchVersions>) {
        if self.git.branch_versions == versions {
            return;
        }
        self.git.branch_versions.clone_from(&versions);
        self.workspace.antora.set_branch_versions(versions);
        self.refresh_open_diagnostics(None);
    }

    /// Puts the known branch versions back on a catalog that indexing has just replaced.
    pub(crate) fn restore_branch_versions(&mut self) {
        let versions = self.git.branch_versions.clone();
        self.workspace.antora.set_branch_versions(versions);
    }
}

/// Reads `antora.yml` at every branch the playbook takes from this repository.
fn published_versions(
    repo: &Repository,
    playbook: &adoc_antora::Playbook,
    playbook_directory: &Path,
    workdir: &Path,
    current: &str,
    remote: &str,
) -> PublishedVersions {
    let mut published = PublishedVersions::default();
    let Ok(branches) = repo.branches() else {
        return published;
    };
    let remote_url = repo.remote_url(remote).ok().flatten();
    for source in &playbook.sources {
        let this_repository = if source.is_local() {
            Repository::discover(&normalize_path(&playbook_directory.join(&source.url)))
                .ok()
                .flatten()
                .is_some_and(|found| normalize_path(found.workdir()) == workdir)
        } else {
            remote_url
                .as_deref()
                .is_some_and(|url| same_repository(url, &source.url))
        };
        if !this_repository {
            continue;
        }
        for branch in &branches {
            if !source.takes_branch(&branch.name, Some(current)) {
                continue;
            }
            let revision = match &branch.remote {
                Some(remote) => format!("{remote}/{}", branch.name),
                None => branch.name.clone(),
            };
            for start in &source.start_paths {
                let descriptor = Path::new(start).join("antora.yml");
                let Ok(Some(text)) = repo.file_at(&revision, &descriptor) else {
                    continue;
                };
                if let Ok(component) = parse_component_descriptor(Path::new(start), &text) {
                    published.record(&component.name, component.version.as_deref(), &branch.name);
                }
            }
        }
    }
    published
}

/// Whether two remote URLs name the same repository, whatever their scheme or `.git` suffix:
/// `git@host:org/repo.git` and `https://host/org/repo` do.
fn same_repository(left: &str, right: &str) -> bool {
    fn key(url: &str) -> String {
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
        let without_user = without_scheme
            .split_once('@')
            .map_or(without_scheme, |(_, rest)| rest);
        without_user.replacen(':', "/", 1).to_lowercase()
    }
    key(left) == key(right)
}

pub(crate) fn register_versions(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::NAVIGATE_OPEN_IN_VERSION,
            "Open in Another Version",
            CommandCategory::Navigate,
            None,
            CommandContext::Antora,
        ),
        open_in_version,
    );
}

/// Opens the same resource in another version of its component: `Custom` arguments
/// `{ "document": <id>, "version": "1.0" | null }`, `null` for an unversioned component.
fn open_in_version(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    let invalid = EngineError::InvalidArguments {
        command: ids::NAVIGATE_OPEN_IN_VERSION,
        expected: "a document and a version",
    };
    let CommandArgs::Custom(value) = args else {
        return Err(invalid);
    };
    let Some(id) = value
        .get("document")
        .and_then(serde_json::Value::as_u64)
        .map(DocumentId)
    else {
        return Err(invalid);
    };
    let version = match value.get("version") {
        Some(serde_json::Value::String(version)) => Some(version.clone()),
        Some(serde_json::Value::Null) => None,
        _ => return Err(invalid),
    };
    let path = engine
        .other_versions(id)
        .into_iter()
        .find(|(other, _)| *other == version)
        .map(|(_, path)| path)
        .ok_or_else(|| {
            EngineError::NotFound(
                "that version is not in the workspace: check out its branch in another folder \
                 and add the folder to the workspace"
                    .to_owned(),
            )
        })?;
    let opened = engine.open(&path)?;
    Ok(CommandOutcome::Opened(opened))
}

#[cfg(test)]
mod tests {
    use super::same_repository;

    #[test]
    fn remote_urls_name_the_same_repository_in_any_form() {
        assert!(same_repository(
            "git@github.com:Org/Docs.git",
            "https://github.com/org/docs"
        ));
        assert!(same_repository(
            "https://github.com/org/docs.git/",
            "ssh://git@github.com/org/docs"
        ));
        assert!(!same_repository(
            "https://github.com/org/docs",
            "https://github.com/org/other"
        ));
    }
}
