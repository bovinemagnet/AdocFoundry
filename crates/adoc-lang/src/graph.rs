//! One file's links, for the engine's link graph: each xref and include resolved to the file it
//! reaches, or kept as an unresolved target.

use std::path::{Path, PathBuf};

use adoc_antora::{parse_resource_id, ResolutionError, ResourceFamily};
use adoc_core::{Document, ReferenceKind, SourceRange};
use adoc_index::{is_asciidoc_path, normalize_path, resolve_include_target};

use crate::{definition::reference_target_path, WorkspaceState};

/// What a link is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LinkKind {
    Xref,
    Include,
    Nav,
}

/// Where a link leads.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LinkTarget {
    /// A normalised path.
    File(PathBuf),
    /// A target that does not resolve, as written, without its fragment.
    Unresolved(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub kind: LinkKind,
    pub target: LinkTarget,
    /// The xref or include directive.
    pub range: SourceRange,
}

/// The xrefs and includes of the indexed file at `path`, in source order, each resolved as
/// following it would. A link to the file itself, an anchor-only xref, a dynamic target
/// (`include::{attr}/x.adoc[]`), an xref into a component or version published elsewhere and
/// an include that is not AsciiDoc make no link. A conditional include that does not resolve
/// makes none either, as it is left out of the diagnostics.
#[must_use]
pub fn links_from(ws: &WorkspaceState, path: &Path) -> Vec<Link> {
    let path = normalize_path(path);
    let Some(file) = ws.index.file(&path) else {
        return Vec::new();
    };
    let mut links = Vec::new();
    xrefs(ws, &path, &file.document, &mut links);
    includes(ws, &path, &file.document, &mut links);
    links.retain(|link| link.target != LinkTarget::File(path.clone()));
    links.sort_by_key(|link| link.range);
    links
}

fn xrefs(ws: &WorkspaceState, path: &Path, document: &Document, links: &mut Vec<Link>) {
    // A page often names one target many times: each is resolved once.
    let mut resolved: std::collections::BTreeMap<&str, Option<LinkTarget>> =
        std::collections::BTreeMap::new();
    for reference in &document.references {
        if reference.kind != ReferenceKind::Xref {
            continue;
        }
        let target = reference
            .target
            .split_once('#')
            .map_or(reference.target.as_str(), |(file, _)| file)
            .trim();
        if target.is_empty() || is_dynamic(target) || target.contains("://") {
            continue;
        }
        let target = resolved.entry(target).or_insert_with(|| {
            match reference_target_path(&ws.index, &ws.antora, path, target) {
                Some(found) => Some(LinkTarget::File(normalize_path(&found))),
                None if published_elsewhere(ws, path, target) => None,
                None => Some(LinkTarget::Unresolved(target.to_owned())),
            }
        });
        let Some(target) = target.clone() else {
            continue;
        };
        links.push(Link {
            kind: LinkKind::Xref,
            target,
            range: reference.range,
        });
    }
}

fn includes(ws: &WorkspaceState, path: &Path, document: &Document, links: &mut Vec<Link>) {
    for include in &document.includes {
        let target = include.target.trim();
        if target.is_empty() || target.contains("://") {
            continue;
        }
        // As `includes_of` resolves them: relative and attribute targets, then resource ids.
        let resolved = resolve_include_target(document, path, target)
            .filter(|found| ws.index.file(found).is_some() || found.is_file())
            .or_else(|| reference_target_path(&ws.index, &ws.antora, path, target));
        let target = match resolved {
            Some(found) if is_asciidoc_path(&found) => LinkTarget::File(normalize_path(&found)),
            Some(_) => continue,
            None if include.conditional
                || is_dynamic(target)
                || published_elsewhere(ws, path, target) =>
            {
                continue
            }
            None if is_asciidoc_path(Path::new(resource_path(target))) => {
                LinkTarget::Unresolved(target.to_owned())
            }
            None => continue,
        };
        links.push(Link {
            kind: LinkKind::Include,
            target,
            range: include.range,
        });
    }
}

/// An Antora target naming a component or version missing from the workspace: published from
/// elsewhere in the playbook, so not a broken link.
fn published_elsewhere(ws: &WorkspaceState, path: &Path, target: &str) -> bool {
    let (Some(context), Ok(id)) = (ws.antora.context_for_path(path), parse_resource_id(target))
    else {
        return false;
    };
    matches!(
        ws.antora.resolve_detailed(&id, &context),
        Err(ResolutionError::UnknownComponent { .. } | ResolutionError::UnknownVersion { .. })
    )
}

/// Where a page a link expects, but the workspace lacks, would go.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingPage {
    /// The root of the module (`…/modules/<name>`).
    pub module: PathBuf,
    /// The page's path in the module's `pages` folder.
    pub file: PathBuf,
}

/// The page the xref `target` from `from` names, when it is an AsciiDoc page in a module the
/// workspace has and no file holds it. `None` for a target that resolves, another family, a
/// path leaving its folder, and a link outside Antora.
#[must_use]
pub fn missing_page(ws: &WorkspaceState, from: &Path, target: &str) -> Option<MissingPage> {
    let context = ws.antora.context_for_path(from)?;
    let id = parse_resource_id(target).ok()?;
    let Err(ResolutionError::UnknownResource {
        component,
        version,
        module,
        family: ResourceFamily::Page,
        path,
    }) = ws.antora.resolve_detailed(&id, &context)
    else {
        return None;
    };
    let plain = path
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !plain || !is_asciidoc_path(&path) {
        return None;
    }
    let module = ws.antora.module(&component, version.as_deref(), &module)?;
    Some(MissingPage {
        module: module.root.clone(),
        file: path,
    })
}

/// The file the unresolved include `target` in `from` names: for a resource id
/// (`partial$x.adoc`, `admin:partial$x.adoc`), the file in its family's folder of a module the
/// workspace has; else the path relative to `from`'s folder. `None` for a target that
/// resolves, a dynamic or remote one, one that is not AsciiDoc, and a resource id naming a
/// component, version or module the workspace lacks or a path leaving its folder.
#[must_use]
pub fn missing_include(ws: &WorkspaceState, from: &Path, target: &str) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty()
        || is_dynamic(target)
        || target.contains("://")
        || !is_asciidoc_path(Path::new(resource_path(target)))
    {
        return None;
    }
    if !target.contains('$') {
        let path = normalize_path(&from.parent()?.join(target));
        return (ws.index.file(&path).is_none() && !path.exists()).then_some(path);
    }
    let context = ws.antora.context_for_path(from)?;
    let id = parse_resource_id(target).ok()?;
    let Err(ResolutionError::UnknownResource {
        component,
        version,
        module,
        family,
        path,
    }) = ws.antora.resolve_detailed(&id, &context)
    else {
        return None;
    };
    let plain = path
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    let module = ws.antora.module(&component, version.as_deref(), &module)?;
    let directory = family.directories().first()?;
    plain.then(|| normalize_path(&module.root.join(directory).join(path)))
}

/// The path part of a target, after any resource id's coordinates.
fn resource_path(target: &str) -> &str {
    let after_family = target.rsplit_once('$').map_or(target, |(_, path)| path);
    after_family
        .rsplit_once(':')
        .map_or(after_family, |(_, path)| path)
}

fn is_dynamic(target: &str) -> bool {
    target.contains('{') || target.contains('}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/graph")
                .join(name),
        )
    }

    fn workspace() -> WorkspaceState {
        let mut ws = WorkspaceState::new();
        ws.index_roots(&[fixture("")]).unwrap();
        ws
    }

    fn targets(ws: &WorkspaceState, page: &str) -> Vec<(LinkKind, String)> {
        let root = fixture("");
        links_from(ws, &fixture(page))
            .into_iter()
            .map(|link| {
                let target = match link.target {
                    LinkTarget::File(path) => path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    LinkTarget::Unresolved(text) => format!("?{text}"),
                };
                (link.kind, target)
            })
            .collect()
    }

    #[test]
    fn links_from_resolves_xrefs_and_includes() {
        let ws = workspace();
        let found = targets(&ws, "guide/modules/ROOT/pages/index.adoc");
        let expected = [
            (LinkKind::Xref, "guide/modules/ROOT/pages/install.adoc"),
            (LinkKind::Xref, "guide/modules/admin/pages/users.adoc"),
            (LinkKind::Xref, "reference/modules/ROOT/pages/api.adoc"),
            // Through the `page-aliases` of `install.adoc`.
            (LinkKind::Xref, "guide/modules/ROOT/pages/install.adoc"),
            (LinkKind::Include, "guide/modules/ROOT/partials/note.adoc"),
        ];
        for (kind, path) in expected {
            assert!(
                found.contains(&(kind, path.to_owned())),
                "{kind:?} {path} in {found:?}"
            );
        }
        // A relative include outside any module resolves too.
        let plain = targets(&ws, "loose/chapter.adoc");
        assert_eq!(plain, [(LinkKind::Include, "loose/_part.adoc".to_owned())]);
    }

    #[test]
    fn links_from_keeps_unresolved_and_skips_dynamic() {
        let ws = workspace();
        let found = targets(&ws, "guide/modules/ROOT/pages/install.adoc");
        assert!(found.contains(&(LinkKind::Xref, "?missing.adoc".to_owned())));
        // The dynamic include and the conditional one that does not resolve make no link,
        // nor does an xref into a component published elsewhere.
        assert!(found
            .iter()
            .all(|(_, target)| !target.contains("{") && !target.contains("optional")));
        assert!(found
            .iter()
            .all(|(_, target)| !target.contains("elsewhere")));

        let mut half_typed = WorkspaceState::new();
        let page = Path::new("/docs/page.adoc");
        half_typed.index.index_source(
            page,
            "= Page\n\nxref:[] and xref:#top[] and xref:: and include::[]\ninclude::{\n",
        );
        assert!(links_from(&half_typed, page).is_empty());
    }

    #[test]
    fn missing_include_names_the_file_an_include_expects() {
        let ws = workspace();
        let install = fixture("guide/modules/ROOT/pages/install.adoc");
        assert_eq!(
            missing_include(&ws, &install, "partial$tips.adoc"),
            Some(fixture("guide/modules/ROOT/partials/tips.adoc"))
        );
        assert_eq!(
            missing_include(&ws, &install, "admin:example$setup/run.adoc"),
            Some(fixture("guide/modules/admin/examples/setup/run.adoc"))
        );
        // Without a family, an include is relative to its page, in Antora or not.
        assert_eq!(
            missing_include(&ws, &install, "../snippets/run.adoc"),
            Some(fixture("guide/modules/ROOT/snippets/run.adoc"))
        );
        assert_eq!(
            missing_include(&ws, &fixture("loose/chapter.adoc"), "parts/_gone.adoc"),
            Some(fixture("loose/parts/_gone.adoc"))
        );
        for target in [
            "partial$note.adoc",
            "_part.adoc",
            "nowhere:partial$gone.adoc",
            "partial$../gone.adoc",
            "{snippets}/gone.adoc",
            "partial$gone.json",
            "https://example.com/gone.adoc",
        ] {
            let from = if target == "_part.adoc" {
                fixture("loose/chapter.adoc")
            } else {
                install.clone()
            };
            assert_eq!(missing_include(&ws, &from, target), None, "{target}");
        }
    }

    #[test]
    fn missing_page_names_the_file_an_xref_expects() {
        let ws = workspace();
        let install = fixture("guide/modules/ROOT/pages/install.adoc");
        assert_eq!(
            missing_page(&ws, &install, "missing.adoc"),
            Some(MissingPage {
                module: fixture("guide/modules/ROOT"),
                file: PathBuf::from("missing.adoc"),
            })
        );
        assert_eq!(
            missing_page(&ws, &install, "admin:setup/roles.adoc"),
            Some(MissingPage {
                module: fixture("guide/modules/admin"),
                file: PathBuf::from("setup/roles.adoc"),
            })
        );
        // A page that exists, another family, a module or component the workspace lacks, a
        // file that is not AsciiDoc, a path leaving its folder, and a page outside Antora.
        for target in [
            "configure.adoc",
            "partial$gone.adoc",
            "nowhere:gone.adoc",
            "elsewhere::gone.adoc",
            "gone.pdf",
            "../gone.adoc",
        ] {
            assert_eq!(missing_page(&ws, &install, target), None, "{target}");
        }
        assert_eq!(
            missing_page(&ws, &fixture("loose/chapter.adoc"), "gone.adoc"),
            None
        );
    }
}
