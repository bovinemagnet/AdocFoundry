use std::{collections::HashSet, path::Path};

use adoc_antora::{AntoraCatalog, AntoraContext, ResourceFamily};

use adoc_core::{asciidoctor_id, canonical_id, Document, SourceRange};
use adoc_index::{list_directory, WorkspaceIndex};
use adoc_parser::{completion_context, CompletionKind};

use crate::handlers::definition::reference_target_path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateKind {
    Page,
    Resource,
    Family,
    Directory,
    Anchor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// Inserted verbatim over `range` when the candidate is accepted.
    pub label: String,
    pub detail: Option<String>,
    pub sort_text: String,
    pub kind: CandidateKind,
    pub range: SourceRange,
}

/// Candidates for the construct the cursor sits inside, or an empty list.
///
/// Never returns an error: an unresolvable context, an unknown target and an unreadable
/// directory all mean "no suggestions", which matches how navigation and diagnostics
/// behave here.
#[must_use]
pub fn completion_at_offset(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
    offset: usize,
) -> Vec<Candidate> {
    let Some(context) = completion_context(&document.text, offset) else {
        return Vec::new();
    };

    let candidates = match &context.kind {
        CompletionKind::LocalAnchor => {
            anchor_candidates(index, antora, current_path, context.range)
        }
        CompletionKind::XrefAnchor { target } => {
            match reference_target_path(index, antora, current_path, target) {
                Some(path) => anchor_candidates(index, antora, &path, context.range),
                None => Vec::new(),
            }
        }
        CompletionKind::XrefTarget => match antora.context_for_path(current_path) {
            Some(antora_context) => {
                antora_page_candidates(index, antora, &antora_context, context.range)
            }
            None => path_candidates(index, current_path, &context.prefix, context.range),
        },
        CompletionKind::IncludeTarget => match antora.context_for_path(current_path) {
            Some(antora_context) => antora_include_candidates(
                index,
                antora,
                &antora_context,
                &context.prefix,
                context.range,
            )
            .unwrap_or_else(|| {
                path_candidates(index, current_path, &context.prefix, context.range)
            }),
            None => path_candidates(index, current_path, &context.prefix, context.range),
        },
        CompletionKind::ImageTarget => match antora.context_for_path(current_path) {
            Some(antora_context) => antora_family_candidates(
                index,
                antora,
                &antora_context,
                ResourceFamily::Image,
                "",
                context.range,
            ),
            None => path_candidates(index, current_path, &context.prefix, context.range),
        },
    };

    filter_by_prefix(candidates, &context.prefix)
}

/// One candidate per section, plus one per anchor that is not attached to a heading.
///
/// `Section::implicit_ids` deliberately registers several id forms per section in the index —
/// the title, Antora's generated form, Asciidoctor's `_`-prefixed form — so that *resolution*
/// is forgiving about how a reference is written. That makes it the wrong input for
/// *enumeration*: collapsing those forms back down, whether by the location they point at or
/// by a punctuation-insensitive spelling, either merges two distinct sections that happen to
/// normalise the same way (`Set-up` and `Setup`), or, when a section declares an explicit
/// anchor, still offers Asciidoctor's generated id alongside it — an id the explicit anchor
/// actually suppresses, so completion would offer a link that does not resolve.
///
/// Candidates are built from the document itself instead, which already carries exactly one
/// row per section in the author's own spelling: `Section::id` where the author wrote an
/// explicit anchor, and a generated id where they did not — never a suppressed one. The
/// generated form depends on where `path` sits: Antora's `canonical_id` inside a module, since
/// that is what an Antora-rendered site resolves, and plain Asciidoctor's `asciidoctor_id`
/// everywhere else, since that is what `asciidoctor` itself would generate. Anchors that are
/// not attached to any heading (a `[[bibliography-entry]]` in body text) are offered too, but
/// only once: an anchor immediately above a heading is *also* that section's `id`, so it is
/// skipped here to avoid duplicating the section's own candidate.
fn anchor_candidates(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    path: &Path,
    range: SourceRange,
) -> Vec<Candidate> {
    let Some(document) = index.file(path).map(|entry| &entry.document) else {
        return Vec::new();
    };

    let generate_id: fn(&str) -> String = if antora.context_for_path(path).is_some() {
        canonical_id
    } else {
        asciidoctor_id
    };

    let mut candidates: Vec<Candidate> = document
        .sections
        .iter()
        .map(|section| {
            section
                .id
                .clone()
                .unwrap_or_else(|| generate_id(&section.title))
        })
        .map(|label| candidate(label, range))
        .collect();

    let section_ids: HashSet<&str> = document
        .sections
        .iter()
        .filter_map(|section| section.id.as_deref())
        .collect();

    candidates.extend(
        document
            .anchors
            .iter()
            .filter(|anchor| !section_ids.contains(anchor.id.as_str()))
            .map(|anchor| candidate(anchor.id.clone(), range)),
    );

    candidates.sort_by(|a, b| a.label.cmp(&b.label));
    candidates
}

fn candidate(label: String, range: SourceRange) -> Candidate {
    let sort_text = format!("0{label}");
    Candidate {
        label,
        detail: None,
        sort_text,
        kind: CandidateKind::Anchor,
        range,
    }
}

/// Narrow the list to what the author has typed, case-insensitively and by substring.
///
/// The response is marked incomplete, so the client asks again as the prefix grows and
/// this runs against the longer prefix each time.
fn filter_by_prefix(candidates: Vec<Candidate>, prefix: &str) -> Vec<Candidate> {
    if prefix.is_empty() {
        return candidates;
    }
    let needle = prefix.to_lowercase();
    candidates
        .into_iter()
        .filter(|candidate| candidate.label.to_lowercase().contains(&needle))
        .collect()
}

fn antora_page_candidates(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    range: SourceRange,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    // The current module's pages are written bare, the way Antora authors write them.
    for resource in antora.resources_in(
        &context.component,
        context.version.as_deref(),
        &context.module,
        ResourceFamily::Page,
    ) {
        let label = resource
            .coordinate
            .relative_path
            .to_string_lossy()
            .into_owned();
        candidates.push(Candidate {
            detail: title_of(index, &resource.source_path),
            sort_text: format!("0{label}"),
            kind: CandidateKind::Page,
            range,
            label,
        });
    }

    // Every other module of the same component, module-qualified and ranked below.
    for module in antora.modules_of(&context.component, context.version.as_deref()) {
        if module.name == context.module {
            continue;
        }
        for resource in antora.resources_in(
            &context.component,
            context.version.as_deref(),
            &module.name,
            ResourceFamily::Page,
        ) {
            let label = format!(
                "{}:{}",
                module.name,
                resource.coordinate.relative_path.to_string_lossy()
            );
            candidates.push(Candidate {
                detail: title_of(index, &resource.source_path),
                sort_text: format!("1{label}"),
                kind: CandidateKind::Page,
                range,
                label,
            });
        }
    }

    candidates
}

/// Candidates for `include::`, given a context already resolved by the caller.
///
/// Returns `None` only when the typed family name (before `$`) does not parse as a known
/// `ResourceFamily`, so the caller falls back to the plain path completer for a target such
/// as `include::bogus$`.
fn antora_include_candidates(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    prefix: &str,
    range: SourceRange,
) -> Option<Vec<Candidate>> {
    let Some((family_name, _)) = prefix.split_once('$') else {
        // No family chosen yet: offer the families themselves.
        return Some(
            ResourceFamily::ALL
                .iter()
                .map(|family| {
                    let label = format!("{family}$");
                    Candidate {
                        detail: None,
                        sort_text: format!("0{label}"),
                        kind: CandidateKind::Family,
                        range,
                        label,
                    }
                })
                .collect(),
        );
    };

    let family = family_name.parse::<ResourceFamily>().ok()?;
    Some(antora_family_candidates(
        index,
        antora,
        context,
        family,
        &format!("{family}$"),
        range,
    ))
}

/// Candidates for one Antora family, given a context already resolved by the caller.
fn antora_family_candidates(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    context: &AntoraContext,
    family: ResourceFamily,
    label_prefix: &str,
    range: SourceRange,
) -> Vec<Candidate> {
    antora
        .resources_in(
            &context.component,
            context.version.as_deref(),
            &context.module,
            family,
        )
        .map(|resource| {
            let label = format!(
                "{label_prefix}{}",
                resource.coordinate.relative_path.to_string_lossy()
            );
            Candidate {
                detail: title_of(index, &resource.source_path),
                sort_text: format!("0{label}"),
                kind: CandidateKind::Resource,
                range,
                label,
            }
        })
        .collect()
}

fn title_of(index: &WorkspaceIndex, path: &Path) -> Option<String> {
    index
        .file(path)?
        .document
        .title
        .as_ref()
        .map(|title| title.text.clone())
}

/// Relative-path candidates for a workspace with no Antora catalog.
///
/// The typed prefix is split at its last `/`: the left half names the directory to look
/// in, and is kept on every label so that accepting a candidate leaves a valid path. That
/// is also why `../shared/` needs no special handling.
fn path_candidates(
    index: &WorkspaceIndex,
    current_path: &Path,
    prefix: &str,
    range: SourceRange,
) -> Vec<Candidate> {
    let (typed_directory, _) = prefix.rsplit_once('/').unwrap_or(("", prefix));
    let label_prefix = if typed_directory.is_empty() {
        String::new()
    } else {
        format!("{typed_directory}/")
    };
    let Some(base) = current_path.parent() else {
        return Vec::new();
    };
    // `PathBuf::join` treats an argument starting with a separator as absolute and
    // discards `base` entirely, unlike Node's `path.join`, which ignores a leading
    // separator on a later segment; `adoc-antora/src/resource_id.rs` already has to make
    // the same correction when parsing a resource id. Strip it so a prefix that merely
    // looks absolute (`include::/etc/`) still resolves relative to the current document
    // instead of escaping the workspace; the label above keeps what was typed, leading
    // separator included, so the inserted text is unaffected.
    let relative_directory = typed_directory.trim_start_matches('/');
    let directory = adoc_index::normalize_path(&base.join(relative_directory));

    let mut candidates = Vec::new();
    for entry in list_directory(&directory) {
        let label = format!(
            "{label_prefix}{}{}",
            entry.name,
            if entry.is_directory { "/" } else { "" }
        );
        let detail = if entry.is_directory {
            None
        } else {
            title_of(index, &directory.join(&entry.name))
        };
        candidates.push(Candidate {
            detail,
            // Directories sort below files: the file is usually what is wanted.
            sort_text: format!("{}{label}", u8::from(entry.is_directory)),
            kind: if entry.is_directory {
                CandidateKind::Directory
            } else {
                CandidateKind::Page
            },
            range,
            label,
        });
    }
    candidates
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use adoc_antora::AntoraCatalog;
    use adoc_index::WorkspaceIndex;
    use adoc_parser::parse;

    use super::completion_at_offset;

    #[test]
    fn completes_anchors_declared_in_the_current_file() {
        let path = Path::new("/docs/guide.adoc");
        let text = "[[intro]]\n== Intro\n\n[[detail]]\n== Detail\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert!(labels.contains(&"intro".to_owned()));
        assert!(labels.contains(&"detail".to_owned()));
    }

    #[test]
    fn offers_one_candidate_per_section_not_one_per_id_form() {
        // The index registers `Detail`, `detail`, `_detail` and `detail` again so that
        // resolution is forgiving. Completion must collapse them to the anchor as written.
        let path = Path::new("/docs/guide.adoc");
        let text = "[[intro]]\n== Intro\n\n[[detail]]\n== Detail\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["detail".to_owned(), "intro".to_owned()]);
    }

    #[test]
    fn prefers_asciidoctor_s_generated_id_outside_antora() {
        // Plain Asciidoctor's defaults are `idprefix=_` and `idseparator=_`, unlike
        // Antora's `idprefix: ''`/`idseparator: '-'`, so a workspace with no Antora
        // catalogue must offer the `_`-prefixed form real Asciidoctor would resolve.
        let path = Path::new("/docs/guide.adoc");
        let text = "== Getting Started\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["_getting_started".to_owned()]);
    }

    #[test]
    fn filters_anchors_by_what_has_been_typed() {
        let path = Path::new("/docs/guide.adoc");
        let text = "[[intro]]\n== Intro\n\n[[detail]]\n== Detail\n\nSee <<det";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["detail".to_owned()]);
    }

    #[test]
    fn offers_distinct_candidates_for_sections_that_normalise_to_the_same_alphanumeric_form() {
        let path = Path::new("/docs/guide.adoc");
        let text = "== Set-up\n\n== Setup\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["_set_up".to_owned(), "_setup".to_owned()]);
    }

    #[test]
    fn an_explicit_anchor_suppresses_the_generated_id() {
        let path = Path::new("/docs/guide.adoc");
        let text = "[[install-linux]]\n== Installing on Linux\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["install-linux".to_owned()]);
    }

    #[test]
    fn offers_a_standalone_anchor_not_attached_to_any_heading() {
        let path = Path::new("/docs/guide.adoc");
        let text = "[[bibliography-entry]]\nSome reference text.\n\nSee <<";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert!(labels.contains(&"bibliography-entry".to_owned()));
    }

    #[test]
    fn completes_anchors_of_another_file_after_a_hash() {
        let index_path = Path::new("/docs/index.adoc");
        let other_path = Path::new("/docs/other.adoc");
        let text = "= Index\n\nSee xref:other.adoc#";
        let mut index = WorkspaceIndex::new();
        index.index_source(index_path, text);
        index.index_source(other_path, "[[details]]\n== Details\n");
        let document = parse("file:///docs/index.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(
            &index,
            &AntoraCatalog::new(),
            index_path,
            &document,
            text.len(),
        )
        .into_iter()
        .map(|candidate| candidate.label)
        .collect();

        assert!(labels.contains(&"details".to_owned()));
    }

    #[test]
    fn returns_nothing_where_there_is_no_context() {
        let path = Path::new("/docs/guide.adoc");
        let text = "= Guide\n\nOrdinary prose.\n";
        let mut index = WorkspaceIndex::new();
        index.index_source(path, text);
        let document = parse("file:///docs/guide.adoc", text).document;

        assert!(
            completion_at_offset(&index, &AntoraCatalog::new(), path, &document, text.len())
                .is_empty()
        );
    }

    use adoc_antora::discover_antora_workspace;

    fn antora_fixture() -> (WorkspaceIndex, AntoraCatalog, std::path::PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        let catalog = discover_antora_workspace(std::slice::from_ref(&root))
            .expect("discover fixture")
            .catalog;
        (index, catalog, root)
    }

    #[test]
    fn prefers_antora_s_generated_id_for_a_section_with_no_explicit_anchor() {
        // Inside an Antora module the generated id must match Antora's own convention
        // (`idprefix: ''`, `idseparator: '-'`), not plain Asciidoctor's `_`-prefixed form.
        let (mut index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/getting-started.adoc");
        let text = "== Getting Started\n\nSee <<";
        index.index_source(&path, text);
        let document = parse("file:///getting-started.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();

        assert_eq!(labels, vec!["getting-started".to_owned()]);
    }

    #[test]
    fn an_explicit_anchor_suppresses_the_generated_id_inside_an_antora_module() {
        let (mut index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/getting-started.adoc");
        let text = "[[install-linux]]\n== Installing on Linux\n\nSee <<";
        index.index_source(&path, text);
        let document = parse("file:///getting-started.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();

        assert_eq!(labels, vec!["install-linux".to_owned()]);
    }

    #[test]
    fn ranks_the_current_module_above_other_modules() {
        let (index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\nSee xref:";
        let document = parse("file:///index.adoc", text).document;

        let candidates = completion_at_offset(&index, &catalog, &path, &document, text.len());
        let labels: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.label.clone())
            .collect();

        assert!(
            labels.contains(&"index.adoc".to_owned()),
            "the current module's pages are offered as bare ids: {labels:?}"
        );
        assert!(
            labels.contains(&"security:authentication.adoc".to_owned()),
            "other modules are offered module-qualified: {labels:?}"
        );
        let bare = candidates
            .iter()
            .find(|candidate| candidate.label == "index.adoc")
            .expect("bare id");
        let qualified = candidates
            .iter()
            .find(|candidate| candidate.label == "security:authentication.adoc")
            .expect("qualified id");
        assert!(
            bare.sort_text < qualified.sort_text,
            "the current module must sort first: {} vs {}",
            bare.sort_text,
            qualified.sort_text
        );
    }

    #[test]
    fn offers_the_family_prefixes_before_a_dollar_is_typed() {
        let (index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\ninclude::";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();

        for family in ["page$", "partial$", "example$", "image$", "attachment$"] {
            assert!(
                labels.contains(&family.to_owned()),
                "`{family}` must be offered: {labels:?}"
            );
        }
    }

    #[test]
    fn offers_a_family_s_resources_once_the_dollar_is_typed() {
        let (index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\ninclude::partial$";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();

        assert!(
            labels.contains(&"partial$welcome.adoc".to_owned()),
            "{labels:?}"
        );
        assert!(
            !labels.contains(&"partial$token-note.adoc".to_owned()),
            "another module's partials must not leak in: {labels:?}"
        );
    }

    #[test]
    fn offers_image_resources_for_an_image_macro() {
        let (index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\nimage::";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();

        assert!(
            labels.contains(&"architecture.svg".to_owned()),
            "{labels:?}"
        );
    }

    #[test]
    fn carries_the_target_title_as_detail() {
        let (index, catalog, root) = antora_fixture();
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\nSee xref:security:";
        let document = parse("file:///index.adoc", text).document;

        let detail = completion_at_offset(&index, &catalog, &path, &document, text.len())
            .into_iter()
            .find(|candidate| candidate.label == "security:authentication.adoc")
            .and_then(|candidate| candidate.detail);

        assert_eq!(detail, Some("Authentication".to_owned()));
    }

    #[test]
    fn completes_relative_paths_outside_antora() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/xrefs");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        let path = root.join("index.adoc");
        let text = "= Index\n\nSee xref:";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), &path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert!(labels.contains(&"other.adoc".to_owned()), "{labels:?}");
    }

    #[test]
    fn completes_non_asciidoc_include_targets_from_the_filesystem() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        // No catalog, so this exercises the plain-workspace path.
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\ninclude::../examples/";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), &path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert!(
            labels.contains(&"../examples/sample.json".to_owned()),
            "a non-AsciiDoc include target must come from the directory read: {labels:?}"
        );
    }

    #[test]
    fn offers_directories_as_path_candidates() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\nSee xref:";
        let document = parse("file:///index.adoc", text).document;

        let candidates =
            completion_at_offset(&index, &AntoraCatalog::new(), &path, &document, text.len());
        let directory = candidates
            .iter()
            .find(|candidate| candidate.label == "guides/")
            .expect("a directory candidate");

        assert_eq!(directory.kind, super::CandidateKind::Directory);
    }

    #[test]
    fn a_leading_separator_does_not_escape_the_document_directory() {
        // `PathBuf::join` would treat this as an absolute path and discard the document's
        // own directory entirely, listing this real fixture directory instead. Point it at
        // one that certainly exists so the vulnerable behaviour would be observable, then
        // assert it is not: the sibling file below must not appear in the candidates.
        let escape_target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component/modules/ROOT/examples");
        assert!(
            escape_target.join("sample.json").exists(),
            "fixture must exist for this test to be meaningful"
        );

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/xrefs");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        let path = root.join("index.adoc");
        let text = format!("= Index\n\ninclude::{}/", escape_target.display());
        let document = parse("file:///index.adoc", &text).document;

        let candidates =
            completion_at_offset(&index, &AntoraCatalog::new(), &path, &document, text.len());

        assert!(
            candidates.is_empty(),
            "a leading separator must resolve relative to the document, not escape it: \
             {candidates:?}"
        );
    }

    #[test]
    fn a_parent_relative_prefix_still_resolves_above_the_document_directory() {
        // Unlike a leading `/`, a leading `../` is a legitimate way to reach above the
        // document's own directory and must keep working after the fix above.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index
            .index_roots(std::slice::from_ref(&root))
            .expect("index fixture");
        let path = root.join("modules/ROOT/pages/index.adoc");
        let text = "= Index\n\ninclude::../examples/";
        let document = parse("file:///index.adoc", text).document;

        let labels: Vec<_> =
            completion_at_offset(&index, &AntoraCatalog::new(), &path, &document, text.len())
                .into_iter()
                .map(|candidate| candidate.label)
                .collect();

        assert_eq!(labels, vec!["../examples/sample.json".to_owned()]);
    }
}
