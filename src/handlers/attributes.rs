//! Attributes a document can reference without declaring them itself.

use std::path::Path;

use adoc_antora::AntoraCatalog;

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
