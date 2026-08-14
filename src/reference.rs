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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageDirective {
    pub target: String,
    pub attributes: Option<String>,
    pub range: SourceRange,
}
