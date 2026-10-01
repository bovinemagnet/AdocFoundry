//! Where an image macro's target points inside an Antora module.

use adoc_antora::{parse_resource_id, AntoraResourceId, ResourceFamily};

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
