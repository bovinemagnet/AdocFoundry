//! Handling of `textDocument/hover`.
//!
//! Editor-agnostic: this works on byte offsets and returns Markdown, leaving all `lsp-types`
//! mapping to `protocol.rs`.

use std::path::Path;

use adoc_antora::AntoraCatalog;
use adoc_core::{Document, ReferenceKind, SourceRange};
use adoc_index::{normalize_path, WorkspaceIndex};

use super::{
    attributes::{attribute_definitions, AttributeSource},
    definition::{definition_at_offset, DefinitionTarget},
    images::resolve_image,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverContent {
    pub markdown: String,
    /// The construct the hover describes.
    pub range: SourceRange,
}

/// What the construct under `offset` points at, or `None` when there is nothing to say.
///
/// Targets are resolved exactly as Go to Definition resolves them, so a hover never describes
/// a file navigation would not reach. Unresolved targets say nothing: diagnostics report them.
#[must_use]
pub fn hover_at_offset(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
    offset: usize,
) -> Option<HoverContent> {
    // An attribute reference is the narrowest construct, and may sit inside a macro target.
    if let Some((name, range)) = attribute_reference_at(&document.text, offset) {
        return attribute_hover(index, antora, current_path, document, name, range);
    }

    if let Some(reference) = document
        .references
        .iter()
        .find(|reference| contains_offset(reference.range, offset))
    {
        let target = definition_at_offset(index, antora, current_path, document, offset)?;
        let anchor = match reference.kind {
            ReferenceKind::LocalAnchor => Some(reference.target.as_str()),
            _ => reference
                .target
                .split_once('#')
                .map(|(_, anchor)| anchor)
                .filter(|anchor| !anchor.is_empty()),
        };
        let section = anchor.and_then(|anchor| section_title(index, &target, anchor));
        return Some(HoverContent {
            markdown: describe_file(index, antora, current_path, &target.path, section),
            range: reference.range,
        });
    }

    if let Some(include) = document
        .includes
        .iter()
        .find(|include| contains_offset(include.range, offset))
    {
        let target = definition_at_offset(index, antora, current_path, document, offset)?;
        return Some(HoverContent {
            markdown: describe_file(index, antora, current_path, &target.path, None),
            range: include.range,
        });
    }

    let image = document
        .images
        .iter()
        .find(|image| contains_offset(image.range, offset))?;
    let path = resolve_image(index, antora, current_path, document, &image.target)?;
    Some(HoverContent {
        markdown: describe_file(index, antora, current_path, &path, None),
        range: image.range,
    })
}

/// The `{name}` reference around `offset`, with the range of the whole reference.
fn attribute_reference_at(text: &str, offset: usize) -> Option<(&str, SourceRange)> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    // The cursor may sit on the opening brace itself.
    let search_end = if text[offset..].starts_with('{') {
        offset + 1
    } else {
        offset
    };
    let open = line_start + text[line_start..search_end].rfind('{')?;
    if text[..open].ends_with('\\') {
        return None;
    }
    let close = open + 1 + text[open + 1..line_end].find('}')?;
    if offset > close {
        return None;
    }
    let name = &text[open + 1..close];
    let is_name_character =
        |character: char| character.is_alphanumeric() || character == '_' || character == '-';
    if name.is_empty() || name.starts_with('-') || !name.chars().all(is_name_character) {
        return None;
    }
    Some((name, SourceRange::new(open, close + 1)))
}

fn attribute_hover(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
    name: &str,
    range: SourceRange,
) -> Option<HoverContent> {
    let definition = attribute_definitions(index, antora, current_path, document)
        .into_iter()
        .find(|definition| definition.name == name)?;
    let value = definition.value.unwrap_or_default();
    let summary = match definition.source {
        AttributeSource::BuiltIn => format!("`{{{name}}}`: {value}"),
        _ if value.is_empty() => format!("`{{{name}}}` is set, with no value"),
        _ => format!("`{{{name}}}` = `{value}`"),
    };
    let origin = match &definition.source {
        AttributeSource::Document => "Declared in this document".to_owned(),
        AttributeSource::Included(path) => {
            format!("Declared in `{}`", display_path(path, current_path))
        }
        AttributeSource::Component => "Set in the component's `antora.yml`".to_owned(),
        AttributeSource::Antora => "Set by Antora for this page".to_owned(),
        AttributeSource::BuiltIn => "Asciidoctor built-in".to_owned(),
    };
    Some(HoverContent {
        markdown: format!("{summary}\n\n{origin}"),
        range,
    })
}

/// The title of the section `anchor` leads to in the target file.
fn section_title<'a>(
    index: &'a WorkspaceIndex,
    target: &DefinitionTarget,
    anchor: &str,
) -> Option<&'a str> {
    index
        .file(&target.path)?
        .document
        .sections
        .iter()
        .find(|section| {
            section.selection_range == target.range || section.id.as_deref() == Some(anchor)
        })
        .map(|section| section.title.as_str())
}

/// The target's title, the section within it, and where it lives. A section of the current
/// document needs neither the title nor the location: the author is looking at both.
fn describe_file(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    path: &Path,
    section: Option<&str>,
) -> String {
    let section_line = section.map(|section| format!("Section: **{section}**"));
    if normalize_path(path) == normalize_path(current_path) {
        if let Some(line) = section_line {
            return line;
        }
    }

    let mut parts = Vec::new();
    if let Some(title) = index
        .file(path)
        .and_then(|file| file.document.title.as_ref())
    {
        parts.push(format!("**{}**", title.text));
    }
    parts.extend(section_line);
    let location =
        antora_resource_id(antora, path).unwrap_or_else(|| display_path(path, current_path));
    parts.push(format!("`{location}`"));
    parts.join("\n\n")
}

/// The fully qualified Antora resource ID of `path`, when it sits in a family directory.
fn antora_resource_id(antora: &AntoraCatalog, path: &Path) -> Option<String> {
    let context = antora.context_for_path(path)?;
    let module = antora.module(
        &context.component,
        context.version.as_deref(),
        &context.module,
    )?;
    let family_root = normalize_path(&module.root.join(context.family.directory()));
    let relative = normalize_path(path)
        .strip_prefix(&family_root)
        .ok()?
        .to_path_buf();
    let version = context
        .version
        .map_or_else(String::new, |version| format!("{version}@"));
    Some(format!(
        "{version}{}:{}:{}${}",
        context.component,
        context.module,
        context.family,
        relative.display()
    ))
}

/// `path` relative to the current document's directory where it sits beneath it.
fn display_path(path: &Path, current_path: &Path) -> String {
    let path = normalize_path(path);
    normalize_path(current_path)
        .parent()
        .and_then(|directory| path.strip_prefix(directory).ok())
        .unwrap_or(&path)
        .display()
        .to_string()
}

fn contains_offset(range: SourceRange, offset: usize) -> bool {
    range.start <= offset && offset < range.end
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use adoc_antora::{discover_antora_workspace, AntoraCatalog};
    use adoc_index::WorkspaceIndex;

    use super::{hover_at_offset, HoverContent};

    /// Hover over the first occurrence of `needle` in `text`, as indexed at `path`.
    fn hover(
        index: &mut WorkspaceIndex,
        antora: &AntoraCatalog,
        path: &Path,
        text: &str,
        needle: &str,
    ) -> Option<HoverContent> {
        index.index_source(path, text);
        let document = index.file(path).expect("indexed").document.clone();
        let offset = text.find(needle).expect("needle in text") + 1;
        hover_at_offset(index, antora, path, &document, offset)
    }

    fn antora_fixture() -> (WorkspaceIndex, AntoraCatalog, PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();
        let antora = discover_antora_workspace(std::slice::from_ref(&root))
            .unwrap()
            .catalog;
        (index, antora, root)
    }

    #[test]
    fn describes_an_xref_target_by_title_and_path() {
        let mut index = WorkspaceIndex::new();
        index.index_source("/docs/other.adoc", "= Other Page\n");
        let text = "= Home\n\nSee xref:other.adoc[].\n";

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            Path::new("/docs/index.adoc"),
            text,
            "xref:",
        )
        .expect("a hover");

        assert!(content.markdown.contains("**Other Page**"), "{content:?}");
        assert!(content.markdown.contains("`other.adoc`"), "{content:?}");
        let start = text.find("xref:").unwrap();
        assert_eq!(content.range.start, start);
    }

    #[test]
    fn names_the_section_an_anchor_leads_to() {
        let mut index = WorkspaceIndex::new();
        let text = "= Home\n\nSee <<details>>.\n\n[[details]]\n== Details Section\n";

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            Path::new("/docs/index.adoc"),
            text,
            "<<details",
        )
        .expect("a hover");

        assert!(content.markdown.contains("Details Section"), "{content:?}");
    }

    #[test]
    fn describes_an_antora_xref_by_its_resource_id() {
        let (mut index, antora, root) = antora_fixture();
        let text = "= Home\n\nxref:security:authentication.adoc[Authentication]\n";

        let content = hover(
            &mut index,
            &antora,
            &root.join("modules/ROOT/pages/index.adoc"),
            text,
            "xref:",
        )
        .expect("a hover");

        assert!(
            content
                .markdown
                .contains("demo:security:page$authentication.adoc"),
            "{content:?}"
        );
    }

    #[test]
    fn describes_an_include_target() {
        let mut index = WorkspaceIndex::new();
        index.index_source("/docs/partials/intro.adoc", "= Introduction\n");

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            Path::new("/docs/index.adoc"),
            "= Home\n\ninclude::partials/intro.adoc[]\n",
            "include::",
        )
        .expect("a hover");

        assert!(content.markdown.contains("**Introduction**"), "{content:?}");
        assert!(
            content.markdown.contains("`partials/intro.adoc`"),
            "{content:?}"
        );
    }

    #[test]
    fn describes_an_antora_image_by_its_resource_id() {
        let (mut index, antora, root) = antora_fixture();

        let content = hover(
            &mut index,
            &antora,
            &root.join("modules/ROOT/pages/index.adoc"),
            "= Home\n\nimage::architecture.svg[]\n",
            "image::",
        )
        .expect("a hover");

        assert!(
            content
                .markdown
                .contains("demo:ROOT:image$architecture.svg"),
            "{content:?}"
        );
    }

    #[test]
    fn describes_an_image_under_the_declared_imagesdir() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            &root.join("guide.adoc"),
            "= Guide\n:imagesdir: media\n\nimage::present.svg[]\n",
            "image::",
        )
        .expect("a hover");

        assert!(
            content.markdown.contains("`media/present.svg`"),
            "{content:?}"
        );
    }

    #[test]
    fn gives_an_attribute_s_value_and_where_it_is_declared() {
        let mut index = WorkspaceIndex::new();
        let text = ":product: Widget\n\nVersion {product}.\n";

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            Path::new("/docs/index.adoc"),
            text,
            "{product}",
        )
        .expect("a hover");

        assert!(content.markdown.contains("`Widget`"), "{content:?}");
        assert!(content.markdown.contains("this document"), "{content:?}");
        let start = text.find("{product}").unwrap();
        assert_eq!(content.range.start, start);
        assert_eq!(content.range.end, start + "{product}".len());
    }

    #[test]
    fn names_the_included_file_that_declares_an_attribute() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/attributes");
        let mut index = WorkspaceIndex::new();
        index.index_roots(std::slice::from_ref(&root)).unwrap();

        let content = hover(
            &mut index,
            &AntoraCatalog::new(),
            &root.join("index.adoc"),
            "include::shared/settings.adoc[]\n\nRelease {release}.\n",
            "{release}",
        )
        .expect("a hover");

        assert!(content.markdown.contains("`2.0`"), "{content:?}");
        assert!(
            content.markdown.contains("`shared/release.adoc`"),
            "{content:?}"
        );
    }

    #[test]
    fn attributes_component_and_built_in_values_to_their_source() {
        let (mut index, antora, root) = antora_fixture();
        let page = root.join("modules/ROOT/pages/index.adoc");

        let component = hover(
            &mut index,
            &antora,
            &page,
            "= Home\n\nUses {source-highlighter}.\n",
            "{source-highlighter}",
        )
        .expect("a hover");
        assert!(
            component.markdown.contains("`highlight.js`"),
            "{component:?}"
        );
        assert!(component.markdown.contains("antora.yml"), "{component:?}");

        let built_in =
            hover(&mut index, &antora, &page, "= Home\n\nA{nbsp}B\n", "{nbsp}").expect("a hover");
        assert!(
            built_in.markdown.contains("non-breaking space"),
            "{built_in:?}"
        );
        assert!(built_in.markdown.contains("built-in"), "{built_in:?}");
    }

    #[test]
    fn says_nothing_where_there_is_nothing_to_resolve() {
        let mut index = WorkspaceIndex::new();
        let path = Path::new("/docs/index.adoc");
        let antora = AntoraCatalog::new();

        for (text, needle) in [
            ("Plain prose here.\n", "prose"),
            ("Version {unknown}.\n", "{unknown}"),
            ("See xref:missing.adoc[].\n", "xref:"),
            ("Escaped \\{product}.\n:product: x\n", "{product}"),
        ] {
            assert_eq!(
                hover(&mut index, &antora, path, text, needle),
                None,
                "{text}"
            );
        }
    }
}
