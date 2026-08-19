//! Preparation of document source for rendering.
//!
//! Antora resolves family-qualified includes such as `include::partial$note.adoc[]`
//! through its own Asciidoctor extensions. A stock `asciidoctor` knows nothing about
//! them and emits "Unresolved directive". Because `adoc-ls` already carries the Antora
//! catalogue, it can rewrite those targets to absolute paths before handing the source
//! to any renderer — which works for every backend, not just Asciidoctor.

use std::path::Path;

use adoc_antora::{parse_resource_id, AntoraCatalog, AntoraResolver};
use adoc_core::Document;

/// `document`'s text with resolvable Antora includes rewritten to absolute paths.
///
/// Conservative by design: an include that is not family-qualified, or that does not
/// resolve, is left exactly as the author wrote it. Rewriting only the outermost
/// document means an Antora include *inside* an included partial is still unresolved;
/// resolving those needs the renderer to call back for each nested file.
#[must_use]
pub fn source_for_render(
    document: &Document,
    antora: &AntoraCatalog,
    current_path: &Path,
) -> String {
    let Some(context) = antora.context_for_path(current_path) else {
        return document.text.clone();
    };

    let mut source = document.text.clone();

    // Rewrite back to front so that earlier ranges stay valid as the text changes.
    for include in document.includes.iter().rev() {
        if !include.target.contains('$') {
            continue;
        }
        let Ok(id) = parse_resource_id(&include.target) else {
            continue;
        };
        let Ok(resource) = AntoraResolver::resolve(antora, &id, &context) else {
            continue;
        };

        let attributes = include.attributes.as_deref().unwrap_or_default();
        let replacement = format!("include::{}[{attributes}]", resource.source_path.display());
        let range = include.range.start..include.range.end;
        if source.get(range.clone()).is_some() {
            source.replace_range(range, &replacement);
        }
    }

    source
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use adoc_antora::discover_antora_workspace;
    use adoc_parser::parse;

    /// Normalised, so expectations match the catalogue's own normalised source paths.
    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repository root")
            .join("tests/fixtures/antora-single-component")
    }

    fn catalog() -> AntoraCatalog {
        discover_antora_workspace(&[fixture_root()])
            .expect("discover fixture")
            .catalog
    }

    fn rendered(path: &Path, text: &str) -> String {
        let parsed = parse(&format!("file://{}", path.display()), text);
        source_for_render(&parsed.document, &catalog(), path)
    }

    #[test]
    fn rewrites_a_family_qualified_include_to_an_absolute_path() {
        let page = fixture_root().join("modules/security/pages/authentication.adoc");
        let partial = fixture_root().join("modules/security/partials/token-note.adoc");

        let source = rendered(
            &page,
            "= Authentication\n\ninclude::partial$token-note.adoc[]\n",
        );

        assert_eq!(
            source,
            format!("= Authentication\n\ninclude::{}[]\n", partial.display())
        );
    }

    #[test]
    fn preserves_include_attributes() {
        let page = fixture_root().join("modules/security/pages/authentication.adoc");

        let source = rendered(&page, "include::partial$token-note.adoc[leveloffset=+1]\n");

        assert!(source.contains("[leveloffset=+1]"), "{source}");
        assert!(!source.contains("partial$"), "{source}");
    }

    #[test]
    fn leaves_plain_relative_includes_untouched() {
        let page = fixture_root().join("modules/security/pages/authentication.adoc");
        let text = "include::../partials/token-note.adoc[]\n";

        assert_eq!(rendered(&page, text), text);
    }

    #[test]
    fn leaves_an_unresolvable_antora_include_untouched() {
        let page = fixture_root().join("modules/security/pages/authentication.adoc");
        let text = "include::partial$missing.adoc[]\n";

        assert_eq!(rendered(&page, text), text);
    }

    #[test]
    fn rewrites_every_include_without_disturbing_later_offsets() {
        let page = fixture_root().join("modules/security/pages/authentication.adoc");
        let partial = fixture_root().join("modules/security/partials/token-note.adoc");

        let source = rendered(
            &page,
            "include::partial$token-note.adoc[]\n\ntext\n\ninclude::partial$token-note.adoc[]\n",
        );

        assert_eq!(
            source,
            format!(
                "include::{0}[]\n\ntext\n\ninclude::{0}[]\n",
                partial.display()
            )
        );
    }

    #[test]
    fn a_document_outside_an_antora_module_is_unchanged() {
        let text = "include::partial$token-note.adoc[]\n";
        let path = PathBuf::from("/elsewhere/guide.adoc");

        assert_eq!(rendered(&path, text), text);
    }
}
