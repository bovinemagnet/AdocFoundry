//! Finding references and renaming anchors. The index's reverse maps give the candidates, by the
//! file name a target ends with or by fragment, and resolving each decides which reach the
//! page or anchor; no request visits every document.

use std::path::{Path, PathBuf};

use adoc_core::{Document, Location, Reference, ReferenceKind, SourceRange};
use adoc_index::{normalize_path, IncludeLocation, ReferenceLocation};

use crate::{definition::reference_target_path, with_document, WorkspaceState};

/// The anchor a position names: its file and id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorTarget {
    pub path: PathBuf,
    pub id: String,
}

/// The references to what is at `offset` of the page at `path`: to the anchor there, when
/// `offset` is on an anchor or a reference to one, else to the page itself (the xrefs that
/// link to it and the includes that include it).
#[must_use]
pub fn references_at(ws: &WorkspaceState, path: &Path, text: &str, offset: usize) -> Vec<Location> {
    let path = normalize_path(path);
    with_document(ws, &path, text, |document| {
        match anchor_at(ws, &path, document, offset) {
            Some(anchor) => anchor_references(ws, &anchor)
                .into_iter()
                .map(|location| Location {
                    path: location.path.clone(),
                    range: location.reference.range,
                })
                .collect(),
            None => page_references(ws, &path),
        }
    })
}

/// The xrefs that link to the page at `target` and the include directives that include it,
/// sorted.
#[must_use]
pub fn page_references(ws: &WorkspaceState, target: &Path) -> Vec<Location> {
    let target = normalize_path(target);
    let mut found = crate::references_to(ws, &target);
    found.extend(
        includes_of(ws, &target)
            .into_iter()
            .map(|location| Location {
                path: location.path.clone(),
                range: location.include.range,
            }),
    );
    found.sort();
    found.dedup();
    found
}

/// The include directives that include the file at `target`: relative ones, ones through an
/// attribute the including page declares, and Antora resource ids.
#[must_use]
pub fn includes_of<'a>(ws: &'a WorkspaceState, target: &Path) -> Vec<&'a IncludeLocation> {
    let target = normalize_path(target);
    let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    ws.index
        .includes_naming(name)
        .filter(|location| {
            let Some(file) = ws.index.file(&location.path) else {
                return false;
            };
            adoc_index::resolve_include_target(
                &file.document,
                &location.path,
                &location.include.target,
            )
            .or_else(|| {
                reference_target_path(
                    &ws.index,
                    &ws.antora,
                    &location.path,
                    &location.include.target,
                )
            })
            .is_some_and(|resolved| normalize_path(&resolved) == target)
        })
        .collect()
}

/// The anchor at `offset`: an `[[id]]` or `[#id]` there, or the anchor a reference there points
/// at.
#[must_use]
pub fn anchor_at(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Option<AnchorTarget> {
    if let Some(anchor) = document
        .anchors
        .iter()
        .find(|anchor| anchor.range.start <= offset && offset < anchor.range.end)
    {
        return Some(AnchorTarget {
            path: path.to_path_buf(),
            id: anchor.id.clone(),
        });
    }
    let reference = document
        .references
        .iter()
        .find(|reference| reference.range.start <= offset && offset < reference.range.end)?;
    let (target, id) = fragment_target(ws, path, reference)?;
    Some(AnchorTarget {
        path: target,
        id: id.to_owned(),
    })
}

/// Renames the anchor at `offset` to `new_id`: the edits to its declaration and to every
/// reference to it, in file order. `None` when there is no anchor there or `new_id` is not a
/// usable id.
#[must_use]
pub fn rename_anchor(
    ws: &WorkspaceState,
    path: &Path,
    text: &str,
    offset: usize,
    new_id: &str,
) -> Option<Vec<(Location, String)>> {
    if new_id.is_empty()
        || new_id
            .chars()
            .any(|c| c.is_whitespace() || "[]<>#,\"'".contains(c))
    {
        return None;
    }
    let path = normalize_path(path);
    let anchor = with_document(ws, &path, text, |document| {
        anchor_at(ws, &path, document, offset)
    })?;
    let mut edits = Vec::new();
    // The declaration, in the open text when it is this page.
    let declared = |document: &Document, text: &str| {
        document
            .anchors
            .iter()
            .filter(|declared| declared.id == anchor.id)
            .filter_map(|declared| id_range(text, declared.range, &anchor.id))
            .collect::<Vec<_>>()
    };
    let declarations = if anchor.path == path {
        with_document(ws, &path, text, |document| declared(document, text))
    } else {
        let file = ws.index.file(&anchor.path)?;
        declared(&file.document, &file.document.text)
    };
    if declarations.is_empty() {
        return None;
    }
    for range in declarations {
        edits.push((
            Location {
                path: anchor.path.clone(),
                range,
            },
            new_id.to_owned(),
        ));
    }
    for location in anchor_references(ws, &anchor) {
        let Some(file) = ws.index.file(&location.path) else {
            continue;
        };
        if let Some(range) = id_range(&file.document.text, location.reference.range, &anchor.id) {
            edits.push((
                Location {
                    path: location.path.clone(),
                    range,
                },
                new_id.to_owned(),
            ));
        }
    }
    edits.sort_by(|a, b| a.0.cmp(&b.0));
    edits.dedup();
    Some(edits)
}

/// The references that reach `anchor`.
fn anchor_references<'a>(
    ws: &'a WorkspaceState,
    anchor: &AnchorTarget,
) -> Vec<&'a ReferenceLocation> {
    ws.index
        .references_with_fragment(&anchor.id)
        .filter(|location| {
            fragment_target(ws, &location.path, &location.reference)
                .is_some_and(|(target, _)| target == anchor.path)
        })
        .collect()
}

/// The file and id a reference's fragment points at. A local reference reaches the page it is
/// on, or a page that page is included in, whichever declares the anchor.
fn fragment_target<'a>(
    ws: &WorkspaceState,
    path: &Path,
    reference: &'a Reference,
) -> Option<(PathBuf, &'a str)> {
    match reference.kind {
        ReferenceKind::LocalAnchor => {
            let id = reference.target.trim();
            let declares = |candidate: &Path| ws.index.resolve_anchor(candidate, id).is_some();
            if declares(path) {
                return Some((path.to_path_buf(), id));
            }
            ws.index
                .includers(path)
                .find(|includer| declares(includer))
                .map(|includer| (includer.to_path_buf(), id))
                .or_else(|| Some((path.to_path_buf(), id)))
        }
        ReferenceKind::Xref => {
            let (file, id) = reference.target.split_once('#')?;
            let target = reference_target_path(&ws.index, &ws.antora, path, file.trim())?;
            Some((normalize_path(&target), id.trim()))
        }
        ReferenceKind::Link | ReferenceKind::Attribute => None,
    }
}

/// The range of `id` within `text[range]`: after its `#`, or after the opening `[[`, `[#`,
/// `<<` or `xref:`.
fn id_range(text: &str, range: SourceRange, id: &str) -> Option<SourceRange> {
    let source = text.get(range.start..range.end)?;
    let at = match source.find('#') {
        Some(hash) if source[hash + 1..].starts_with(id) => hash + 1,
        _ => ["[[", "<<", "xref:"]
            .iter()
            .filter_map(|opener| source.strip_prefix(opener).map(|_| opener.len()))
            .map(|start| start + (source[start..].len() - source[start..].trim_start().len()))
            .find(|&start| source[start..].starts_with(id))?,
    };
    Some(SourceRange::new(
        range.start + at,
        range.start + at + id.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/references")
                .join(name),
        )
    }

    fn workspace() -> WorkspaceState {
        let mut ws = WorkspaceState::new();
        ws.index_roots(&[fixture("")]).unwrap();
        ws
    }

    fn quoted(locations: &[Location]) -> Vec<(String, String)> {
        locations
            .iter()
            .map(|location| {
                let text = std::fs::read_to_string(&location.path).unwrap();
                (
                    location.path.file_name().unwrap().to_string_lossy().into(),
                    text[location.range.start..location.range.end].to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn a_page_is_referenced_by_its_xrefs_and_includes() {
        let ws = workspace();
        let index = fixture("index.adoc");
        let text = std::fs::read_to_string(&index).unwrap();
        assert_eq!(
            quoted(&references_at(&ws, &index, &text, 2)),
            [
                ("other.adoc".into(), "xref:index.adoc#setup[setup]".into()),
                ("other.adoc".into(), "xref:index.adoc[the index]".into()),
            ]
        );
        assert_eq!(
            quoted(&page_references(&ws, &fixture("shared.adoc"))),
            [("index.adoc".into(), "include::shared.adoc[]".into())]
        );
    }

    #[test]
    fn a_partial_is_referenced_by_its_antora_includes() {
        let root = normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/antora-single-component"),
        );
        let mut ws = WorkspaceState::new();
        ws.index_roots(std::slice::from_ref(&root)).unwrap();
        let partial = root.join("modules/security/partials/token-note.adoc");
        let found = page_references(&ws, &partial);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0]
            .path
            .ends_with("security/pages/authentication.adoc"));
    }

    #[test]
    fn an_anchor_is_referenced_wherever_it_is_named() {
        let ws = workspace();
        let index = fixture("index.adoc");
        let text = std::fs::read_to_string(&index).unwrap();
        let expected: Vec<(String, String)> = vec![
            ("index.adoc".into(), "<<setup>>".into()),
            ("other.adoc".into(), "xref:index.adoc#setup[setup]".into()),
        ];
        let on_declaration = text.find("[[setup]]").unwrap() + 3;
        let mut found = quoted(&references_at(&ws, &index, &text, on_declaration));
        found.sort();
        assert_eq!(found, expected);
        let on_reference = text.find("<<setup>>").unwrap() + 3;
        let mut found = quoted(&references_at(&ws, &index, &text, on_reference));
        found.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn renaming_an_anchor_edits_its_declaration_and_references() {
        let ws = workspace();
        let index = fixture("index.adoc");
        let text = std::fs::read_to_string(&index).unwrap();
        let offset = text.find("[[setup]]").unwrap() + 3;
        let edits = rename_anchor(&ws, &index, &text, offset, "install").unwrap();
        let mut renamed: Vec<_> = edits
            .iter()
            .map(|(location, new)| {
                assert_eq!(new, "install");
                quoted(std::slice::from_ref(location)).remove(0)
            })
            .collect();
        renamed.sort();
        assert_eq!(
            renamed,
            [
                ("index.adoc".to_owned(), "setup".to_owned()),
                ("index.adoc".to_owned(), "setup".to_owned()),
                ("other.adoc".to_owned(), "setup".to_owned()),
            ]
        );
        assert_eq!(edits.len(), 3);
        assert!(rename_anchor(&ws, &index, &text, offset, "two words").is_none());
        assert!(rename_anchor(&ws, &index, &text, 2, "install").is_none());
    }
}
