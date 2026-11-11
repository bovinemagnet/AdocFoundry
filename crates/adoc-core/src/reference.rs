use std::borrow::Cow;

use crate::SourceRange;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    LocalAnchor,
    Xref,
    Link,
    Attribute,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub target: String,
    pub text: Option<String>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeDirective {
    pub target: String,
    pub attributes: Option<String>,
    pub range: SourceRange,
    /// Inside an `ifdef`, `ifndef` or `ifeval` block, so the build may never include it.
    pub conditional: bool,
    /// No body content comes before it (only blank lines, comments, attribute entries, block
    /// metadata, conditionals and other includes) and it is in no verbatim block, so what it
    /// includes may be the document header, the title included.
    pub before_body: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageDirective {
    pub target: String,
    pub attributes: Option<String>,
    pub range: SourceRange,
    /// An inline `image:target[]` rather than a block `image::target[]`.
    pub inline: bool,
    /// For a block image, what each block attribute line above it holds between its brackets,
    /// in order (`[#fig,alt=Sunset]` gives `#fig,alt=Sunset`): the attributes Asciidoctor gives
    /// the block besides the macro's own. Empty for an inline image.
    pub block_attributes: Vec<String>,
}

/// The document an xref target names, as Asciidoctor 2.0 and Antora 3.2 read it: the part
/// before its `#`, with `.adoc` added when a `#` follows and the last segment of the path has
/// no extension, so `other#details` and `admin:other#` name `other.adoc` and
/// `admin:other.adoc`. A target without `#` is returned as written (`xref:other[]` is an id in
/// the page), and so is an empty file part (`#details`).
#[must_use]
pub fn xref_file(target: &str) -> Cow<'_, str> {
    let Some((file, _)) = target.split_once('#') else {
        return Cow::Borrowed(target);
    };
    let last = file.rsplit(['/', ':', '$']).next().unwrap_or(file);
    // `File.extname` gives a name starting with its only dot (`.hidden`) no extension.
    let extension = last.rfind('.').is_some_and(|dot| dot > 0);
    if file.trim().is_empty() || extension {
        Cow::Borrowed(file)
    } else {
        Cow::Owned(format!("{file}.adoc"))
    }
}

#[cfg(test)]
mod tests {
    use super::xref_file;

    #[test]
    fn an_extensionless_file_before_a_fragment_is_an_adoc_file() {
        for (target, file) in [
            ("other#details", "other.adoc"),
            ("other#", "other.adoc"),
            ("sub/deep#x", "sub/deep.adoc"),
            ("admin:other#x", "admin:other.adoc"),
            ("other.adoc#details", "other.adoc"),
            ("other.adoc", "other.adoc"),
            ("other", "other"),
            ("#details", ""),
            ("v1.0/other#x", "v1.0/other.adoc"),
            ("partial$snippet.adoc#x", "partial$snippet.adoc"),
        ] {
            assert_eq!(xref_file(target), file, "{target}");
        }
    }
}
