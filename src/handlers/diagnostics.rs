use std::{collections::BTreeSet, path::Path};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraContext, AntoraResolver, ResolutionError,
    ResolutionResult, ResourceFamily,
};
use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, ReferenceKind, SourceRange};
use adoc_index::{workspace_diagnostics, WorkspaceIndex};

use super::includes::composed_files;

#[must_use]
pub fn diagnostics(index: &WorkspaceIndex, antora: &AntoraCatalog, path: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = workspace_diagnostics(index, path);
    let Some(file) = index.file(path) else {
        return diagnostics;
    };
    let Some(context) = antora.context_for_path(path) else {
        return diagnostics;
    };

    // Inside an Antora module every xref target is a resource ID whose path is relative to the
    // module's family directory, never to the current file. Resolve them here and drop the
    // path-relative findings the Antora-unaware index layer produced for the same references.
    let mut pending = Vec::new();
    let mut resolved = BTreeSet::new();
    for reference in &file.document.references {
        if reference.kind != ReferenceKind::Xref || !is_antora_target(&reference.target) {
            continue;
        }
        let (target, anchor) = reference
            .target
            .split_once('#')
            .map_or((reference.target.as_str(), None), |(target, anchor)| {
                (target, Some(anchor))
            });
        let Ok(id) = parse_resource_id(target) else {
            continue;
        };
        match AntoraResolver::resolve(antora, &id, &context) {
            Ok(resource) => {
                resolved.insert(reference.range);
                if let Some(anchor) = anchor.filter(|anchor| !anchor.is_empty()) {
                    if !declares_anchor(index, antora, &context, &resource.source_path, anchor) {
                        pending.push((
                            DiagnosticCode::UnresolvedAnchor,
                            format!("Unresolved AsciiDoc anchor: {anchor}"),
                            reference.range,
                        ));
                    }
                }
            }
            // A component missing from the workspace is normally published from another
            // repository in the playbook, so its absence here says nothing about the xref.
            Err(ResolutionError::UnknownComponent { .. }) => {
                resolved.insert(reference.range);
            }
            // Only explicit resource IDs are reported as Antora failures. A bare path that the
            // catalog cannot resolve keeps whatever the index layer decided, so an incomplete
            // catalog cannot turn into a false positive.
            Err(error) if is_explicit_antora_target(target) => {
                pending.push(resolution_diagnostic(&error, target, reference.range));
            }
            Err(_) => {}
        }
    }

    // `<<anchor>>` may point at a partial the page pulls in, which the index layer resolves
    // per file and therefore cannot see. A partial is the mirror image: it is only ever read as
    // part of some page, so an anchor it references may well be declared by its includer and
    // nothing it says can be judged on its own.
    let composed_elsewhere = context.family == ResourceFamily::Partial;
    for reference in &file.document.references {
        if reference.kind == ReferenceKind::LocalAnchor
            && !is_dynamic(&reference.target)
            && (composed_elsewhere
                || declares_anchor(index, antora, &context, path, &reference.target))
        {
            resolved.insert(reference.range);
        }
    }

    diagnostics.retain(|diagnostic| {
        !matches!(
            diagnostic.code,
            DiagnosticCode::UnresolvedXrefFile | DiagnosticCode::UnresolvedAnchor
        ) || !resolved.contains(&diagnostic.range)
    });

    let mut seen = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.range))
        .collect::<BTreeSet<_>>();

    {
        let mut output = DiagnosticOutput {
            diagnostics: &mut diagnostics,
            seen: &mut seen,
        };
        for (code, message, range) in pending {
            output.push(code, message, range);
        }

        for include in &file.document.includes {
            if !include.target.contains('$') || is_dynamic(&include.target) {
                continue;
            }
            let Ok(id) = parse_resource_id(&include.target) else {
                continue;
            };
            diagnose_resolution(
                AntoraResolver::resolve(antora, &id, &context),
                &include.target,
                None,
                index,
                include.range,
                &mut output,
            );
        }
    }

    diagnostics.sort_by_key(|diagnostic| (diagnostic.range, diagnostic.code));
    diagnostics
}

fn diagnose_resolution(
    resolution: ResolutionResult<'_>,
    target: &str,
    anchor: Option<&str>,
    index: &WorkspaceIndex,
    range: SourceRange,
    output: &mut DiagnosticOutput<'_>,
) {
    match resolution {
        Ok(resource) => {
            if let Some(anchor) = anchor.filter(|anchor| !anchor.is_empty()) {
                if index
                    .resolve_anchor(&resource.source_path, anchor)
                    .is_none()
                {
                    output.push(
                        DiagnosticCode::UnresolvedAnchor,
                        format!("Unresolved AsciiDoc anchor: {anchor}"),
                        range,
                    );
                }
            }
        }
        Err(ResolutionError::UnknownModule { module, .. }) => output.push(
            DiagnosticCode::AntoraUnknownModule,
            format!("Unknown Antora module: {module}"),
            range,
        ),
        Err(ResolutionError::UnknownComponent { .. })
        | Err(ResolutionError::UnknownResource { .. }) => output.push(
            DiagnosticCode::AntoraUnknownResource,
            format!("Unknown Antora resource: {target}"),
            range,
        ),
    }
}

/// An anchor counts as declared when any file composed into `path` declares it.
fn declares_anchor(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    path: &Path,
    anchor: &str,
) -> bool {
    if index.resolve_anchor(path, anchor).is_some() {
        return true;
    }
    composed_files(index, antora, context, path)
        .iter()
        .any(|composed| index.resolve_anchor(composed, anchor).is_some())
}

fn resolution_diagnostic(
    error: &ResolutionError,
    target: &str,
    range: SourceRange,
) -> (DiagnosticCode, String, SourceRange) {
    match error {
        ResolutionError::UnknownModule { module, .. } => (
            DiagnosticCode::AntoraUnknownModule,
            format!("Unknown Antora module: {module}"),
            range,
        ),
        ResolutionError::UnknownComponent { .. } | ResolutionError::UnknownResource { .. } => (
            DiagnosticCode::AntoraUnknownResource,
            format!("Unknown Antora resource: {target}"),
            range,
        ),
    }
}

/// Any xref target the Antora resolver can be asked about: bare page paths included, external
/// links and half-typed attribute references excluded.
fn is_antora_target(target: &str) -> bool {
    !is_dynamic(target) && !target.contains("://") && !target.starts_with("mailto:")
}

fn is_explicit_antora_target(target: &str) -> bool {
    !is_dynamic(target) && !target.contains("://") && (target.contains(':') || target.contains('$'))
}

fn is_dynamic(target: &str) -> bool {
    target.contains('{') || target.contains('}')
}

struct DiagnosticOutput<'a> {
    diagnostics: &'a mut Vec<Diagnostic>,
    seen: &'a mut BTreeSet<(DiagnosticCode, SourceRange)>,
}

impl DiagnosticOutput<'_> {
    fn push(&mut self, code: DiagnosticCode, message: String, range: SourceRange) {
        if self.seen.insert((code, range)) {
            self.diagnostics.push(Diagnostic {
                code,
                severity: DiagnosticSeverity::Warning,
                message,
                range,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_antora::discover_antora_workspace;
    use adoc_core::DiagnosticCode;
    use adoc_index::WorkspaceIndex;

    use super::diagnostics;

    #[test]
    fn validates_antora_xrefs_and_includes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;
        let index_path = root.join("modules/ROOT/pages/index.adoc");
        let authentication_path = root.join("modules/security/pages/authentication.adoc");

        assert!(diagnostics(&index, &antora, &index_path).is_empty());
        assert!(diagnostics(&index, &antora, &authentication_path).is_empty());

        index.index_source(
            &index_path,
            "= Home\n\nxref:missing:page.adoc[]\ninclude::partial$missing.adoc[]\n",
        );
        let codes = diagnostics(&index, &antora, &index_path)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&DiagnosticCode::AntoraUnknownModule));
        assert!(codes.contains(&DiagnosticCode::AntoraUnknownResource));
    }

    #[test]
    fn leaves_anchors_in_partials_to_the_pages_that_compose_them() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        let partial = root.join("modules/ROOT/partials/glossary.adoc");
        index.index_source(&partial, "See <<declared-by-the-including-page>>.\n");

        assert_eq!(diagnostics(&index, &antora, &partial), Vec::new());
    }

    #[test]
    fn stays_silent_about_components_outside_the_workspace() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        let page = root.join("modules/ROOT/pages/index.adoc");
        index.index_source(
            &page,
            "= Home\n\nxref:other-repo:ROOT:index.adoc[]\nxref:other-repo::index.adoc[]\nxref:missing:page.adoc[]\n",
        );
        let codes = diagnostics(&index, &antora, &page)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert_eq!(codes, vec![DiagnosticCode::AntoraUnknownModule]);
    }

    #[test]
    fn resolves_anchors_declared_in_included_partials() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        let composed = root.join("modules/ROOT/pages/guides/composed.adoc");
        assert_eq!(diagnostics(&index, &antora, &composed), Vec::new());
    }

    #[test]
    fn resolves_navigation_file_xrefs_against_its_module() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;

        let nav = root.join("modules/ROOT/nav.adoc");
        assert_eq!(diagnostics(&index, &antora, &nav), Vec::new());
    }

    #[test]
    fn resolves_module_root_relative_xrefs_in_nested_pages() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;
        let nested = root.join("modules/ROOT/pages/guides/getting-started.adoc");

        assert_eq!(diagnostics(&index, &antora, &nested), Vec::new());

        index.index_source(
            &nested,
            "= Getting Started\n\nxref:reference/missing.adoc[]\nxref:reference/api.adoc#nope[]\n",
        );
        let codes = diagnostics(&index, &antora, &nested)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                DiagnosticCode::UnresolvedXrefFile,
                DiagnosticCode::UnresolvedAnchor
            ]
        );
    }
}
