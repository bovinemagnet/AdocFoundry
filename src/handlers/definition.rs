use std::path::{Path, PathBuf};

use adoc_antora::{parse_resource_id, AntoraCatalog, AntoraResolver, AntoraResource};
use adoc_core::{Document, IncludeDirective, Reference, ReferenceKind, SourceRange};
use adoc_index::{resolve_include_target, WorkspaceIndex};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub range: SourceRange,
}

#[must_use]
pub fn definition_at_offset(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
    offset: usize,
) -> Option<DefinitionTarget> {
    if let Some(reference) = document
        .references
        .iter()
        .find(|reference| contains_offset(reference.range, offset))
    {
        if let Some(target) = resolve_antora_reference(index, antora, current_path, reference) {
            return Some(target);
        }
        return resolve_reference(index, current_path, reference);
    }

    let include = document
        .includes
        .iter()
        .find(|include| contains_offset(include.range, offset))?;
    resolve_antora_include(index, antora, current_path, include)
        .or_else(|| resolve_include(index, current_path, document, include))
}

fn resolve_antora_reference(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    reference: &Reference,
) -> Option<DefinitionTarget> {
    if reference.kind != ReferenceKind::Xref {
        return None;
    }
    let context = antora.context_for_path(current_path)?;
    let (target, anchor) = reference
        .target
        .split_once('#')
        .map_or((reference.target.as_str(), None), |(target, anchor)| {
            (target, Some(anchor))
        });
    let id = parse_resource_id(target).ok()?;
    let resource = AntoraResolver::resolve(antora, &id, &context).ok()?;
    resource_definition(index, resource, anchor)
}

fn resolve_antora_include(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    include: &IncludeDirective,
) -> Option<DefinitionTarget> {
    if !include.target.contains('$') {
        return None;
    }
    let context = antora.context_for_path(current_path)?;
    let id = parse_resource_id(&include.target).ok()?;
    let resource = AntoraResolver::resolve(antora, &id, &context).ok()?;
    resource_definition(index, resource, None)
}

fn resource_definition(
    index: &WorkspaceIndex,
    resource: &AntoraResource,
    anchor: Option<&str>,
) -> Option<DefinitionTarget> {
    if let Some(anchor) = anchor.filter(|anchor| !anchor.is_empty()) {
        let location = index.resolve_anchor(&resource.source_path, anchor)?;
        return Some(DefinitionTarget {
            path: location.path.clone(),
            range: location.range,
        });
    }

    let range = index
        .file(&resource.source_path)
        .and_then(|file| file.document.title.as_ref())
        .map_or(SourceRange::new(0, 0), |title| title.range);
    Some(DefinitionTarget {
        path: resource.source_path.clone(),
        range,
    })
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

#[must_use]
pub fn resolve_include(
    index: &WorkspaceIndex,
    current_path: &Path,
    document: &Document,
    include: &IncludeDirective,
) -> Option<DefinitionTarget> {
    let path = resolve_include_target(document, current_path, &include.target)?;
    if let Some(file) = index.file(&path) {
        return Some(DefinitionTarget {
            path: file.path.clone(),
            range: file
                .document
                .title
                .as_ref()
                .map_or(SourceRange::new(0, 0), |title| title.range),
        });
    }
    path.exists().then(|| DefinitionTarget {
        path,
        range: SourceRange::new(0, 0),
    })
}

fn contains_offset(range: SourceRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_antora::{discover_antora_workspace, AntoraCatalog};
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

    #[test]
    fn resolves_include_at_source_offset() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/includes");
        let path = root.join("index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_roots(&[root]).unwrap();
        let document = &index.file(&path).unwrap().document;
        let offset = document.text.find("partials/intro").unwrap();

        let target =
            super::definition_at_offset(&index, &AntoraCatalog::default(), &path, document, offset)
                .unwrap();

        assert!(target.path.ends_with("partials/intro.adoc"));
    }

    #[test]
    fn resolves_antora_xrefs_and_family_qualified_includes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        let index_path = root.join("modules/ROOT/pages/index.adoc");
        let document = &index.file(&index_path).unwrap().document;
        let offset = document.text.find("security:").unwrap();
        let target =
            super::definition_at_offset(&index, &antora, &index_path, document, offset).unwrap();
        assert!(target
            .path
            .ends_with("modules/security/pages/authentication.adoc"));

        let authentication_path = root.join("modules/security/pages/authentication.adoc");
        let document = &index.file(&authentication_path).unwrap().document;
        let offset = document.text.find("partial$").unwrap();
        let target =
            super::definition_at_offset(&index, &antora, &authentication_path, document, offset)
                .unwrap();
        assert!(target
            .path
            .ends_with("modules/security/partials/token-note.adoc"));
    }
}
