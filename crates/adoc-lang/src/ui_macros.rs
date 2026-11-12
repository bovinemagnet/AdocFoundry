//! A hint on each keyboard, button or menu macro where the `experimental` attribute is not set,
//! since Asciidoctor then shows the macro as text (#600).

use std::path::Path;

use adoc_core::{Diagnostic, DiagnosticCode, DiagnosticSeverity, UiMacro};
use adoc_index::normalize_path;

use crate::{
    effective::{resolve, resolve_through, top_pages},
    WorkspaceState,
};

const EXPERIMENTAL: &str = "experimental";

/// The hints for the file at `path`: one for each UI macro where `experimental` has no value,
/// read with the file as a page and, for a file pages include, in every such page.
pub(crate) fn ui_macro_hints(ws: &WorkspaceState, path: &Path) -> Vec<Diagnostic> {
    let path = normalize_path(path);
    let Some(file) = ws.index.file(&path) else {
        return Vec::new();
    };
    let document = &file.document;
    if document.ui_macros.is_empty() {
        return Vec::new();
    }
    let includers = top_pages(ws, &path);
    let unset_at = |found: &UiMacro| {
        let offset = found.range.start;
        resolve(&ws.index, &ws.antora, &path, document, EXPERIMENTAL, offset)
            .value()
            .is_none()
            && includers.iter().all(|page| {
                let Some(including) = ws.index.file(page) else {
                    return true;
                };
                resolve_through(
                    &ws.index,
                    &ws.antora,
                    page,
                    &including.document,
                    EXPERIMENTAL,
                    (&path, offset),
                )
                .is_none_or(|resolution| resolution.value().is_none())
            })
    };
    document
        .ui_macros
        .iter()
        .filter(|found| unset_at(found))
        .map(|found| Diagnostic {
            code: DiagnosticCode::UiMacroNotEnabled,
            severity: DiagnosticSeverity::Information,
            message: format!(
                "`{}:` shows as text: the keyboard, button and menu macros need the \
                 `experimental` attribute, which is not set here; set it in the page header \
                 (`:experimental:`), `antora.yml` or the playbook",
                found.name
            ),
            range: found.range,
            related: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use adoc_core::{DiagnosticCode, DiagnosticSeverity};
    use adoc_index::normalize_path;

    use crate::WorkspaceState;

    fn fixture(root: &str) -> PathBuf {
        normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/ui-macros")
                .join(root),
        )
    }

    /// The text each hint in `file` (under the root `root`) covers.
    fn hinted(root: &str, file: &str) -> Vec<String> {
        let mut ws = WorkspaceState::new();
        ws.index_roots(&[fixture(root)]).unwrap();
        let path = fixture(root).join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        crate::diagnostics(&ws, &path)
            .into_iter()
            .filter(|found| found.code == DiagnosticCode::UiMacroNotEnabled)
            .map(|found| {
                assert_eq!(found.severity, DiagnosticSeverity::Information);
                assert!(found.message.contains("experimental"), "{}", found.message);
                text[found.range.start..found.range.end].to_owned()
            })
            .collect()
    }

    const DOCS: &str = "docs/modules/ROOT";

    #[test]
    fn hints_at_ui_macros_where_experimental_is_not_set() {
        assert_eq!(
            hinted("bare", &format!("{DOCS}/pages/plain.adoc")),
            ["kbd:[Ctrl+S]", "btn:[OK]"]
        );
        // After the page unsets it.
        assert_eq!(
            hinted("bare", &format!("{DOCS}/pages/unset.adoc")),
            ["kbd:[B]"]
        );
        // In a partial whose only page leaves it unset.
        assert_eq!(
            hinted("bare", &format!("{DOCS}/partials/loose.adoc")),
            ["menu:File[Close]"]
        );
    }

    #[test]
    fn no_hint_where_anything_sets_experimental() {
        for (root, file) in [
            ("bare", format!("{DOCS}/pages/header.adoc")),
            ("bare", format!("{DOCS}/pages/from-include.adoc")),
            // Its page sets it before the include.
            ("bare", format!("{DOCS}/partials/keys.adoc")),
            // `antora.yml` sets it.
            ("bare", "keys/modules/ROOT/pages/set.adoc".to_owned()),
            // The playbook sets it.
            ("playbook", format!("{DOCS}/pages/plain.adoc")),
        ] {
            assert!(hinted(root, &file).is_empty(), "{root}/{file}");
        }
    }
}
