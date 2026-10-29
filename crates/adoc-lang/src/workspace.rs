use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use adoc_antora::{
    check_component_descriptor, discover_antora_workspace, AntoraCatalog, DescriptorError,
    ReloadReport,
};
use adoc_core::{Diagnostic, Document};
use adoc_index::{normalize_path, WorkspaceIndex};

/// The semantic state of a workspace: the document index, the Antora catalog and any problems
/// found in component descriptors. Front ends keep their own open-document bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceState {
    pub index: WorkspaceIndex,
    pub antora: AntoraCatalog,
    pub descriptor_issues: Vec<DescriptorError>,
    /// Problems inside each readable `antora.yml`, by its path, at the lines they are on.
    pub descriptor_problems: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

impl WorkspaceState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes every AsciiDoc file beneath `roots` and discovers the Antora components in them,
    /// replacing any previous catalog. Returns the number of files indexed.
    pub fn index_roots(&mut self, roots: &[PathBuf]) -> io::Result<usize> {
        let indexed = self.index.index_roots(roots)?;
        let discovery = discover_antora_workspace(roots)?;
        self.antora = discovery.catalog;
        self.descriptor_issues = discovery.issues;
        let descriptors: Vec<PathBuf> = self
            .antora
            .components()
            .map(|component| component.root.join("antora.yml"))
            .chain(
                self.descriptor_issues
                    .iter()
                    .map(|issue| issue.path.clone()),
            )
            .collect();
        self.descriptor_problems.clear();
        for descriptor in descriptors {
            self.check_descriptor(&descriptor);
        }
        Ok(indexed)
    }

    pub fn replace_document(&mut self, path: PathBuf, document: Document) {
        self.index.replace(path, document);
    }

    pub fn remove_document(&mut self, path: &Path) {
        self.index.remove(path);
    }

    /// Re-reads one `antora.yml`, replacing that component version in the catalog. On success
    /// any issue previously recorded for it is cleared; on failure the issue is recorded, the
    /// component is removed and the error returned.
    pub fn reload_descriptor(
        &mut self,
        descriptor_path: &Path,
    ) -> Result<ReloadReport, DescriptorError> {
        let descriptor_path = normalize_path(descriptor_path);
        self.descriptor_issues
            .retain(|issue| normalize_path(&issue.path) != descriptor_path);
        self.check_descriptor(&descriptor_path);
        self.antora
            .reload_component(&descriptor_path)
            .inspect_err(|error| self.descriptor_issues.push(error.clone()))
    }

    /// Re-reads one `antora.yml` for the problems inside it, forgetting them when it is gone.
    fn check_descriptor(&mut self, descriptor_path: &Path) {
        let descriptor_path = normalize_path(descriptor_path);
        self.descriptor_problems.remove(&descriptor_path);
        let (Ok(source), Some(root)) = (
            fs::read_to_string(&descriptor_path),
            descriptor_path.parent(),
        ) else {
            return;
        };
        let problems = check_component_descriptor(root, &source);
        if !problems.is_empty() {
            self.descriptor_problems.insert(descriptor_path, problems);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_antora_workspace_metadata() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut state = WorkspaceState::new();
        let files = state.index_roots(&[root]).unwrap();
        assert!(files > 0);
        assert!(state.descriptor_issues.is_empty());
        assert!(state.antora.component("demo", Some("latest")).is_some());
        assert!(state.descriptor_problems.is_empty());
    }
}
