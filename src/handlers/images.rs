//! Where an image macro's target points.

use std::path::{Path, PathBuf};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraResolver, AntoraResourceId, ResourceFamily,
};
use adoc_core::Document;
use adoc_index::{resolve_image_target, WorkspaceIndex};

/// The Antora resource an image target names, defaulting to the `image` family.
///
/// `None` for targets no catalog can judge: URLs, `data:` URIs, and attribute references.
/// The resolver itself defaults a missing family to `page`, so the family is set here.
#[must_use]
pub fn antora_image_id(target: &str) -> Option<AntoraResourceId> {
    if target.contains("://") || target.starts_with("data:") || target.contains('{') {
        return None;
    }
    let mut id = parse_resource_id(target).ok()?;
    id.family.get_or_insert(ResourceFamily::Image);
    Some(id)
}

/// The existing file an image target names, or `None` when it cannot be resolved.
///
/// Inside an Antora module the target is an `image$` resource, resolved exactly as the
/// diagnostics resolve it; elsewhere it is a path under the document's `imagesdir`.
#[must_use]
pub fn resolve_image(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
    target: &str,
) -> Option<PathBuf> {
    match antora.context_for_path(current_path) {
        Some(context) => {
            let id = antora_image_id(target)?;
            AntoraResolver::resolve(antora, &id, &context)
                .ok()
                .map(|resource| resource.source_path.clone())
        }
        None => resolve_image_target(document, current_path, target)
            .filter(|path| path.exists() || index.file(path).is_some()),
    }
}
