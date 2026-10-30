//! The xref that links one page to another, or to a section of it, as written when a page is
//! dragged into an editor.

use std::path::{Component, Path};

use adoc_antora::{parse_resource_id, AntoraCatalog, ResourceFamily};
use adoc_core::{asciidoctor_id, canonical_id, Document, Section, SourceRange};
use adoc_index::{is_asciidoc_path, normalize_path};

use crate::WorkspaceState;

/// A link to a page or section: its target and the text to show.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XrefLink {
    /// A resource id or path with any `#fragment`, or a bare id for a section of the same page.
    pub target: String,
    pub text: Option<String>,
}

impl XrefLink {
    /// The link as AsciiDoc: `xref:target[text]`, or `<<id,text>>` for a section of the same
    /// page.
    #[must_use]
    pub fn source(&self) -> String {
        let text = self.text.as_deref().unwrap_or_default();
        if self.target.contains(['.', '$', ':', '/', '#']) {
            format!("xref:{}[{}]", self.target, text.replace(']', "\\]"))
        } else if text.is_empty() {
            format!("<<{}>>", self.target)
        } else {
            format!("<<{},{text}>>", self.target)
        }
    }
}

/// The link from the page at `from` to the page at `to`, whose parsed text is `target`, or to
/// its section whose title is at `section`. In Antora the page is named by the shortest
/// resource id that resolves to it from `from`; elsewhere by its path relative to `from`.
/// `None` when `to` is not a page, or is `from` itself without a section.
#[must_use]
pub fn xref_to(
    ws: &WorkspaceState,
    from: &Path,
    to: &Path,
    target: &Document,
    section: Option<SourceRange>,
) -> Option<XrefLink> {
    let from = normalize_path(from);
    let to = normalize_path(to);
    let section = section.and_then(|at| {
        target
            .sections
            .iter()
            .find(|section| section.selection_range == at)
    });
    let page_title = || target.title.as_ref().map(|title| title.text.clone());
    let Some(section) = section else {
        if from == to {
            return None;
        }
        return Some(XrefLink {
            target: page_id(ws, &from, &to)?,
            text: page_title(),
        });
    };
    let id = section_id(&ws.antora, &to, section);
    let target = if from == to {
        id
    } else {
        format!("{}#{id}", page_id(ws, &from, &to)?)
    };
    Some(XrefLink {
        target,
        text: Some(section.title.clone()),
    })
}

/// The id a reference to `section` of the page at `path` uses: its anchor, else the id the
/// page's build generates, Antora's inside a module and plain Asciidoctor's elsewhere.
pub(crate) fn section_id(antora: &AntoraCatalog, path: &Path, section: &Section) -> String {
    section.id.clone().unwrap_or_else(|| {
        if antora.context_for_path(path).is_some() {
            canonical_id(&section.title)
        } else {
            asciidoctor_id(&section.title)
        }
    })
}

/// How `from` names the page at `to`.
fn page_id(ws: &WorkspaceState, from: &Path, to: &Path) -> Option<String> {
    let Some(context) = ws.antora.context_for_path(from) else {
        let directory = from.parent()?;
        return is_asciidoc_path(to).then(|| relative_path(directory, to));
    };
    let coordinate = &ws
        .antora
        .resource_for_path(to)
        .filter(|resource| resource.coordinate.family == ResourceFamily::Page)?
        .coordinate;
    let path = coordinate
        .relative_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let (component, module) = (&coordinate.component, &coordinate.module);
    let mut candidates = vec![
        path.clone(),
        format!("{module}:{path}"),
        format!("{component}:{module}:{path}"),
    ];
    if let Some(version) = &coordinate.version {
        candidates.push(format!("{version}@{component}:{module}:{path}"));
    }
    if module == "ROOT" {
        let rooted: Vec<String> = candidates
            .iter()
            .filter_map(|id| id.strip_suffix(&format!("{component}:ROOT:{path}")))
            .map(|scope| format!("{scope}{component}::{path}"))
            .collect();
        candidates.extend(rooted);
    }
    // The shortest that reaches the page itself, not another page or the page by an alias.
    candidates
        .into_iter()
        .filter(|id| {
            parse_resource_id(id)
                .ok()
                .and_then(|id| ws.antora.resolve_detailed(&id, &context).ok())
                .is_some_and(|resolved| !resolved.via_alias && resolved.resource.source_path == to)
        })
        .min_by_key(String::len)
}

/// `to` relative to the directory `from`, with `/` separators.
fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_owned(), from.len() - common).collect();
    parts.extend(
        to[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn workspace(fixture: &str) -> (WorkspaceState, PathBuf) {
        let root = adoc_index::normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures")
                .join(fixture),
        );
        let mut ws = WorkspaceState::new();
        ws.index_roots(std::slice::from_ref(&root)).unwrap();
        (ws, root)
    }

    fn link(ws: &WorkspaceState, from: &Path, to: &Path) -> Option<XrefLink> {
        let target = ws.index.file(to)?.document.clone();
        xref_to(ws, from, to, &target, None)
    }

    fn xref(target: &str, text: &str) -> Option<XrefLink> {
        Some(XrefLink {
            target: target.to_owned(),
            text: Some(text.to_owned()),
        })
    }

    #[test]
    fn a_page_in_the_same_module_is_named_by_its_path() {
        let (ws, root) = workspace("antora-multi-component");
        let pages = root.join("alpha/modules/ROOT/pages");
        assert_eq!(
            link(&ws, &pages.join("index.adoc"), &pages.join("guide.adoc")),
            xref("guide.adoc", "Guide")
        );
    }

    #[test]
    fn a_page_in_another_component_is_named_by_component_and_module() {
        let (ws, root) = workspace("antora-multi-component");
        let from = root.join("alpha/modules/ROOT/pages/guide.adoc");
        assert_eq!(
            link(&ws, &from, &root.join("beta/modules/ROOT/pages/index.adoc")),
            xref("beta::index.adoc", "Beta")
        );
        assert_eq!(
            link(
                &ws,
                &from,
                &root.join("beta/modules/api/pages/reference.adoc")
            ),
            xref("beta:api:reference.adoc", "API Reference")
        );
        // From another module of the same component, the module is enough.
        let reference = root.join("beta/modules/api/pages/reference.adoc");
        assert_eq!(
            link(
                &ws,
                &reference,
                &root.join("beta/modules/ROOT/pages/index.adoc")
            ),
            xref("ROOT:index.adoc", "Beta")
        );
    }

    #[test]
    fn a_page_in_a_version_that_is_not_the_latest_names_its_version() {
        let (ws, root) = workspace("antora-versions");
        // The versionless `main` is the latest, which a component without a version means.
        assert_eq!(ws.antora.latest_version("product").unwrap().version, None);
        let target = |from: &str, to: &str| {
            let page = |version: &str, name: &str| {
                root.join(version)
                    .join("modules/ROOT/pages")
                    .join(format!("{name}.adoc"))
            };
            link(&ws, &page(from, "index"), &page(to, "feature")).map(|link| link.target)
        };
        assert_eq!(target("v1", "v1"), Some("feature.adoc".to_owned()));
        assert_eq!(
            target("v1", "main"),
            Some("product::feature.adoc".to_owned())
        );
        assert_eq!(
            target("main", "v1"),
            Some("1.0@product::feature.adoc".to_owned())
        );
        assert_eq!(
            target("v1", "v2"),
            Some("2.0@product::feature.adoc".to_owned())
        );
    }

    #[test]
    fn a_section_adds_its_id_and_title() {
        let (ws, root) = workspace("antora-multi-component");
        let pages = root.join("alpha/modules/ROOT/pages");
        let guide = pages.join("guide.adoc");
        let document = ws.index.file(&guide).unwrap().document.clone();
        let setup = document.sections[0].selection_range;
        assert_eq!(
            xref_to(
                &ws,
                &pages.join("index.adoc"),
                &guide,
                &document,
                Some(setup)
            ),
            xref("guide.adoc#setup", "Setup")
        );
        // In the same page, the id alone.
        assert_eq!(
            xref_to(&ws, &guide, &guide, &document, Some(setup)),
            xref("setup", "Setup")
        );
        // The page's own title is the page.
        let title = document.title.as_ref().unwrap().range;
        assert_eq!(
            xref_to(
                &ws,
                &pages.join("index.adoc"),
                &guide,
                &document,
                Some(title)
            ),
            xref("guide.adoc", "Guide")
        );
    }

    #[test]
    fn a_section_without_an_anchor_has_the_id_its_build_generates() {
        let (mut ws, root) = workspace("antora-multi-component");
        let guide = root.join("alpha/modules/ROOT/pages/guide.adoc");
        ws.index.index_source(&guide, "= Guide\n\n== First Steps\n");
        let document = ws.index.file(&guide).unwrap().document.clone();
        let section = document.sections[0].selection_range;
        let from = root.join("alpha/modules/ROOT/pages/index.adoc");
        assert_eq!(
            xref_to(&ws, &from, &guide, &document, Some(section)),
            xref("guide.adoc#first-steps", "First Steps")
        );

        // Asciidoctor's own form outside Antora.
        let (mut ws, root) = workspace("xrefs");
        let page = root.join("other.adoc");
        ws.index.index_source(&page, "= Other\n\n== First Steps\n");
        let document = ws.index.file(&page).unwrap().document.clone();
        let section = document.sections[0].selection_range;
        assert_eq!(
            xref_to(
                &ws,
                &root.join("index.adoc"),
                &page,
                &document,
                Some(section)
            )
            .map(|link| link.target),
            Some("other.adoc#_first_steps".to_owned())
        );
    }

    #[test]
    fn outside_antora_a_page_is_named_by_its_relative_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        std::fs::create_dir_all(root.join("guide")).unwrap();
        std::fs::write(root.join("index.adoc"), "= Home\n").unwrap();
        std::fs::write(root.join("guide/setup.adoc"), "= Set-up\n").unwrap();
        let mut ws = WorkspaceState::new();
        ws.index_roots(std::slice::from_ref(&root)).unwrap();
        assert_eq!(
            link(
                &ws,
                &root.join("index.adoc"),
                &root.join("guide/setup.adoc")
            ),
            xref("guide/setup.adoc", "Set-up")
        );
        assert_eq!(
            link(
                &ws,
                &root.join("guide/setup.adoc"),
                &root.join("index.adoc")
            ),
            xref("../index.adoc", "Home")
        );
    }

    #[test]
    fn only_a_page_can_be_linked() {
        let (ws, root) = workspace("antora-single-component");
        let from = root.join("modules/ROOT/pages/index.adoc");
        let partial = std::fs::read_dir(root.join("modules/ROOT/partials"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let document = adoc_parser::parse("file:///partial.adoc", "= Partial\n").document;
        assert_eq!(xref_to(&ws, &from, &partial, &document, None), None);
        // Nor the page itself without a section.
        let target = ws.index.file(&from).unwrap().document.clone();
        assert_eq!(xref_to(&ws, &from, &from, &target, None), None);
    }

    #[test]
    fn a_page_without_a_title_has_no_text() {
        let (mut ws, root) = workspace("antora-multi-component");
        let pages = root.join("alpha/modules/ROOT/pages");
        ws.index
            .index_source(pages.join("guide.adoc"), "Untitled.\n");
        assert_eq!(
            link(&ws, &pages.join("index.adoc"), &pages.join("guide.adoc")),
            Some(XrefLink {
                target: "guide.adoc".to_owned(),
                text: None,
            })
        );
    }

    #[test]
    fn writes_an_xref_or_an_in_page_reference() {
        let page = XrefLink {
            target: "beta::index.adoc".to_owned(),
            text: Some("Beta [home]".to_owned()),
        };
        assert_eq!(page.source(), "xref:beta::index.adoc[Beta [home\\]]");
        let untitled = XrefLink {
            target: "guide.adoc".to_owned(),
            text: None,
        };
        assert_eq!(untitled.source(), "xref:guide.adoc[]");
        let section = XrefLink {
            target: "setup".to_owned(),
            text: Some("Setup".to_owned()),
        };
        assert_eq!(section.source(), "<<setup,Setup>>");
    }
}
