//! Editor-agnostic AsciiDoc and Antora language features, operating on byte offsets.
//!
//! Both the language server and the engine call these functions; neither re-implements them.
//! Nothing here knows about LSP, URLs or any UI framework.

pub mod attributes;
pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod hover;
pub mod images;
pub mod includes;
pub mod references;
pub mod render_source;
pub mod symbols;
mod workspace;
pub mod xref;

use std::path::{Path, PathBuf};

use adoc_antora::ResourceFamily;
use adoc_core::{Diagnostic, Document, SourceRange};
use adoc_index::normalize_path;
use adoc_parser::parse;

pub use adoc_core::Location;
pub use adoc_index::{WorkspaceSymbol, WorkspaceSymbolKind};
pub use completion::{Candidate as CompletionItem, CandidateKind as CompletionItemKind};
pub use hover::HoverContent as Hover;
pub use symbols::{DocumentSymbol, DocumentSymbolKind};
pub use workspace::WorkspaceState;
pub use xref::{xref_to, XrefLink};

/// Where following the construct under the cursor leads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionTarget {
    /// A range in a text file to open in the editor.
    Location(Location),
    /// A file to open with the operating system, such as an image or attachment.
    External(PathBuf),
}

/// Runs `f` with the parsed form of `text`, reusing the indexed document when its text matches.
fn with_document<R>(
    ws: &WorkspaceState,
    path: &Path,
    text: &str,
    f: impl FnOnce(&Document) -> R,
) -> R {
    if let Some(file) = ws.index.file(path) {
        if file.document.text == text {
            return f(&file.document);
        }
    }
    let uri = format!("file://{}", normalize_path(path).to_string_lossy());
    f(&parse(&uri, text).document)
}

/// The target of the xref, include, image, attachment or attribute reference at `offset`.
#[must_use]
pub fn definition_at_offset(
    ws: &WorkspaceState,
    path: &Path,
    text: &str,
    offset: usize,
) -> Option<DefinitionTarget> {
    with_document(ws, path, text, |document| {
        definition_in(ws, path, document, offset)
    })
}

/// [`definition_at_offset`] in `document`, the parse of the text at `path` already in hand.
#[must_use]
pub fn definition_in(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Option<DefinitionTarget> {
    {
        if let Some(target) =
            definition::definition_at_offset(&ws.index, &ws.antora, path, document, offset)
        {
            return Some(if is_external(ws, &target.path) {
                DefinitionTarget::External(target.path)
            } else {
                DefinitionTarget::Location(Location {
                    path: target.path,
                    range: target.range,
                })
            });
        }
        if let Some(image) = document
            .images
            .iter()
            .find(|image| contains(image.range, offset))
        {
            return images::resolve_image(&ws.index, &ws.antora, path, document, &image.target)
                .map(DefinitionTarget::External);
        }
        attribute_definition(ws, path, document, offset).map(DefinitionTarget::Location)
    }
}

/// Images and attachments are opened by the operating system rather than the editor.
fn is_external(ws: &WorkspaceState, target: &Path) -> bool {
    let target = normalize_path(target);
    ws.antora
        .resources()
        .find(|resource| normalize_path(&resource.source_path) == target)
        .is_some_and(|resource| {
            matches!(
                resource.coordinate.family,
                ResourceFamily::Image | ResourceFamily::Attachment
            )
        })
}

/// Where the attribute referenced at `offset` is declared, if it is declared in a file.
fn attribute_definition(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Option<Location> {
    let (name, _) = hover::attribute_reference_at(&document.text, offset)?;
    let declared_in = |file: &Path, doc: &Document| {
        doc.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| Location {
                path: normalize_path(file),
                range: attribute.range,
            })
    };
    if let Some(location) = declared_in(path, document) {
        return Some(location);
    }
    let context = ws.antora.context_for_path(path);
    for included in includes::composed_files(&ws.index, &ws.antora, context.as_ref(), path) {
        if let Some(file) = ws.index.file(&included) {
            if let Some(location) = declared_in(&included, &file.document) {
                return Some(location);
            }
        }
    }
    context.as_ref()?;
    // The same precedence `attributes::attribute_definitions` applies.
    match attributes::attribute_definitions(&ws.index, &ws.antora, path, document)
        .into_iter()
        .find(|definition| definition.name == name)?
        .source
    {
        attributes::AttributeSource::Playbook => ws.antora.playbook().map(|playbook| Location {
            path: playbook.path.clone(),
            range: SourceRange::new(0, 0),
        }),
        attributes::AttributeSource::Component => {
            let context = context?;
            let component = ws
                .antora
                .component(&context.component, context.version.as_deref())?;
            Some(Location {
                path: component.root.join("antora.yml"),
                range: SourceRange::new(0, 0),
            })
        }
        _ => None,
    }
}

fn contains(range: SourceRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

#[must_use]
pub fn hover_at_offset(
    ws: &WorkspaceState,
    path: &Path,
    text: &str,
    offset: usize,
) -> Option<Hover> {
    with_document(ws, path, text, |document| {
        hover_in(ws, path, document, offset)
    })
}

/// [`hover_at_offset`] in `document`, the parse of the text at `path` already in hand.
#[must_use]
pub fn hover_in(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Option<Hover> {
    hover::hover_at_offset(&ws.index, &ws.antora, path, document, offset)
}

#[must_use]
pub fn completion_at_offset(
    ws: &WorkspaceState,
    path: &Path,
    text: &str,
    offset: usize,
) -> Vec<CompletionItem> {
    with_document(ws, path, text, |document| {
        completion_in(ws, path, document, offset)
    })
}

/// [`completion_at_offset`] in `document`, the parse of the text at `path` already in hand.
#[must_use]
pub fn completion_in(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Vec<CompletionItem> {
    completion::completion_at_offset(&ws.index, &ws.antora, path, document, offset)
}

/// Every diagnostic for the indexed file at `path`, including problems with the component
/// descriptor when `path` is an `antora.yml`.
#[must_use]
pub fn diagnostics(ws: &WorkspaceState, path: &Path) -> Vec<Diagnostic> {
    let mut found = diagnostics::diagnostics(&ws.index, &ws.antora, path);
    found.extend(diagnostics::descriptor_diagnostics(
        &ws.descriptor_issues,
        path,
    ));
    if let Some(problems) = ws.descriptor_problems.get(&normalize_path(path)) {
        found.extend(problems.iter().cloned());
    }
    found
}

/// The files whose diagnostics the text of `path` can decide, `path` among them: those whose
/// xrefs name its file name, those whose includes do, and the files including those in turn,
/// and the files `path` composes. Name matches stand in for resolution, so the set is generous
/// but never misses a file; a front end re-diagnoses these after an edit, not every file, and
/// calls this before and after the edit, since an edit can change what `path` composes.
#[must_use]
pub fn diagnosed_with(ws: &WorkspaceState, path: &Path) -> std::collections::BTreeSet<PathBuf> {
    let path = normalize_path(path);
    let mut found = std::collections::BTreeSet::from([path.clone()]);
    let context = ws.antora.context_for_path(&path);
    found.extend(includes::composed_files(
        &ws.index,
        &ws.antora,
        context.as_ref(),
        &path,
    ));
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return found;
    };
    found.extend(
        ws.index
            .xrefs_naming(name)
            .map(|location| location.path.to_path_buf()),
    );
    // The files composing `path`, and those composing them, up to the include depth.
    let mut composing = vec![path.clone()];
    let mut seen = std::collections::BTreeSet::from([path]);
    for _ in 0..=includes::MAX_INCLUDE_DEPTH {
        let mut next = Vec::new();
        for file in &composing {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            for location in ws.index.includes_naming(name) {
                if seen.insert(location.path.to_path_buf()) {
                    next.push(location.path.to_path_buf());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        found.extend(next.iter().cloned());
        composing = next;
    }
    found
}

/// Diagnostics for every indexed file and every component descriptor with problems, sorted by
/// path and omitting files without diagnostics.
#[must_use]
pub fn workspace_diagnostics(ws: &WorkspaceState) -> Vec<(PathBuf, Vec<Diagnostic>)> {
    let mut paths: Vec<PathBuf> = ws.index.files().map(|file| file.path.clone()).collect();
    paths.extend(
        ws.descriptor_issues
            .iter()
            .map(|issue| normalize_path(&issue.path)),
    );
    paths.extend(ws.descriptor_problems.keys().cloned());
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let mut found = diagnostics(ws, &path);
            found.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
            (!found.is_empty()).then_some((path, found))
        })
        .collect()
}

/// The outline of a document: its sections, or for an Antora navigation file its entries.
#[must_use]
pub fn document_symbols(document: &Document) -> Vec<DocumentSymbol> {
    symbols::document_symbols(document)
}

#[must_use]
pub fn workspace_symbols(ws: &WorkspaceState, query: &str) -> Vec<WorkspaceSymbol> {
    ws.index
        .symbols_matching(query)
        .into_iter()
        .cloned()
        .collect()
}

/// Every xref in the workspace that resolves to the file `target`, as the range of the whole
/// reference.
#[must_use]
pub fn references_to(ws: &WorkspaceState, target: &Path) -> Vec<Location> {
    let target = normalize_path(target);
    let Some(file_name) = target.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    // An xref may name the page by one of its `page-aliases` instead.
    let mut names = vec![file_name.to_owned()];
    if let Some(resource) = ws.antora.resource_for_path(&target) {
        names.extend(resource.aliases.iter().filter_map(|alias| {
            Path::new(&alias.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        }));
    }
    names.sort();
    names.dedup();
    let mut found: Vec<Location> = names
        .iter()
        .flat_map(|name| ws.index.xrefs_naming(name))
        .filter(|location| {
            let file_part = location
                .reference
                .target
                .split_once('#')
                .map_or(location.reference.target.as_str(), |(file, _)| file);
            !file_part.is_empty()
                && definition::reference_target_path(
                    &ws.index,
                    &ws.antora,
                    location.path,
                    file_part,
                )
                .is_some_and(|path| normalize_path(&path) == target)
        })
        .map(|location| Location {
            path: location.path.to_path_buf(),
            range: location.reference.range,
        })
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures")
                .join(name),
        )
    }

    fn workspace(name: &str) -> WorkspaceState {
        let mut ws = WorkspaceState::new();
        ws.index_roots(&[fixture(name)]).unwrap();
        ws
    }

    #[test]
    fn follows_relative_xrefs_to_a_location() {
        let ws = workspace("xrefs");
        let path = fixture("xrefs/index.adoc");
        let text = std::fs::read_to_string(&path).unwrap();
        let offset = text.find("xref:other.adoc").unwrap() + 5;
        let Some(DefinitionTarget::Location(location)) =
            definition_at_offset(&ws, &path, &text, offset)
        else {
            panic!("xref must resolve to a location");
        };
        assert!(location.path.ends_with("xrefs/other.adoc"));
    }

    #[test]
    fn follows_images_externally() {
        let ws = workspace("images");
        let path = fixture("images/guide.adoc");
        let text = std::fs::read_to_string(&path).unwrap();
        let offset = text.find("image::present").unwrap() + 8;
        assert!(matches!(
            definition_at_offset(&ws, &path, &text, offset),
            Some(DefinitionTarget::External(target)) if target.ends_with("media/present.svg")
        ));
        let missing = text.find("image::missing").unwrap() + 8;
        assert_eq!(definition_at_offset(&ws, &path, &text, missing), None);
    }

    #[test]
    fn follows_attribute_references_to_their_declaration() {
        let ws = WorkspaceState::new();
        let path = Path::new("/docs/page.adoc");
        let text = "= Page\n:product: Foundry\n\nUse {product} here.\n";
        let offset = text.find("{product}").unwrap() + 2;
        let Some(DefinitionTarget::Location(location)) =
            definition_at_offset(&ws, path, text, offset)
        else {
            panic!("attribute reference must resolve to its declaration");
        };
        assert_eq!(
            &text[location.range.start..location.range.end],
            ":product: Foundry"
        );
    }

    #[test]
    fn unsaved_text_is_parsed_rather_than_the_index_copy() {
        let ws = workspace("xrefs");
        let path = fixture("xrefs/index.adoc");
        let text = "See xref:other.adoc[].\n";
        assert!(matches!(
            definition_at_offset(&ws, &path, text, 6),
            Some(DefinitionTarget::Location(_))
        ));
    }

    #[test]
    fn playbook_attributes_are_offered_and_followed() {
        let ws = workspace("antora-multi-component");
        let path = fixture("antora-multi-component/alpha/modules/ROOT/pages/guide.adoc");
        let text = std::fs::read_to_string(&path).unwrap();
        let file = ws.index.file(&path).unwrap();
        let definitions =
            attributes::attribute_definitions(&ws.index, &ws.antora, &path, &file.document);
        let soft = definitions
            .iter()
            .find(|definition| definition.name == "release-name")
            .unwrap();
        assert_eq!(soft.value.as_deref(), Some("Phase One"));
        assert_eq!(soft.source, attributes::AttributeSource::Playbook);

        let offset = text.find("{company}").unwrap() + 2;
        let Some(DefinitionTarget::Location(location)) =
            definition_at_offset(&ws, &path, &text, offset)
        else {
            panic!("a playbook attribute leads to the playbook");
        };
        assert!(location
            .path
            .ends_with("antora-multi-component/antora-playbook.yml"));
        assert!(hover_at_offset(&ws, &path, &text, offset)
            .unwrap()
            .markdown
            .contains("antora-playbook.yml"));
    }

    #[test]
    fn references_to_finds_xrefs_into_a_page() {
        let ws = workspace("xrefs");
        let references = references_to(&ws, &fixture("xrefs/other.adoc"));
        assert!(!references.is_empty());
        assert!(references
            .iter()
            .all(|location| location.path.ends_with("xrefs/index.adoc")));
    }

    #[test]
    fn workspace_diagnostics_are_sorted_and_non_empty() {
        let ws = workspace("antora-invalid-descriptor");
        let all = workspace_diagnostics(&ws);
        assert!(!all.is_empty());
        assert!(all.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(all.iter().all(|(_, found)| !found.is_empty()));
    }
}
