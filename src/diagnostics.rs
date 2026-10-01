use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use adoc_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, Document, ReferenceKind, SourceRange,
};

use crate::{workspace::normalize_path, WorkspaceIndex};

#[must_use]
pub fn workspace_diagnostics(index: &WorkspaceIndex, path: &Path) -> Vec<Diagnostic> {
    let Some(file) = index.file(path) else {
        return Vec::new();
    };
    let document = &file.document;
    let mut diagnostics = document.diagnostics.clone();
    let mut seen = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.range))
        .collect::<BTreeSet<_>>();

    for reference in &document.references {
        if is_dynamic_target(&reference.target) {
            continue;
        }

        match reference.kind {
            ReferenceKind::LocalAnchor => {
                if index.resolve_anchor(path, &reference.target).is_none() {
                    push_unique(
                        &mut diagnostics,
                        &mut seen,
                        DiagnosticCode::UnresolvedAnchor,
                        format!("Unresolved AsciiDoc anchor: {}", reference.target),
                        reference.range,
                    );
                }
            }
            ReferenceKind::Xref => {
                diagnose_xref(
                    index,
                    path,
                    &reference.target,
                    reference.range,
                    &mut diagnostics,
                    &mut seen,
                );
            }
            ReferenceKind::Link | ReferenceKind::Attribute => {}
        }
    }

    for include in &document.includes {
        let Some(target_path) = resolve_include_target(document, path, &include.target) else {
            continue;
        };
        if !target_path.exists() && index.file(&target_path).is_none() {
            push_unique(
                &mut diagnostics,
                &mut seen,
                DiagnosticCode::UnresolvedInclude,
                format!("Unresolved AsciiDoc include target: {}", include.target),
                include.range,
            );
        }
    }

    // Images resolve against the top-level document, so a file composed into another cannot
    // judge its own.
    if index.includers(path).next().is_none() {
        for image in &document.images {
            let Some(target_path) = resolve_image_target(document, path, &image.target) else {
                continue;
            };
            if !target_path.exists() && index.file(&target_path).is_none() {
                push_unique(
                    &mut diagnostics,
                    &mut seen,
                    DiagnosticCode::UnresolvedImage,
                    format!("Unresolved AsciiDoc image target: {}", image.target),
                    image.range,
                );
            }
        }
    }

    diagnostics.sort_by_key(|diagnostic| (diagnostic.range, diagnostic.code));
    diagnostics
}

#[must_use]
pub fn resolve_include_target(
    document: &Document,
    current_path: &Path,
    target: &str,
) -> Option<PathBuf> {
    if is_external_or_antora(target) {
        return None;
    }
    let target = substitute_attributes(document, target)?;
    let parent = current_path.parent().unwrap_or_else(|| Path::new(""));
    Some(normalize_path(&parent.join(target)))
}

/// The file a block image names in a plain workspace, or `None` when the index cannot judge
/// the target: a URL, a `data:` URI, an Antora resource ID, or an attribute the document does
/// not declare.
///
/// Asciidoctor resolves images against `imagesdir`, which defaults to the document's
/// directory, so a declared `imagesdir` is joined in front of the target.
#[must_use]
pub fn resolve_image_target(
    document: &Document,
    current_path: &Path,
    target: &str,
) -> Option<PathBuf> {
    // A colon covers `://`, `data:` and Antora coordinates alike.
    if target.contains(':') || target.contains('$') {
        return None;
    }
    let target = substitute_attributes(document, target)?;
    let parent = current_path.parent().unwrap_or_else(|| Path::new(""));
    let base = match document
        .attributes
        .iter()
        .rev()
        .find(|attribute| attribute.name == "imagesdir")
    {
        Some(attribute) => {
            let directory =
                substitute_attributes(document, attribute.value.as_deref().unwrap_or(""))?;
            if directory.contains(':') {
                return None;
            }
            parent.join(directory)
        }
        None => parent.to_path_buf(),
    };
    Some(normalize_path(&base.join(target)))
}

fn diagnose_xref(
    index: &WorkspaceIndex,
    current_path: &Path,
    target: &str,
    range: SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
    seen: &mut BTreeSet<(DiagnosticCode, SourceRange)>,
) {
    if is_external_or_antora(target) {
        return;
    }

    let (file_target, anchor) = target
        .split_once('#')
        .map_or((target, None), |(file, anchor)| (file, Some(anchor)));
    let target_path = if file_target.is_empty() {
        normalize_path(current_path)
    } else {
        let parent = current_path.parent().unwrap_or_else(|| Path::new(""));
        normalize_path(&parent.join(file_target))
    };

    if !file_target.is_empty() && index.file(&target_path).is_none() {
        if !target_path.exists() {
            push_unique(
                diagnostics,
                seen,
                DiagnosticCode::UnresolvedXrefFile,
                format!("Unresolved AsciiDoc xref target: {file_target}"),
                range,
            );
        }
        return;
    }

    if let Some(anchor) = anchor.filter(|anchor| !anchor.is_empty()) {
        if index.resolve_anchor(&target_path, anchor).is_none() {
            push_unique(
                diagnostics,
                seen,
                DiagnosticCode::UnresolvedAnchor,
                format!("Unresolved AsciiDoc anchor: {anchor}"),
                range,
            );
        }
    }
}

fn substitute_attributes(document: &Document, target: &str) -> Option<String> {
    let mut output = String::with_capacity(target.len());
    let mut remainder = target;

    while let Some(open) = remainder.find('{') {
        output.push_str(&remainder[..open]);
        let after_open = &remainder[open + 1..];
        let close = after_open.find('}')?;
        let name = &after_open[..close];
        let value = document
            .attributes
            .iter()
            .rev()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| attribute.value.as_deref())?;
        output.push_str(value);
        remainder = &after_open[close + 1..];
    }

    if remainder.contains('}') {
        return None;
    }
    output.push_str(remainder);
    Some(output)
}

fn is_dynamic_target(target: &str) -> bool {
    target.contains('{') || target.contains('}')
}

fn is_external_or_antora(target: &str) -> bool {
    target.contains("://")
        || target.starts_with("mailto:")
        || target.contains('$')
        || target
            .split('#')
            .next()
            .is_some_and(|file| file.contains(':'))
}

fn push_unique(
    diagnostics: &mut Vec<Diagnostic>,
    seen: &mut BTreeSet<(DiagnosticCode, SourceRange)>,
    code: DiagnosticCode,
    message: String,
    range: SourceRange,
) {
    if seen.insert((code, range)) {
        diagnostics.push(Diagnostic {
            code,
            severity: DiagnosticSeverity::Warning,
            message,
            range,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use adoc_core::DiagnosticCode;

    use crate::{resolve_image_target, workspace_diagnostics, WorkspaceIndex};

    #[test]
    fn reports_definitely_missing_local_targets() {
        let path = Path::new("docs/index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            path,
            "= Guide\n\nSee <<missing>> and xref:missing.adoc[].\ninclude::missing.adoc[]\n",
        );

        let diagnostics = workspace_diagnostics(&index, path);
        let codes = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&DiagnosticCode::UnresolvedAnchor));
        assert!(codes.contains(&DiagnosticCode::UnresolvedXrefFile));
        assert!(codes.contains(&DiagnosticCode::UnresolvedInclude));
    }

    #[test]
    fn skips_dynamic_and_antora_targets() {
        let path = Path::new("docs/index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            path,
            ":partialsdir: partials\n\ninclude::{unknown}/file.adoc[]\ninclude::partial$note.adoc[]\nxref:security:page.adoc[]\n",
        );

        assert!(workspace_diagnostics(&index, path).is_empty());
    }

    #[test]
    fn resolves_references_to_implicit_section_ids() {
        let path = Path::new("docs/index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            path,
            "= Guide\n\nSee <<Logging Levels>>, <<logging-levels>> and <<_logging_levels>>.\n\n== Logging Levels\n",
        );

        assert_eq!(workspace_diagnostics(&index, path), Vec::new());
    }

    #[test]
    fn matches_section_ids_that_differ_only_in_punctuation() {
        let path = Path::new("docs/index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            path,
            "= Guide\n\nSee <<flow-d--cancellation-unscheduling-by-external-id>>\nand <<the-fun_parse_weeks-function>>.\n\n== Flow D - Cancellation (Unscheduling) by External ID\n\n== The fun_parse_weeks function\n",
        );

        assert_eq!(workspace_diagnostics(&index, path), Vec::new());
    }

    #[test]
    fn matches_section_ids_whose_punctuation_was_dropped_rather_than_replaced() {
        let path = Path::new("docs/index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_source(
            path,
            "= Guide\n\nSee <<version-0-6-0-supersededroom-in-the-key>>.\n\n== Version 0.6.0 (superseded -- room in the key)\n",
        );

        assert_eq!(workspace_diagnostics(&index, path), Vec::new());
    }

    #[test]
    fn resolves_known_attribute_in_include_target() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/includes");
        let index_path = root.join("index.adoc");
        let mut index = WorkspaceIndex::new();
        index.index_roots(&[root]).unwrap();
        index.index_source(
            &index_path,
            ":partialsdir: partials\n\ninclude::{partialsdir}/intro.adoc[]\n",
        );

        assert!(workspace_diagnostics(&index, &index_path).is_empty());
    }

    #[test]
    fn resolves_image_targets_against_the_document_and_its_imagesdir() {
        let path = Path::new("docs/guide.adoc");
        let plain = adoc_parser::parse("file:///docs/guide.adoc", "= Guide\n").document;
        let with_dir = adoc_parser::parse(
            "file:///docs/guide.adoc",
            ":base: assets\n:imagesdir: {base}/img\n",
        )
        .document;

        assert_eq!(
            resolve_image_target(&plain, path, "a.png"),
            Some(PathBuf::from("docs/a.png"))
        );
        assert_eq!(
            resolve_image_target(&plain, path, "/abs/a.png"),
            Some(PathBuf::from("/abs/a.png"))
        );
        assert_eq!(
            resolve_image_target(&with_dir, path, "a.png"),
            Some(PathBuf::from("docs/assets/img/a.png"))
        );
    }

    #[test]
    fn leaves_image_targets_it_cannot_judge_unresolved() {
        let path = Path::new("docs/guide.adoc");
        let document = adoc_parser::parse("file:///docs/guide.adoc", "= Guide\n").document;
        let remote_dir = adoc_parser::parse(
            "file:///docs/guide.adoc",
            ":imagesdir: https://cdn.example\n",
        )
        .document;

        for target in [
            "https://example.com/a.png",
            "data:image/png;base64,AAAA",
            "{undeclared}/a.png",
            "ROOT:a.png",
            "image$a.png",
        ] {
            assert_eq!(
                resolve_image_target(&document, path, target),
                None,
                "{target}"
            );
        }
        assert_eq!(resolve_image_target(&remote_dir, path, "a.png"), None);
    }

    #[test]
    fn reports_a_missing_image_only_in_files_nothing_includes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();

        let guide: Vec<_> = workspace_diagnostics(&index, &root.join("guide.adoc"))
            .into_iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::UnresolvedImage)
            .collect();
        assert_eq!(guide.len(), 1, "{guide:?}");
        assert!(guide[0].message.contains("missing.svg"));

        assert!(
            workspace_diagnostics(&index, &root.join("chapters/setup.adoc")).is_empty(),
            "an included file's images resolve against its includer"
        );
    }
}
