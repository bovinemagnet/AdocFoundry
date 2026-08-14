use std::path::{Path, PathBuf};

use adoc_core::{Reference, ReferenceKind, SourceRange};
use adoc_index::WorkspaceIndex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub range: SourceRange,
}

#[must_use]
pub fn resolve_reference(
    index: &WorkspaceIndex,
    current_path: &Path,
    reference: &Reference,
) -> Option<DefinitionTarget> {
    let (target_path, anchor) = match reference.kind {
        ReferenceKind::LocalAnchor => (current_path.to_path_buf(), Some(reference.target.as_str())),
        ReferenceKind::Xref => {
            let (file, anchor) = reference
                .target
                .split_once('#')
                .map_or((reference.target.as_str(), None), |(file, anchor)| {
                    (file, Some(anchor))
                });
            let path = if file.is_empty() {
                current_path.to_path_buf()
            } else {
                current_path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(file)
            };
            (path, anchor.filter(|anchor| !anchor.is_empty()))
        }
        ReferenceKind::Link | ReferenceKind::Attribute => return None,
    };

    if let Some(anchor) = anchor {
        let location = index.resolve_anchor(&target_path, anchor.trim_start_matches('#'))?;
        return Some(DefinitionTarget {
            path: location.path.clone(),
            range: location.range,
        });
    }

    let file = index.file(&target_path)?;
    Some(DefinitionTarget {
        path: file.path.clone(),
        range: file
            .document
            .title
            .as_ref()
            .map_or(SourceRange::new(0, 0), |title| title.range),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_index::WorkspaceIndex;

    use super::resolve_reference;

    #[test]
    fn resolves_file_and_local_anchor_references() {
        let mut index = WorkspaceIndex::new();
        index.index_source(
            "docs/index.adoc",
            "= Home\n\nSee xref:other.adoc#details[].\n",
        );
        index.index_source("docs/other.adoc", "= Other\n\n[[details]]\n== Details\n");
        let reference = &index
            .file(Path::new("docs/index.adoc"))
            .unwrap()
            .document
            .references[0];

        let target = resolve_reference(&index, Path::new("docs/index.adoc"), reference).unwrap();

        assert_eq!(target.path, Path::new("docs/other.adoc"));
        assert!(!target.range.is_empty());
    }
}
