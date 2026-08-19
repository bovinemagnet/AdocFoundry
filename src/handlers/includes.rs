use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraContext, AntoraResolver, ResourceFamily,
};
use adoc_core::Document;
use adoc_index::{normalize_path, resolve_include_target, WorkspaceIndex};

/// Include chains are shallow in practice; the bound only stops pathological or cyclic input.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Every file whose content is composed into `path`, `path` itself included.
///
/// Asciidoctor resolves anchors against the assembled document, so an anchor declared in an
/// included partial is a legitimate target for a reference in the including page.
#[must_use]
pub fn composed_files(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    path: &Path,
) -> BTreeSet<PathBuf> {
    let mut visited = BTreeSet::new();
    collect(index, antora, context, path, &mut visited, 0);
    visited
}

fn collect(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
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

fn resolve_target(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    current: &Path,
    document: &Document,
    target: &str,
) -> Option<PathBuf> {
    if target.contains('$') {
        let id = parse_resource_id(target).ok()?;
        return AntoraResolver::resolve(antora, &id, context)
            .ok()
            .map(|resource| resource.source_path.clone());
    }

    // Attributes the document declares itself win, exactly as Asciidoctor resolves them.
    if let Some(resolved) = resolve_include_target(document, current, target) {
        return Some(resolved);
    }
    let substituted = substitute_intrinsic_directories(antora, context, target)?;
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
