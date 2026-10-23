use std::{
    io,
    path::{Path, PathBuf},
};

use adoc_antora::{
    discover_antora_workspace, read_component_descriptor, AntoraCatalog, DescriptorError,
};
use adoc_core::Document;
use adoc_index::{normalize_path, WorkspaceIndex};

/// The semantic state of a workspace: the document index, the Antora catalog and any problems
/// found in component descriptors. Front ends keep their own open-document bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceState {
    pub index: WorkspaceIndex,
    pub antora: AntoraCatalog,
    pub descriptor_issues: Vec<DescriptorError>,
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
        Ok(indexed)
    }

    pub fn replace_document(&mut self, path: PathBuf, document: Document) {
        self.index.replace(path, document);
    }

    pub fn remove_document(&mut self, path: &Path) {
        self.index.remove(path);
    }

    /// Re-reads one `antora.yml`. On success, any issue previously recorded for it is cleared;
    /// on failure, the issue is recorded and returned.
    pub fn reload_descriptor(&mut self, descriptor_path: &Path) -> Result<(), DescriptorError> {
        let descriptor_path = normalize_path(descriptor_path);
        self.descriptor_issues
            .retain(|issue| normalize_path(&issue.path) != descriptor_path);
        let root = descriptor_path.parent().unwrap_or_else(|| Path::new(""));
        match read_component_descriptor(root) {
            Ok(_) => {
                // Module and resource discovery is per component root; rebuild the catalog
                // from every component root currently known plus this one.
                let mut roots: Vec<PathBuf> = self
                    .antora
                    .components()
                    .map(|component| component.root.clone())
                    .collect();
                roots.push(root.to_path_buf());
                roots.sort();
                roots.dedup();
                if let Ok(discovery) = discover_antora_workspace(&roots) {
                    self.antora = discovery.catalog;
                    for issue in discovery.issues {
                        if !self.descriptor_issues.contains(&issue) {
                            self.descriptor_issues.push(issue);
                        }
                    }
                }
                Ok(())
            }
            Err(error) => {
                self.descriptor_issues.push(error.clone());
                Err(error)
            }
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
    }
}
