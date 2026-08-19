//! Preparation of document source for rendering.
//!
//! Antora resolves family-qualified includes such as `include::partial$note.adoc[]`
//! through its own Asciidoctor extensions. A stock `asciidoctor` knows nothing about
//! them and emits "Unresolved directive". Because `adoc-ls` already carries the Antora
//! catalogue, it can rewrite those targets to absolute paths before rendering — which
//! works for every backend, not just Asciidoctor.
//!
//! An included partial may itself use family-qualified includes, and Asciidoctor reads
//! that partial from disk, so rewriting the outermost document is not enough. Files that
//! need rewriting are therefore copied into a scratch directory with every include made
//! absolute, and the including document points at the copy. Reading those copies is why
//! preview renders unsafe: they necessarily live outside the component's jail.

use std::{
    collections::hash_map::DefaultHasher,
    collections::HashSet,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use adoc_antora::{parse_resource_id, AntoraCatalog, AntoraContext, AntoraResolver};
use adoc_core::{Document, IncludeDirective};
use adoc_index::normalize_path;
use adoc_parser::parse;

/// `document`'s text with Antora includes rewritten so a stock renderer resolves them.
///
/// Conservative by design: an include that does not resolve, or a file that cannot be
/// read or copied, is left exactly as the author wrote it. `scratch` receives rewritten
/// copies of included files; nothing is written when no include needs rewriting.
#[must_use]
pub fn source_for_render(
    document: &Document,
    antora: &AntoraCatalog,
    current_path: &Path,
    scratch: &Path,
) -> String {
    let Some(context) = antora.context_for_path(current_path) else {
        return document.text.clone();
    };

    let mut visited = HashSet::new();
    visited.insert(normalize_path(current_path));

    let mut rewriter = Rewriter {
        antora,
        context: &context,
        scratch,
        visited,
    };
    rewriter.rewrite(&document.includes, &document.text, current_path, false)
}

struct Rewriter<'a> {
    antora: &'a AntoraCatalog,
    context: &'a AntoraContext,
    scratch: &'a Path,
    visited: HashSet<PathBuf>,
}

impl Rewriter<'_> {
    /// `text` with each resolvable include repointed.
    ///
    /// `absolutise_relative` is set for copied files: a copy no longer sits beside the
    /// files its relative includes name, so those must become absolute too. The document
    /// the author is editing stays where it is, so its relative includes are left alone.
    fn rewrite(
        &mut self,
        includes: &[IncludeDirective],
        text: &str,
        file_path: &Path,
        absolutise_relative: bool,
    ) -> String {
        let mut source = text.to_owned();

        // Back to front, so earlier ranges stay valid as the text changes.
        for include in includes.iter().rev() {
            let Some(resolved) = self.resolve(include, file_path, absolutise_relative) else {
                continue;
            };
            let target = self.prepare(&resolved);

            let attributes = include.attributes.as_deref().unwrap_or_default();
            let replacement = format!("include::{}[{attributes}]", target.display());
            let range = include.range.start..include.range.end;
            if source.get(range.clone()).is_some() {
                source.replace_range(range, &replacement);
            }
        }

        source
    }

    /// The file an include names, or `None` when it should be left untouched.
    fn resolve(
        &self,
        include: &IncludeDirective,
        file_path: &Path,
        absolutise_relative: bool,
    ) -> Option<PathBuf> {
        if include.target.contains('$') {
            let id = parse_resource_id(&include.target).ok()?;
            let resource = AntoraResolver::resolve(self.antora, &id, self.context).ok()?;
            return Some(normalize_path(&resource.source_path));
        }

        if !absolutise_relative || include.target.starts_with('{') {
            return None;
        }
        let candidate = file_path.parent()?.join(&include.target);
        let candidate = normalize_path(&candidate);
        candidate.is_file().then_some(candidate)
    }

    /// The path an including document should name for `path`.
    ///
    /// A file whose own includes all resolve as written needs no copy; one that does not
    /// is rewritten into `scratch`. Anything unreadable, already being visited, or that
    /// cannot be written falls back to the original path.
    fn prepare(&mut self, path: &Path) -> PathBuf {
        if !self.visited.insert(path.to_path_buf()) {
            return path.to_path_buf();
        }

        let result = self.rewrite_file(path);
        self.visited.remove(path);
        result
    }

    fn rewrite_file(&mut self, path: &Path) -> PathBuf {
        let Ok(text) = fs::read_to_string(path) else {
            return path.to_path_buf();
        };
        let parsed = parse(&format!("file://{}", path.display()), &text);
        if parsed.document.includes.is_empty() {
            return path.to_path_buf();
        }

        let rewritten = self.rewrite(&parsed.document.includes, &text, path, true);
        if rewritten == text {
            return path.to_path_buf();
        }

        let copy = self.scratch.join(copy_name(path));
        if fs::create_dir_all(self.scratch).is_err() || fs::write(&copy, rewritten).is_err() {
            return path.to_path_buf();
        }
        copy
    }
}

/// A short, deterministic name for the rewritten copy of `path`.
fn copy_name(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("include");
    format!("{:016x}-{stem}.adoc", hasher.finish())
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
        rendered_in(path, text, &catalog(), &scratch("default"))
    }

    fn rendered_in(path: &Path, text: &str, antora: &AntoraCatalog, scratch: &Path) -> String {
        let parsed = parse(&format!("file://{}", path.display()), text);
        source_for_render(&parsed.document, antora, path, scratch)
    }

    /// A directory unique to one test; these run in parallel.
    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("adoc-ls-scratch-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    fn nested_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repository root")
            .join("tests/fixtures/antora-nested-includes")
    }

    fn nested_catalog() -> AntoraCatalog {
        discover_antora_workspace(&[nested_root()])
            .expect("discover nested fixture")
            .catalog
    }

    #[test]
    fn rewrites_a_family_qualified_include_inside_an_included_partial() {
        let page = nested_root().join("modules/ROOT/pages/outer.adoc");
        let scratch = scratch("nested");

        let source = rendered_in(
            &page,
            "= Outer\n\ninclude::partial$level-one.adoc[]\n",
            &nested_catalog(),
            &scratch,
        );

        // The page points at a rewritten copy, not the original partial.
        let target = include_target(&source);
        assert!(target.starts_with(&scratch), "{}", target.display());

        // That copy resolves the nested include to an absolute path.
        let copy = std::fs::read_to_string(&target).expect("read copy");
        assert!(!copy.contains("partial$"), "{copy}");
        assert!(copy.contains("level-two.adoc"), "{copy}");
    }

    #[test]
    fn a_partial_needing_no_rewrite_is_referenced_in_place() {
        let page = nested_root().join("modules/ROOT/pages/outer.adoc");
        let partial = nested_root().join("modules/ROOT/partials/level-two.adoc");
        let scratch = scratch("in-place");

        let source = rendered_in(
            &page,
            "= Outer\n\ninclude::partial$level-two.adoc[]\n",
            &nested_catalog(),
            &scratch,
        );

        assert_eq!(include_target(&source), partial);
    }

    #[test]
    fn a_cycle_of_partials_terminates() {
        let page = nested_root().join("modules/ROOT/pages/cyclic.adoc");
        let scratch = scratch("cycle");

        let source = rendered_in(
            &page,
            "= Cyclic\n\ninclude::partial$cycle-a.adoc[]\n",
            &nested_catalog(),
            &scratch,
        );

        assert!(source.contains("include::"), "{source}");
    }

    /// The path inside the first `include::...[]` directive.
    fn include_target(source: &str) -> PathBuf {
        let start = source.find("include::").expect("an include") + "include::".len();
        let end = start + source[start..].find('[').expect("an attribute list");
        PathBuf::from(&source[start..end])
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
