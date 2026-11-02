//! Writing a link into a page: the xref or include that names another file from the page, as
//! the graph's Insert Xref at Caret and Include at Caret write it.

use std::path::Path;

use adoc_antora::ResourceFamily;
use adoc_core::SourceRange;
use adoc_index::normalize_path;

use crate::{
    search::{antora_id, relative_path, slash_path, with_family},
    DocumentId, Engine, TextEdit,
};

/// How a link to another file is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkStyle {
    /// `xref:target[]`, at the offset.
    Xref,
    /// `include::target[]`, on a line of its own.
    Include,
}

impl Engine {
    /// The edit writing a link to the file at `target` into document `id` at `offset`: an xref
    /// at the offset, set apart by a space straight after a word, or an include on the offset's
    /// line when that is blank, else on a line after it. The target is the shortest resource id
    /// that names the file from the page, or a path relative to it outside Antora, and is
    /// checked to resolve to the file. `None` for a page linking to itself, a document that is
    /// not AsciiDoc, and a file the page cannot name.
    #[must_use]
    pub fn link_edit(
        &self,
        id: DocumentId,
        offset: usize,
        target: &Path,
        style: LinkStyle,
    ) -> Option<TextEdit> {
        let document = self.document(id)?;
        let from = normalize_path(&document.path);
        let target = normalize_path(target);
        if !document.asciidoc || from == target {
            return None;
        }
        let written = self.link_targets(&from, &target, style).find(|written| {
            let line = match style {
                LinkStyle::Xref => format!("xref:{written}[]"),
                LinkStyle::Include => format!("include::{written}[]"),
            };
            matches!(
                self.resolve_target(&from, &line),
                Some(adoc_lang::DefinitionTarget::Location(found))
                    if normalize_path(&found.path) == target
            )
        })?;
        let text = document.text.to_string();
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let (at, replacement) = match style {
            LinkStyle::Xref => {
                let after_word = text[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric);
                let space = if after_word { " " } else { "" };
                (offset, format!("{space}xref:{written}[]"))
            }
            LinkStyle::Include => {
                let start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
                let end = text[offset..]
                    .find('\n')
                    .map_or(text.len(), |newline| offset + newline);
                if text[start..end].trim().is_empty() {
                    (start, format!("include::{written}[]"))
                } else {
                    (end, format!("\ninclude::{written}[]"))
                }
            }
        };
        Some(TextEdit::new(SourceRange::new(at, at), replacement))
    }

    /// The ways to write `target` from the page `from`, shortest first: resource ids within
    /// Antora (an xref only to a page), with the version when the shortest names another; a
    /// relative path outside it.
    fn link_targets(
        &self,
        from: &Path,
        target: &Path,
        style: LinkStyle,
    ) -> impl Iterator<Item = String> {
        let antora = &self.workspace.antora;
        let mut written = Vec::new();
        match antora.context_for_path(from) {
            Some(context) => {
                if let Some(resource) = antora.resource_for_path(target) {
                    let coordinate = &resource.coordinate;
                    let family = coordinate.family;
                    let spelled = |id: String| match style {
                        LinkStyle::Xref => id,
                        LinkStyle::Include => with_family(id, family),
                    };
                    if style == LinkStyle::Include || family == ResourceFamily::Page {
                        written.push(spelled(antora_id(&context, coordinate, "")));
                        if let Some(version) = &coordinate.version {
                            written.push(spelled(format!(
                                "{version}@{}:{}:{}",
                                coordinate.component,
                                coordinate.module,
                                slash_path(&coordinate.relative_path)
                            )));
                        }
                    }
                }
            }
            None => {
                if let Some(folder) = from.parent() {
                    written.push(relative_path(folder, target));
                }
            }
        }
        written.into_iter()
    }
}
