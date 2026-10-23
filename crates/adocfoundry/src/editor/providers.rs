//! Engine-backed definition, hover and completion for the editor widget.
//!
//! gpui-component's editor exposes these features through provider traits whose types come from
//! `lsp-types`. This module is the only place the app uses them: it is a translation layer, and
//! every answer comes from the engine (and so from `adoc-lang`), exactly as `adoc-ls` gives it.

use std::{cell::Cell, path::Path, str::FromStr as _};

use adoc_core::SourceRange;
use adoc_engine::{CompletionItemKind, DefinitionTarget, DocumentId};
use gpui::{App, Result, Task, WeakEntity, Window};
use gpui_component::input::{
    CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt as _,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Hover,
    HoverContents, LocationLink, MarkupContent, MarkupKind, Range, TextEdit, Uri,
};

use crate::{app::AppState, editor::sync_widget_to_engine};

pub struct EngineProviders {
    app: WeakEntity<AppState>,
    document: DocumentId,
    /// The offset of the last definition request, which is the link a click follows.
    last_definition_offset: Cell<usize>,
}

impl EngineProviders {
    pub fn new(app: WeakEntity<AppState>, document: DocumentId) -> Self {
        Self {
            app,
            document,
            last_definition_offset: Cell::new(0),
        }
    }

    pub fn last_definition_offset(&self) -> usize {
        self.last_definition_offset.get()
    }

    /// Brings the engine up to date with the widget's text before asking it anything.
    fn sync(&self, text: &Rope, cx: &mut App) -> Option<gpui::Entity<AppState>> {
        let app = self.app.upgrade()?;
        let text = text.to_string();
        app.update(cx, |app, cx| {
            sync_widget_to_engine(app, self.document, &text, cx)
        });
        Some(app)
    }
}

fn lsp_range(text: &Rope, range: SourceRange) -> Range {
    Range::new(
        text.offset_to_position(range.start.min(text.len())),
        text.offset_to_position(range.end.min(text.len())),
    )
}

/// A `file:` URI for `path`; the editor only needs it to be well formed, since the app's
/// show-document handler navigates through the engine.
fn file_uri(path: &Path) -> Uri {
    let mut encoded = String::from("file://");
    for byte in path.to_string_lossy().replace('\\', "/").bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if !encoded.starts_with("file:///") {
        encoded.insert(7, '/');
    }
    Uri::from_str(&encoded).unwrap_or_else(|_| Uri::from_str("file:///").expect("valid URI"))
}

/// The range of the xref, include, image or attribute reference at `offset`, which is what the
/// editor underlines while hovering with the modifier held.
fn link_range(app: &AppState, document: DocumentId, offset: usize) -> Option<SourceRange> {
    let parsed = &app.engine.document(document)?.parsed;
    let contains = |range: &SourceRange| range.start <= offset && offset < range.end;
    parsed
        .references
        .iter()
        .map(|reference| reference.range)
        .chain(parsed.includes.iter().map(|include| include.range))
        .chain(parsed.images.iter().map(|image| image.range))
        .find(contains)
}

impl DefinitionProvider for EngineProviders {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        self.last_definition_offset.set(offset);
        let Some(app) = self.sync(text, cx) else {
            return Task::ready(Ok(Vec::new()));
        };
        let app = app.read(cx);
        let Some(target) = app.engine.definition(self.document, offset) else {
            return Task::ready(Ok(Vec::new()));
        };
        let origin = link_range(app, self.document, offset)
            .unwrap_or_else(|| SourceRange::new(offset, offset));
        let (path, target_range) = match target {
            DefinitionTarget::Location(location) => (location.path, Range::default()),
            DefinitionTarget::External(path) => (path, Range::default()),
        };
        Task::ready(Ok(vec![LocationLink {
            origin_selection_range: Some(lsp_range(text, origin)),
            target_uri: file_uri(&path),
            target_range,
            target_selection_range: target_range,
        }]))
    }
}

impl HoverProvider for EngineProviders {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let Some(app) = self.sync(text, cx) else {
            return Task::ready(Ok(None));
        };
        let hover = app
            .read(cx)
            .engine
            .hover(self.document, offset)
            .map(|hover| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: hover.markdown,
                }),
                range: Some(lsp_range(text, hover.range)),
            });
        Task::ready(Ok(hover))
    }
}

impl CompletionProvider for EngineProviders {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let Some(app) = self.sync(text, cx) else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let items = app
            .read(cx)
            .engine
            .completions(self.document, offset)
            .into_iter()
            .map(|candidate| CompletionItem {
                kind: Some(match candidate.kind {
                    CompletionItemKind::Page => lsp_types::CompletionItemKind::FILE,
                    CompletionItemKind::Resource => lsp_types::CompletionItemKind::REFERENCE,
                    CompletionItemKind::Family => lsp_types::CompletionItemKind::MODULE,
                    CompletionItemKind::Directory => lsp_types::CompletionItemKind::FOLDER,
                    CompletionItemKind::Anchor => lsp_types::CompletionItemKind::REFERENCE,
                    CompletionItemKind::Attribute => lsp_types::CompletionItemKind::VARIABLE,
                }),
                detail: candidate.detail.clone(),
                sort_text: Some(candidate.sort_text.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: lsp_range(text, candidate.range),
                    new_text: candidate
                        .insert_text
                        .clone()
                        .unwrap_or_else(|| candidate.label.clone()),
                })),
                label: candidate.label,
                ..CompletionItem::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // The engine decides from context whether anything completes; typing inside a target
        // or after `{`, `$`, `:` or `#` is where it will.
        new_text
            .chars()
            .last()
            .is_some_and(|character| !character.is_whitespace() && character != ']')
    }
}
