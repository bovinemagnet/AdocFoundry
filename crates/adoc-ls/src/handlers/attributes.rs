//! Attributes a document can reference without declaring them itself.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use adoc_antora::AntoraCatalog;
use adoc_core::Document;
use adoc_index::{normalize_path, WorkspaceIndex};

use super::includes::composed_files;

/// Where an attribute's value comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttributeSource {
    Document,
    Included(PathBuf),
    /// The component's `antora.yml`.
    Component,
    /// The page and family attributes Antora computes for every page.
    Antora,
    BuiltIn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeDefinition {
    pub name: String,
    /// The value, or for a built-in what it stands for.
    pub value: Option<String>,
    pub source: AttributeSource,
}

/// Every attribute the document can reference, first definition winning.
///
/// The document's own declarations come first, then those of the files it includes, then
/// what Antora supplies, then Asciidoctor's built-ins. An attribute the document unsets
/// (`:name!:`) is not offered from any source.
#[must_use]
pub fn attribute_definitions(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    current_path: &Path,
    document: &Document,
) -> Vec<AttributeDefinition> {
    let mut seen = HashSet::new();
    let mut definitions = Vec::new();
    let mut define = |name: &str, value: Option<&str>, source: AttributeSource| {
        if let Some(unset) = name.strip_suffix('!').or_else(|| name.strip_prefix('!')) {
            seen.insert(unset.to_owned());
            return;
        }
        if !seen.insert(name.to_owned()) {
            return;
        }
        definitions.push(AttributeDefinition {
            name: name.to_owned(),
            value: value.map(str::to_owned),
            source,
        });
    };

    for attribute in &document.attributes {
        define(
            &attribute.name,
            attribute.value.as_deref(),
            AttributeSource::Document,
        );
    }

    let antora_context = antora.context_for_path(current_path);
    let current = normalize_path(current_path);
    for path in composed_files(index, antora, antora_context.as_ref(), current_path) {
        if path == current {
            continue;
        }
        let Some(file) = index.file(&path) else {
            continue;
        };
        for attribute in &file.document.attributes {
            define(
                &attribute.name,
                attribute.value.as_deref(),
                AttributeSource::Included(path.clone()),
            );
        }
    }

    for (name, value) in component_attributes(antora, current_path) {
        define(name, Some(value), AttributeSource::Component);
    }
    for (name, value) in antora_attributes(antora, current_path) {
        define(&name, Some(&value), AttributeSource::Antora);
    }

    for (name, description) in BUILT_IN_ATTRIBUTES {
        define(name, Some(description), AttributeSource::BuiltIn);
    }

    definitions
}

/// Attributes Asciidoctor defines for every document, with what each one stands for.
pub const BUILT_IN_ATTRIBUTES: &[(&str, &str)] = &[
    // Character replacements.
    ("blank", "nothing"),
    ("empty", "nothing"),
    ("sp", "space"),
    ("nbsp", "non-breaking space"),
    ("zwsp", "zero-width space"),
    ("wj", "word joiner"),
    ("apos", "'"),
    ("quot", "\""),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("deg", "°"),
    ("plus", "+"),
    ("brvbar", "¦"),
    ("vbar", "|"),
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("startsb", "["),
    ("endsb", "]"),
    ("caret", "^"),
    ("asterisk", "*"),
    ("tilde", "~"),
    ("backslash", "\\"),
    ("backtick", "`"),
    ("two-colons", "::"),
    ("two-semicolons", ";;"),
    ("cpp", "C++"),
    ("pp", "++"),
    // Intrinsic document attributes.
    ("doctitle", "document title"),
    ("docname", "document file name without suffix"),
    ("docfile", "document file path"),
    ("docfilesuffix", "document file suffix"),
    ("docdir", "document directory"),
    ("doctype", "document type"),
    ("docdate", "document modification date"),
    ("doctime", "document modification time"),
    ("docdatetime", "document modification date and time"),
    ("docyear", "document modification year"),
    ("localdate", "render date"),
    ("localtime", "render time"),
    ("localdatetime", "render date and time"),
    ("localyear", "render year"),
    ("backend", "output backend"),
    ("basebackend", "base output backend"),
    ("outfilesuffix", "output file suffix"),
    ("filetype", "output file type"),
    ("asciidoctor-version", "Asciidoctor version"),
];

/// The `asciidoc.attributes` of the component `source` belongs to, empty when the file is not
/// in an Antora module.
pub fn component_attributes<'a>(
    antora: &'a AntoraCatalog,
    source: &Path,
) -> impl Iterator<Item = (&'a str, &'a str)> {
    antora
        .context_for_path(source)
        .and_then(|context| antora.component(&context.component, context.version.as_deref()))
        .into_iter()
        .flat_map(|component| {
            component
                .asciidoc_attributes
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
        })
}

/// Antora page attributes for `source`, empty when the file is not in an Antora module.
///
/// `adoc-ls` knows the component and module; a bare Asciidoctor invocation would not.
pub fn antora_attributes(
    antora: &AntoraCatalog,
    source: &Path,
) -> impl Iterator<Item = (String, String)> + use<> {
    let context = antora.context_for_path(source);
    let module_root = context.as_ref().and_then(|context| {
        antora
            .module(
                &context.component,
                context.version.as_deref(),
                &context.module,
            )
            .map(|module| module.root.clone())
    });

    // Antora sets these for every page; a document written against them fails under a
    // stock Asciidoctor otherwise. Absolute, so a preview rendered elsewhere still finds
    // partials, examples, and images.
    let families = module_root.into_iter().flat_map(|root| {
        let moduledir = ("moduledir".to_owned(), root.display().to_string());
        [
            ("pagesdir", "pages"),
            ("partialsdir", "partials"),
            ("examplesdir", "examples"),
            ("attachmentsdir", "attachments"),
            ("imagesdir", "images"),
        ]
        .into_iter()
        .map(move |(attribute, family)| {
            // Trailing separator so both `{partialsdir}note.adoc` and
            // `{partialsdir}/note.adoc` resolve. Antora sets these to a family prefix
            // rather than a path, so both spellings work there and both appear in the
            // wild; a doubled separator is harmless.
            (
                attribute.to_owned(),
                format!("{}/", root.join(family).display()),
            )
        })
        .chain(std::iter::once(moduledir))
    });

    let page = context.into_iter().flat_map(|context| {
        let mut attributes = vec![
            ("page-component-name".to_owned(), context.component),
            ("page-module".to_owned(), context.module),
        ];
        if let Some(version) = context.version {
            attributes.push(("page-component-version".to_owned(), version));
        }
        attributes
    });

    page.chain(families)
}
