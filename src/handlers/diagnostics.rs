use std::{collections::BTreeSet, path::Path};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraContext, AntoraResolver, DescriptorError,
    ResolutionError, ResolutionResult, ResourceFamily, ResourceIdParseError,
};
use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, ReferenceKind, SourceRange};
use adoc_index::{normalize_path, workspace_diagnostics, WorkspaceIndex};

use super::{images::antora_image_id, includes::composed_files};

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
        let id = match parse_resource_id(target) {
            Ok(id) => id,
            Err(error) => {
                if is_explicit_antora_target(target) {
                    pending.extend(
                        invalid_id(target, &error)
                            .map(|(code, message)| (code, message, reference.range)),
                    );
                }
                continue;
            }
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
    // The index layer resolved images against the document's directory, which is wrong
    // inside a module: Antora resolves them in the module's `images` family instead.
    diagnostics.retain(|diagnostic| diagnostic.code != DiagnosticCode::UnresolvedImage);

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
            let id = match parse_resource_id(&include.target) {
                Ok(id) => id,
                Err(error) => {
                    if let Some((code, message)) = invalid_id(&include.target, &error) {
                        output.push(code, message, include.range);
                    }
                    continue;
                }
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

        // As with anchors, a partial is judged only as part of the page that includes it.
        if !composed_elsewhere {
            for image in &file.document.images {
                let id = match antora_image_id(&image.target) {
                    Ok(Some(id)) => id,
                    Ok(None) => continue,
                    Err(error) => {
                        if is_explicit_antora_target(&image.target) {
                            if let Some((code, message)) = invalid_id(&image.target, &error) {
                                output.push(code, message, image.range);
                            }
                        }
                        continue;
                    }
                };
                match AntoraResolver::resolve(antora, &id, &context) {
                    // A component missing from the workspace is published from elsewhere.
                    Ok(_) | Err(ResolutionError::UnknownComponent { .. }) => {}
                    Err(ResolutionError::UnknownModule { module, .. }) => output.push(
                        DiagnosticCode::AntoraUnknownModule,
                        format!("Unknown Antora module: {module}"),
                        image.range,
                    ),
                    Err(ResolutionError::UnknownResource { .. }) => output.push(
                        DiagnosticCode::UnresolvedImage,
                        format!("Unresolved AsciiDoc image target: {}", image.target),
                        image.range,
                    ),
                }
            }
        }
    }

    diagnostics.sort_by_key(|diagnostic| (diagnostic.range, diagnostic.code));
    diagnostics
}

/// Warnings for a file inside a component whose `antora.yml` could not be read.
///
/// Such a component is missing from the catalog, so every Antora feature is silently off for
/// its files; this says why. Descriptors are read when the server starts, so the warning
/// outlives a fix until the server restarts.
#[must_use]
pub fn descriptor_diagnostics(issues: &[DescriptorError], path: &Path) -> Vec<Diagnostic> {
    let path = normalize_path(path);
    issues
        .iter()
        .filter(|issue| {
            issue
                .path
                .parent()
                .is_some_and(|root| path.starts_with(normalize_path(root)))
        })
        .map(|issue| Diagnostic {
            code: DiagnosticCode::AntoraInvalidDescriptor,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "`{}` is invalid, so Antora navigation and checks are off for this component: {}",
                issue.path.display(),
                issue.message
            ),
            range: SourceRange::new(0, 0),
        })
        .collect()
}

/// The diagnostic for a target written as a resource ID that does not parse.
///
/// Path errors are left alone: Antora accepts relative forms such as `./` that the parser
/// rejects, and a false positive is worse than silence.
fn invalid_id(target: &str, error: &ResourceIdParseError) -> Option<(DiagnosticCode, String)> {
    let code = match error {
        ResourceIdParseError::UnknownFamily(_) => DiagnosticCode::AntoraInvalidFamily,
        ResourceIdParseError::EmptyCoordinate
        | ResourceIdParseError::TooManyCoordinates
        | ResourceIdParseError::InvalidVersionCoordinate => DiagnosticCode::AntoraInvalidCoordinate,
        ResourceIdParseError::Empty | ResourceIdParseError::InvalidPath(_) => return None,
    };
    Some((
        code,
        format!("Invalid Antora resource ID `{target}`: {error}"),
    ))
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
    composed_files(index, antora, Some(context), path)
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

    use super::{descriptor_diagnostics, diagnostics};

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

    fn antora_single_component() -> (
        WorkspaceIndex,
        adoc_antora::AntoraCatalog,
        std::path::PathBuf,
    ) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;
        (index, antora, root)
    }

    fn codes_for(
        index: &mut WorkspaceIndex,
        antora: &adoc_antora::AntoraCatalog,
        path: &Path,
        text: &str,
    ) -> Vec<DiagnosticCode> {
        index.index_source(path, text);
        diagnostics(index, antora, path)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn resolves_antora_images_in_the_module_s_images_family() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");
        let security = root.join("modules/security/pages/authentication.adoc");

        // Bare, explicit-family and module-qualified forms. The index layer alone would
        // resolve the bare form against `pages/` and warn; the Antora answer replaces it.
        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nimage::architecture.svg[]\nimage::image$architecture.svg[]\n",
            ),
            Vec::new()
        );
        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &security,
                "= Auth\n\nimage::ROOT:architecture.svg[]\n"
            ),
            Vec::new()
        );
    }

    #[test]
    fn warns_once_about_a_missing_antora_image() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");

        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nimage::missing.svg[]\n"
            ),
            vec![DiagnosticCode::UnresolvedImage]
        );
    }

    #[test]
    fn reports_an_unknown_module_but_not_an_absent_component_for_images() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");

        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nimage::nowhere:a.svg[]\n"
            ),
            vec![DiagnosticCode::AntoraUnknownModule]
        );
        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nimage::2.0@other:ROOT:a.svg[]\n"
            ),
            Vec::new()
        );
    }

    #[test]
    fn leaves_images_in_partials_and_dynamic_targets_alone() {
        let (mut index, antora, root) = antora_single_component();
        let partial = root.join("modules/ROOT/partials/welcome.adoc");
        let home = root.join("modules/ROOT/pages/index.adoc");

        assert_eq!(
            codes_for(&mut index, &antora, &partial, "image::missing.svg[]\n"),
            Vec::new()
        );
        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nimage::{undeclared}/a.svg[]\nimage::https://example.com/a.svg[]\nimage::data:image/png;base64,AAAA[]\n",
            ),
            Vec::new()
        );
    }

    #[test]
    fn reports_malformed_resource_ids_by_kind() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");

        for (text, expected) in [
            (
                "include::partials$x.adoc[]\n",
                DiagnosticCode::AntoraInvalidFamily,
            ),
            (
                "xref:a:b:c:x.adoc[]\n",
                DiagnosticCode::AntoraInvalidCoordinate,
            ),
            (
                "xref::security:x.adoc[]\n",
                DiagnosticCode::AntoraInvalidCoordinate,
            ),
            (
                "xref:1@2@demo:x.adoc[]\n",
                DiagnosticCode::AntoraInvalidCoordinate,
            ),
            (
                "image::pictures$x.svg[]\n",
                DiagnosticCode::AntoraInvalidFamily,
            ),
        ] {
            let text = format!("= Home\n\n{text}");
            assert_eq!(
                codes_for(&mut index, &antora, &home, &text),
                vec![expected],
                "{text}"
            );
        }
    }

    #[test]
    fn names_the_malformed_target_in_the_message() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");
        index.index_source(&home, "= Home\n\ninclude::partials$x.adoc[]\n");

        let messages: Vec<_> = diagnostics(&index, &antora, &home)
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();

        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("partials$x.adoc"), "{messages:?}");
        assert!(messages[0].contains("`partials`"), "{messages:?}");
    }

    #[test]
    fn leaves_path_errors_and_non_antora_targets_alone() {
        let (mut index, antora, root) = antora_single_component();
        let home = root.join("modules/ROOT/pages/index.adoc");

        assert_eq!(
            codes_for(
                &mut index,
                &antora,
                &home,
                "= Home\n\nxref:security:../x.adoc[]\ninclude::partial$./x.adoc[]\nxref:{attr}:x.adoc[]\nimage::https://example.com/a:b.svg[]\n",
            ),
            Vec::new()
        );
    }

    #[test]
    fn warns_files_inside_a_component_whose_descriptor_is_invalid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-invalid-descriptor");
        let issues = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .issues;

        let inside = descriptor_diagnostics(&issues, &root.join("modules/ROOT/pages/index.adoc"));
        assert_eq!(
            inside
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            vec![DiagnosticCode::AntoraInvalidDescriptor]
        );
        assert!(inside[0].message.contains("antora.yml"), "{inside:?}");
        assert!(inside[0].message.contains("name"), "{inside:?}");

        let outside = descriptor_diagnostics(
            &issues,
            &root.join("../antora-single-component/modules/ROOT/pages/index.adoc"),
        );
        assert!(outside.is_empty(), "{outside:?}");
    }
}
