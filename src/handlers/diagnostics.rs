use std::{collections::BTreeSet, path::Path};

use adoc_antora::{
    parse_resource_id, AntoraCatalog, AntoraResolver, ResolutionError, ResolutionResult,
};
use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, ReferenceKind, SourceRange};
use adoc_index::{workspace_diagnostics, WorkspaceIndex};

#[must_use]
pub fn diagnostics(index: &WorkspaceIndex, antora: &AntoraCatalog, path: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = workspace_diagnostics(index, path);
    let mut seen = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.range))
        .collect::<BTreeSet<_>>();
    let Some(file) = index.file(path) else {
        return diagnostics;
    };
    let Some(context) = antora.context_for_path(path) else {
        return diagnostics;
    };

    {
        let mut output = DiagnosticOutput {
            diagnostics: &mut diagnostics,
            seen: &mut seen,
        };
        for reference in &file.document.references {
            if reference.kind != ReferenceKind::Xref
                || !is_explicit_antora_target(&reference.target)
            {
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
            diagnose_resolution(
                AntoraResolver::resolve(antora, &id, &context),
                target,
                anchor,
                index,
                reference.range,
                &mut output,
            );
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
}
