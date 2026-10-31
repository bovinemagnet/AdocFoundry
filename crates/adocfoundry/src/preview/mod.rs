//! The rendered preview: the engine's view tree of the preview target, drawn natively. It
//! follows the active document unless pinned, and remembers each page's scroll position.

pub(crate) mod blocks;
pub(crate) mod inlines;
pub(crate) mod pdf;
pub(crate) mod sync;

use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, EngineEvent, NavigationPosition, RenderedPreview,
};
use gpui::{
    div, prelude::FluentBuilder as _, px, App, Context, Entity, HighlightStyle,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Subscription, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    highlighter::SyntaxHighlighter,
    v_flex, ActiveTheme as _, Sizable as _, TitleBar, WindowExt as _,
};

use crate::app::AppState;

pub use inlines::Link;

/// How far beyond the view rows are laid out.
const LIST_OVERDRAW: Pixels = px(400.);

/// Frames to wait for a block scrolled to before giving up on placing it exactly.
const REVEAL_FRAMES: u8 = 3;

/// Highlighted code, by theme brightness and a hash of its language and text, with the language
/// and text it is of.
pub(crate) type HighlightCache = HashMap<(bool, u64), (String, String, Highlights)>;

/// The styles of highlighted code, shared by the cache and whoever draws it.
pub(crate) type Highlights = Rc<[(Range<usize>, HighlightStyle)]>;

/// What the preview tells the window around it.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewEvent {
    /// The user scrolled the preview; this block is now at its top.
    ScrolledTo(adoc_render::SourceSpan),
}

pub struct PreviewView {
    app: Entity<AppState>,
    extents: sync::Extents,
    /// In its own window, which then needs a title bar.
    standalone: bool,
    shown: Option<Arc<RenderedPreview>>,
    /// The rows drawn, laid out only where they are in view.
    list: gpui::ListState,
    /// The rows of the view tree shown.
    rows: Rc<[blocks::Row]>,
    /// A block to scroll to once it has been laid out.
    revealing: Option<(Vec<usize>, u8)>,
    positions: HashMap<DocumentId, gpui::ListOffset>,
    highlights: Rc<RefCell<HighlightCache>>,
    /// Inline text as laid out for the view tree shown, in the style it was laid out in.
    inline_cache: Rc<inlines::InlineCache>,
    inline_for: Option<(Arc<RenderedPreview>, inlines::InlineStyle)>,
    /// A sorted index of the blocks shown, for following the caret.
    blocks: Option<adoc_render::BlockIndex>,
    /// The target's PDF pages, while the preview shows them.
    pdf: pdf::PdfPages,
    _subscription: Subscription,
}

impl PreviewView {
    pub fn new(
        app: Entity<AppState>,
        standalone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe_in(&app, window, |this, _, event, _, cx| {
            let relevant = match event {
                EngineEvent::PreviewReady { id, .. } | EngineEvent::PdfPreviewReady { id, .. } => {
                    this.app.read(cx).engine.preview_target() == Some(*id)
                }
                EngineEvent::PreviewLayoutChanged
                | EngineEvent::NavigationChanged
                | EngineEvent::DocumentClosed(_) => true,
                _ => false,
            };
            if relevant {
                this.refresh(cx);
            }
        });
        let mut view = Self {
            app,
            extents: sync::Extents::default(),
            standalone,
            shown: None,
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, LIST_OVERDRAW),
            rows: Rc::from(Vec::new()),
            revealing: None,
            positions: HashMap::new(),
            highlights: Rc::default(),
            inline_cache: Rc::default(),
            inline_for: None,
            blocks: None,
            pdf: pdf::PdfPages::default(),
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The preview on screen.
    #[cfg(test)]
    pub fn shown(&self) -> Option<&Arc<RenderedPreview>> {
        self.shown.as_ref()
    }

    /// Shows the engine's latest preview of its target, keeping each page's scroll position.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let latest = {
            let engine = &self.app.read(cx).engine;
            engine
                .preview_target()
                .and_then(|id| engine.rendered_preview(id))
        };
        let before = self.shown.as_ref().map(|shown| shown.document);
        let after = latest.as_ref().map(|shown| shown.document);
        let changed = match (&self.shown, &latest) {
            (Some(shown), Some(latest)) => !Arc::ptr_eq(shown, latest),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.blocks = latest
                .as_ref()
                .map(|shown| adoc_render::BlockIndex::new(&shown.view));
            let rows = latest
                .as_ref()
                .map(|shown| blocks::rows(&shown.view))
                .unwrap_or_default();
            if before == after {
                // A new version of the same page: rows past the old count are added or
                // dropped, and every other row is measured again where it is next laid out, so
                // the scroll position holds.
                let (old, new) = (self.list.item_count(), rows.len());
                if new != old {
                    self.list.splice(new.min(old)..old, new.saturating_sub(old));
                }
                self.list.remeasure_items(0..new.min(old));
            } else {
                if let Some(before) = before {
                    self.positions
                        .insert(before, self.list.logical_scroll_top());
                }
                self.list.reset(rows.len());
                let offset = after
                    .and_then(|after| self.positions.get(&after).copied())
                    .unwrap_or_default();
                self.list.scroll_to(offset);
                self.revealing = None;
            }
            self.rows = Rc::from(rows);
        }
        self.shown = latest;
        let pdf = {
            let engine = &self.app.read(cx).engine;
            engine
                .preview_layout()
                .pdf
                .then(|| engine.preview_target())
                .flatten()
                .and_then(|id| engine.pdf_preview(id))
        };
        self.pdf.update(pdf, cx, |view: &mut Self| &mut view.pdf);
        cx.notify();
    }

    /// The PDF pages on screen.
    #[cfg(test)]
    pub fn pdf_shown(&self) -> Option<&Arc<adoc_engine::PdfPreview>> {
        self.pdf.shown.as_ref()
    }

    /// Scrolls the PDF pages so page `page` is at the top.
    #[cfg(test)]
    pub fn pdf_scroll_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(pages) = self
            .pdf
            .shown
            .as_ref()
            .and_then(|shown| shown.pages.as_ref().ok())
        else {
            return;
        };
        if let Some(top) = pdf::page_tops(pages).get(page) {
            self.pdf.scroll.set_offset(gpui::point(px(0.), -px(*top)));
            cx.notify();
        }
    }

    /// Where a point `y` points down page `page` of the PDF is drawn, for tests that click it.
    #[cfg(test)]
    pub fn pdf_position(&self, page: usize, y: f32) -> Option<gpui::Point<Pixels>> {
        let pages = self.pdf.shown.as_ref()?.pages.as_ref().ok()?;
        let top = *pdf::page_tops(pages).get(page)?;
        let container = self.pdf.scroll.bounds();
        Some(gpui::point(
            container.origin.x + container.size.width / 2.,
            container.origin.y + self.pdf.scroll.offset().y + px(top + y),
        ))
    }

    /// Scrolls so the innermost block containing `offset` in `path` is at the top, keeping the
    /// preview level with the caret.
    pub fn reveal_source(&mut self, path: &std::path::Path, offset: usize, cx: &mut Context<Self>) {
        let Some(shown) = &self.shown else {
            return;
        };
        let Some(blocks) = &self.blocks else {
            return;
        };
        let Some(trail) = blocks.block_at(path, offset) else {
            // The title and header are not blocks: a caret above the first block shows the top.
            let view = &shown.view;
            if path == view.source
                && view
                    .blocks
                    .first()
                    .is_none_or(|block| offset < block.span.range.start)
            {
                self.revealing = None;
                self.scroll_to(px(0.), cx);
            }
            return;
        };
        let trail = trail.to_vec();
        let extent = self.extents.borrow().get(&trail).copied();
        match extent {
            Some(extent) => {
                self.revealing = None;
                self.scroll_to((extent.top - px(8.)).max(px(0.)), cx);
            }
            // Not laid out: to its row, which places a block that is a row of its own exactly;
            // a block inside a row is placed once the row has been laid out.
            None => {
                let Some(index) = blocks::row_of(&self.rows, &trail) else {
                    return;
                };
                let own_row = matches!(
                    &self.rows[index],
                    blocks::Row::Block { trail: row, .. } if *row == trail
                );
                let offset = gpui::ListOffset {
                    item_ix: index,
                    offset_in_item: (blocks::row_gap(&self.rows, index) - px(8.)).max(px(0.)),
                };
                let logical = self.list.logical_scroll_top();
                if logical.item_ix != offset.item_ix
                    || logical.offset_in_item != offset.offset_in_item
                {
                    self.list.scroll_to(offset);
                    cx.notify();
                }
                self.revealing = (!own_row).then_some((trail, REVEAL_FRAMES));
            }
        }
    }

    /// Scrolls so content position `top` is at the top, redrawing only when that moves it: the
    /// caret moving within a block leaves the preview as it is.
    fn scroll_to(&mut self, top: Pixels, cx: &mut Context<Self>) {
        if top <= px(0.) {
            if self.list.logical_scroll_top().item_ix != 0
                || self.list.logical_scroll_top().offset_in_item != px(0.)
            {
                self.list.scroll_to(gpui::ListOffset::default());
                cx.notify();
            }
            return;
        }
        let distance = top - self.content_top();
        if distance.abs() >= px(0.5) {
            self.list.scroll_by(distance);
            cx.notify();
        }
    }

    /// The content position at the top of the preview, as far as the list has measured it.
    fn content_top(&self) -> Pixels {
        -self.list.scroll_px_offset_for_scrollbar().y
    }

    /// Finishes revealing a block that was not laid out when asked for: scrolls to it once it
    /// has been, or asks for another frame, a few times.
    fn finish_revealing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((trail, frames)) = self.revealing.take() else {
            return;
        };
        let extent = self.extents.borrow().get(&trail).copied();
        match extent {
            Some(extent) => self.scroll_to((extent.top - px(8.)).max(px(0.)), cx),
            None if frames > 0 => {
                self.revealing = Some((trail, frames - 1));
                window.request_animation_frame();
            }
            None => {}
        }
    }

    /// Reports the block now at the top after the user scrolled.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        self.revealing = None;
        let Some(shown) = &self.shown else {
            return;
        };
        let top = self.content_top();
        let Some(trail) = sync::block_at_y(&self.extents.borrow(), top) else {
            return;
        };
        if let Some(block) = adoc_render::block_by_trail(&shown.view, &trail) {
            cx.emit(PreviewEvent::ScrolledTo(block.span.clone()));
        }
    }

    /// Where the block at `trail` is drawn, for tests that click it.
    #[cfg(test)]
    pub fn block_position(&self, trail: &[usize]) -> Option<gpui::Point<Pixels>> {
        let extent = *self.extents.borrow().get(trail)?;
        let viewport = self.list.viewport_bounds();
        let y = viewport.origin.y + extent.top - self.content_top() + px(2.);
        Some(gpui::point(viewport.origin.x + px(24.), y))
    }

    /// Simulates the user scrolling the preview to content position `top`.
    #[cfg(test)]
    pub fn scroll_for_test(&mut self, top: Pixels, cx: &mut Context<Self>) {
        self.list.scroll_by(top - self.content_top());
        self.scrolled(cx);
    }

    /// The content position at the top of the preview.
    #[cfg(test)]
    pub fn scroll_top(&self) -> Pixels {
        self.content_top()
    }

    /// How many blocks the last frame laid out.
    #[cfg(test)]
    pub fn laid_out(&self) -> usize {
        self.extents.borrow().len()
    }

    /// The extent of the block at `trail`, once laid out.
    #[cfg(test)]
    pub fn extent(&self, trail: &[usize]) -> Option<sync::Extent> {
        self.extents.borrow().get(trail).copied()
    }

    fn invoke(
        &self,
        command: adoc_engine::CommandId,
        args: CommandArgs,
        window: &mut Window,
        cx: &mut App,
    ) {
        let result = self.app.update(cx, |app, cx| app.invoke(command, args, cx));
        if let Err(error) = result {
            window.push_notification(error.to_string(), cx);
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, layout) = {
            let engine = &self.app.read(cx).engine;
            let title = self
                .shown
                .as_ref()
                .and_then(|shown| shown.path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Preview".to_owned());
            (title, engine.preview_layout())
        };
        let command = |command: adoc_engine::CommandId| {
            cx.listener(move |this: &mut Self, _, window, cx| {
                this.invoke(command, CommandArgs::None, window, cx);
            })
        };
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(title),
            )
            .when(
                self.app
                    .read(cx)
                    .engine
                    .commands()
                    .get(ids::VIEW_TOGGLE_PDF_PREVIEW)
                    .is_some(),
                |header| {
                    header.child(
                        Button::new("preview-pdf")
                            .ghost()
                            .xsmall()
                            .label(if layout.pdf { "Page" } else { "PDF" })
                            .on_click(command(ids::VIEW_TOGGLE_PDF_PREVIEW)),
                    )
                },
            )
            .child(
                Button::new("preview-pin")
                    .ghost()
                    .xsmall()
                    .label(if layout.pinned.is_some() {
                        "Unpin"
                    } else {
                        "Pin"
                    })
                    .on_click(command(ids::PREVIEW_PIN)),
            )
            .child(
                Button::new("preview-sync")
                    .ghost()
                    .xsmall()
                    .label(if layout.scroll_sync {
                        "Sync on"
                    } else {
                        "Sync off"
                    })
                    .on_click(command(ids::VIEW_TOGGLE_SCROLL_SYNC)),
            )
            .child(
                Button::new("preview-detach")
                    .ghost()
                    .xsmall()
                    .label(if layout.detached { "Dock" } else { "Detach" })
                    .on_click(command(ids::VIEW_DETACH_PREVIEW)),
            )
    }

    fn context<'a>(
        &self,
        highlight: blocks::Highlighter<'a>,
        window: &Window,
        cx: &App,
    ) -> blocks::BlockContext<'a> {
        let app = self.app.clone();
        let on_block: blocks::BlockHandler = Rc::new(move |span, window, cx| {
            let position = NavigationPosition {
                path: span.path.clone(),
                offset: span.range.start,
            };
            let result = app.update(cx, |app, cx| {
                app.invoke(ids::NAVIGATE_REVEAL, CommandArgs::Position(position), cx)
            });
            if let Err(error) = result {
                window.push_notification(error.to_string(), cx);
            }
        });
        let mut context = block_context(
            &self.app,
            self.extents.clone(),
            Some(self.list.clone()),
            on_block,
            highlight,
            window,
            cx,
        );
        context.inline_cache = Some(self.inline_cache.clone());
        context
    }
}

/// How blocks are drawn: the theme's fonts and colours, links that navigate through the engine,
/// and `on_block` for a click on a block.
pub(crate) fn block_context<'a>(
    app: &Entity<AppState>,
    extents: sync::Extents,
    list: Option<gpui::ListState>,
    on_block: blocks::BlockHandler,
    highlight: blocks::Highlighter<'a>,
    window: &Window,
    cx: &App,
) -> blocks::BlockContext<'a> {
    let theme = cx.theme();
    let app = app.clone();
    let on_link: inlines::LinkHandler = Rc::new(move |link, window, cx| match link {
        Link::Url(url) => cx.open_url(url),
        Link::File(path) => cx.open_with_system(path),
        Link::Xref { link, .. } => match &link.target {
            Some(target) => {
                let position = NavigationPosition {
                    path: target.path.clone(),
                    offset: target.range.start,
                };
                let result = app.update(cx, |app, cx| {
                    app.invoke(ids::NAVIGATE_GO_TO, CommandArgs::Position(position), cx)
                });
                if let Err(error) = result {
                    window.push_notification(error.to_string(), cx);
                }
            }
            None => window.push_notification("This reference does not resolve.", cx),
        },
    });
    blocks::BlockContext {
        extents,
        list,
        inline: inline_style(window, cx),
        palette: blocks::Palette {
            border: theme.border,
            surface: theme.muted,
            muted: theme.muted_foreground,
            note: theme.info,
            tip: theme.success,
            important: theme.primary,
            warning: theme.warning,
            caution: theme.danger,
            danger: theme.danger,
        },
        radius: theme.radius,
        on_link,
        on_block,
        highlight,
        inline_cache: None,
    }
}

/// The theme's fonts and colours for inline text.
fn inline_style(window: &Window, cx: &App) -> inlines::InlineStyle {
    let theme = cx.theme();
    inlines::InlineStyle {
        font: window.text_style().font(),
        mono_family: theme.mono_font_family.clone(),
        color: theme.foreground,
        muted: theme.muted_foreground,
        link: theme.link,
        unresolved: theme.danger,
        mark: theme.yellow_light,
        code_background: theme.muted,
        formula_backing: theme.mode.is_dark().then(gpui::white),
    }
}

/// Highlights `text` as `language` with the editor's tree-sitter grammars, cached.
pub(crate) fn highlight(
    cache: &RefCell<HighlightCache>,
    dark: bool,
    theme: &gpui_component::highlighter::HighlightTheme,
    language: &str,
    text: &str,
) -> Highlights {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (language, text).hash(&mut hasher);
    let key = (dark, hasher.finish());
    if let Some((cached_language, cached_text, styles)) = cache.borrow().get(&key) {
        if cached_language == language && cached_text == text {
            return styles.clone();
        }
    }
    let mut highlighter = SyntaxHighlighter::new(language);
    let rope = ropey::Rope::from_str(text);
    highlighter.update(None, &rope, None);
    let styles: Highlights = highlighter.styles(&(0..text.len()), theme).into();
    cache
        .borrow_mut()
        .insert(key, (language.to_owned(), text.to_owned(), styles.clone()));
    styles
}

impl PreviewView {
    /// Row `index` of `rows`, the rows of the view tree shown.
    fn row(
        &self,
        rows: &[blocks::Row],
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(shown) = self.shown.clone() else {
            return gpui::Empty.into_any_element();
        };
        let cache = self.highlights.clone();
        let dark = cx.theme().mode.is_dark();
        let theme = cx.theme().highlight_theme.clone();
        let highlight =
            move |language: &str, text: &str| highlight(&cache, dark, &theme, language, text);
        let context = self.context(&highlight, window, cx);
        blocks::row(&shown.view, rows, index, &context)
    }
}

impl gpui::EventEmitter<PreviewEvent> for PreviewView {}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::renders::rendered(crate::renders::View::Preview);
        self.finish_revealing(window, cx);
        // Repopulated as this frame's blocks are painted.
        self.extents.borrow_mut().clear();
        let header = self.header(cx);
        let waiting = self.shown.is_none() && self.app.read(cx).engine.preview_target().is_some();
        if let Some(shown) = &self.shown {
            // Laid out again for a new view tree, or a new font or theme.
            let style = inline_style(window, cx);
            let current = self
                .inline_for
                .as_ref()
                .is_some_and(|(of, kept)| Arc::ptr_eq(of, shown) && *kept == style);
            if !current {
                self.inline_cache.borrow_mut().clear();
                self.inline_for = Some((shown.clone(), style));
            }
        }
        let window_title = self.standalone.then(|| {
            let name = self
                .shown
                .as_ref()
                .and_then(|shown| shown.path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            TitleBar::new().child(format!("Preview — {name}"))
        });
        let layout = self.app.read(cx).engine.preview_layout();
        if layout.pdf {
            let target = self.app.read(cx).engine.preview_target().is_some();
            let pages = self.pdf.render(
                target,
                |this: &mut Self, path, offset, window, cx| {
                    let position = NavigationPosition { path, offset };
                    this.invoke(
                        ids::NAVIGATE_REVEAL,
                        CommandArgs::Position(position),
                        window,
                        cx,
                    );
                },
                cx,
            );
            return v_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .children(window_title)
                .child(header)
                .child(pages)
                .into_any_element();
        }
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .children(window_title)
            .child(header)
            .child(
                div()
                    .id("preview-scroll")
                    .flex_1()
                    .min_h_0()
                    .on_scroll_wheel(cx.listener(|_, _, window, cx| {
                        // After the scroll itself has been applied.
                        cx.defer_in(window, |this, _, cx| this.scrolled(cx));
                    }))
                    .when(self.shown.is_some(), |content| {
                        let rows = self.rows.clone();
                        content.child(
                            gpui::list(
                                self.list.clone(),
                                cx.processor(move |this, index: usize, window, cx| {
                                    this.row(&rows, index, window, cx)
                                }),
                            )
                            .size_full(),
                        )
                    })
                    .when(self.shown.is_none(), |content| {
                        content.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(if waiting {
                                    "Rendering…"
                                } else {
                                    "Open a page to preview it."
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// PDF export highlights source blocks with its own copy of the grammars, queries and light
/// colours the preview uses; the two must agree.
#[cfg(test)]
mod highlight_tests {
    use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};

    const SAMPLES: &[(&str, &str)] = &[
        (
            "rust",
            "/// Docs.\n#[derive(Debug)]\npub struct Point { x: i32 }\n\nfn main() {\n    let p = Point { x: 1 };\n    println!(\"{:?} {}\", p, 'c');\n    // done\n}\n",
        ),
        ("python", "import os\n\n@cache\ndef area(r: float) -> float:\n    \"\"\"Doc.\"\"\"\n    return 3.14 * r ** 2  # pi\n"),
        ("javascript", "const answer = 42;\nfunction greet(name) {\n  return `Hello ${name}`;\n}\nclass A extends B {}\n"),
        ("typescript", "interface Point { x: number }\nexport const p: Point = { x: 1 };\ntype Id = string | null;\n"),
        ("go", "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hi\", 3)\n}\n"),
        ("java", "public class App {\n  public static void main(String[] args) {\n    System.out.println(\"hi\" + 1);\n  }\n}\n"),
        ("bash", "#!/bin/sh\nfor f in *.adoc; do\n  echo \"$f\" | grep -q x && exit 1\ndone\n"),
        ("json", "{\"name\": \"x\", \"n\": [1, 2.5, true, null]}\n"),
        ("yaml", "name: adoc\nlist:\n  - 1\n  - true # yes\n"),
        ("toml", "[package]\nname = \"x\"\nversion = 1\nok = true\n"),
        ("sql", "SELECT id, name FROM users WHERE age > 21 ORDER BY name;\n"),
        ("html", "<!DOCTYPE html>\n<p class=\"x\">Hi <b>there</b></p>\n<script>let a = 1;</script>\n"),
    ];

    /// The colour of each byte as the preview styles it.
    fn preview(language: &str, code: &str) -> Vec<Option<[u8; 3]>> {
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &ropey::Rope::from_str(code), None);
        let mut colours = vec![None; code.len()];
        for (range, style) in
            highlighter.styles(&(0..code.len()), &*HighlightTheme::default_light())
        {
            let colour = style.color.map(|colour| {
                let rgba = gpui::Rgba::from(colour);
                [rgba.r, rgba.g, rgba.b].map(|channel| (channel * 255.0).round() as u8)
            });
            for byte in range {
                colours[byte] = colour;
            }
        }
        colours
    }

    /// The colour of each byte as PDF export styles it.
    fn pdf(language: &str, code: &str) -> Vec<Option<[u8; 3]>> {
        let runs = adoc_pdf::highlight::Highlighter::new()
            .highlight(language, code)
            .unwrap_or_else(|| panic!("{language} has no grammar"));
        let mut colours = vec![None; code.len()];
        for (range, style) in runs {
            for byte in range {
                colours[byte] = style.color;
            }
        }
        colours
    }

    fn close(left: Option<[u8; 3]>, right: Option<[u8; 3]>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => left.iter().zip(right).all(|(a, b)| a.abs_diff(b) <= 1),
            (left, right) => left == right,
        }
    }

    #[test]
    fn pdf_highlighting_matches_the_preview() {
        for (language, code) in SAMPLES {
            let (preview, pdf) = (preview(language, code), pdf(language, code));
            for (at, (left, right)) in preview.iter().zip(&pdf).enumerate() {
                if !code.is_char_boundary(at) || code[at..].starts_with(char::is_whitespace) {
                    continue;
                }
                assert!(
                    close(*left, *right),
                    "{language}: byte {at} ({:?}): preview {left:?}, pdf {right:?}",
                    &code[at..code.len().min(at + 12)]
                );
            }
        }
    }
}
