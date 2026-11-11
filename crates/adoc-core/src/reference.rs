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
