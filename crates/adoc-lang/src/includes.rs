use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraContext, AntoraResolver, ResourceFamily,
};
use adoc_core::Document;
use adoc_index::{normalize_path, resolve_include_target, WorkspaceIndex};

/// Include chains are shallow in practice; the bound only stops pathological or cyclic input.
pub(crate) const MAX_INCLUDE_DEPTH: usize = 16;

/// Every file whose content is composed into `path`, `path` itself included.
///
/// Asciidoctor resolves anchors against the assembled document, so an anchor declared in an
/// included partial is a legitimate target for a reference in the including page. Without
/// an Antora `context`, only relative and attribute-based include targets are followed.
#[must_use]
pub fn composed_files(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: Option<&AntoraContext>,
    path: &Path,
) -> BTreeSet<PathBuf> {
    let mut visited = BTreeSet::new();
    collect(index, antora, context, path, &mut visited, 0);
    visited
}

fn collect(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: Option<&AntoraContext>,
    path: &Path,
    visited: &mut BTreeSet<PathBuf>,
    depth: usize,
) {
    let path = normalize_path(path);
    if depth > MAX_INCLUDE_DEPTH || !visited.insert(path.clone()) {
        return;
    }
    let Some(file) = index.file(&path) else {
        return;
    };

    for include in &file.document.includes {
        if let Some(target) = resolve_target(
            index,
            antora,
            context,
            &path,
            &file.document,
            &include.target,
        ) {
            collect(index, antora, context, &target, visited, depth + 1);
        }
    }
}

/// Where each include in the files composed into a page leads, keyed the way a parser running
/// from the page asks for it: the including file's requested path's directory joined with the
/// target as written, normalised. Nested includes are keyed beneath their parent's requested
/// path, not its real one, because the parser only knows the former.
///
/// `document` is the page's current parse, which may be newer than the index.
#[must_use]
pub fn include_map(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    path: &Path,
    document: &Document,
) -> BTreeMap<PathBuf, PathBuf> {
    let context = antora.context_for_path(path);
    let mut map = BTreeMap::new();
    let mut visited = BTreeSet::new();
    visited.insert(normalize_path(path));
    map_includes(
        index,
        antora,
        context.as_ref(),
        (path, path),
        document,
        &mut map,
        &mut visited,
        0,
    );
    map
}

#[allow(clippy::too_many_arguments)]
fn map_includes(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: Option<&AntoraContext>,
    (requested, real): (&Path, &Path),
    document: &Document,
    map: &mut BTreeMap<PathBuf, PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    let directory = requested.parent().unwrap_or_else(|| Path::new(""));
    for include in &document.includes {
        let Some(target) = resolve_target(index, antora, context, real, document, &include.target)
        else {
            continue;
        };
        let key = normalize_path(&directory.join(&include.target));
        map.insert(key.clone(), target.clone());
        if !visited.insert(target.clone()) {
            continue;
        }
        if let Some(file) = index.file(&target) {
            map_includes(
                index,
                antora,
                context,
                (&key, &target),
                &file.document,
                map,
                visited,
                depth + 1,
            );
        }
    }
}

fn resolve_target(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: Option<&AntoraContext>,
    current: &Path,
    document: &Document,
    target: &str,
) -> Option<PathBuf> {
    if target.contains('$') {
        let id = parse_resource_id(target).ok()?;
        return AntoraResolver::resolve(antora, &id, context?)
            .ok()
            .map(|resource| resource.source_path.clone());
    }

    // Attributes the document declares itself win, exactly as Asciidoctor resolves them.
    if let Some(resolved) = resolve_include_target(document, current, target) {
        return Some(resolved);
    }
    let substituted = substitute_intrinsic_directories(antora, context?, target)?;
    let resolved = normalize_path(&substituted);
    index
        .file(&resolved)
        .map(|file| file.path.clone())
        .or_else(|| resolved.exists().then_some(resolved))
}

/// Antora supplies `partialsdir` and friends implicitly, so documents reference them without
/// ever declaring them.
fn substitute_intrinsic_directories(
    antora: &AntoraCatalog,
    context: &AntoraContext,
    target: &str,
) -> Option<PathBuf> {
    let module = antora.module(
        &context.component,
        context.version.as_deref(),
        &context.module,
    )?;
    let mut substituted = target.to_owned();
    substituted = substituted.replace("{moduledir}", &module.root.to_string_lossy());
    for family in ResourceFamily::ALL {
        let placeholder = match family {
            ResourceFamily::Page => continue,
            ResourceFamily::Partial => "{partialsdir}",
            ResourceFamily::Example => "{examplesdir}",
            ResourceFamily::Image => "{imagesdir}",
            ResourceFamily::Attachment => "{attachmentsdir}",
        };
        substituted = substituted.replace(
            placeholder,
            &module.root.join(family.directory()).to_string_lossy(),
        );
    }

    (substituted != target && !substituted.contains('{')).then(|| PathBuf::from(substituted))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_antora::discover_antora_workspace;
    use adoc_index::WorkspaceIndex;

    use super::include_map;

    #[test]
    fn include_map_keys_nested_includes_by_requested_path() {
        let root = adoc_index::normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/antora-nested-includes"),
        );
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;
        let pages = root.join("modules/ROOT/pages");
        let partials = root.join("modules/ROOT/partials");
        let page = pages.join("outer.adoc");
        let document = &index.file(&page).unwrap().document;

        let map = include_map(&index, &antora, &page, document);
        // Both levels are asked for from the page's directory: the parser does not know that
        // `partial$level-one.adoc` really lives in `partials/`.
        assert_eq!(
            map.get(&pages.join("partial$level-one.adoc")),
            Some(&partials.join("level-one.adoc"))
        );
        assert_eq!(
            map.get(&pages.join("partial$level-two.adoc")),
            Some(&partials.join("level-two.adoc"))
        );
        assert_eq!(map.len(), 2);

        let cyclic = pages.join("cyclic.adoc");
        let document = &index.file(&cyclic).unwrap().document;
        assert_eq!(include_map(&index, &antora, &cyclic, document).len(), 2);
    }
}
