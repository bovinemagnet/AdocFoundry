//! The preview's PDF pages: the engine's typeset pages of the preview target, rasterised from
//! SVG on a background task and drawn one under another. A click on a page reveals the source
//! of the line of text under it.

use std::{path::PathBuf, sync::Arc};

use adoc_engine::{PdfPreview, PreviewPage};
use gpui::{
    div, img, px, AppContext as _, Context, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderImage, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Task,
};
use gpui_component::{v_flex, ActiveTheme as _};

use crate::image_decode::{decode, Limits};

/// Space around and between the pages, in pixels.
pub(crate) const PAGE_PADDING: f32 = 16.0;
pub(crate) const PAGE_GAP: f32 = 16.0;

/// The PDF pages on screen and those being rasterised.
#[derive(Default)]
pub(crate) struct PdfPages {
    /// The typeset preview the images show.
    pub(crate) shown: Option<Arc<PdfPreview>>,
    images: Vec<Arc<RenderImage>>,
    generation: u64,
    _rasterising: Option<Task<()>>,
    pub(crate) scroll: ScrollHandle,
}

type Rasterised = Result<Vec<Arc<RenderImage>>, String>;

impl PdfPages {
    /// Rasterises `latest` on a background task, unless it is already shown; `finish` shows it.
    pub(crate) fn update<V: 'static>(
        &mut self,
        latest: Option<Arc<PdfPreview>>,
        cx: &mut Context<V>,
        finish: fn(&mut V) -> &mut PdfPages,
    ) {
        let same = match (&self.shown, &latest) {
            (Some(shown), Some(latest)) => Arc::ptr_eq(shown, latest),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let Some(latest) = latest else {
            self.show(None, Ok(Vec::new()), cx);
            return;
        };
        let pages: Vec<String> = match &latest.pages {
            Ok(pages) => pages.iter().map(|page| page.svg.clone()).collect(),
            Err(_) => Vec::new(),
        };
        let task = cx.background_spawn(async move { rasterise(&pages) });
        self._rasterising = Some(cx.spawn(async move |this, cx| {
            let images = task.await;
            let _ = this.update(cx, |view, cx| {
                let pages = finish(view);
                if pages.generation == generation {
                    pages.show(Some(latest), images, cx);
                }
            });
        }));
    }

    fn show<V: 'static>(
        &mut self,
        preview: Option<Arc<PdfPreview>>,
        images: Rasterised,
        cx: &mut Context<V>,
    ) {
        let images = images.unwrap_or_default();
        for old in std::mem::replace(&mut self.images, images) {
            cx.drop_image(old, None);
        }
        self.shown = preview;
        cx.notify();
    }

    /// The pages, or what stands in for them, and a handler for clicks on them.
    pub(crate) fn render<V: 'static>(
        &self,
        waiting: bool,
        on_click: impl Fn(&mut V, PathBuf, usize, &mut gpui::Window, &mut Context<V>) + Clone + 'static,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let message = |text: String| {
            div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(text))
        };
        let content = match self.shown.as_ref().map(|shown| &shown.pages) {
            None => message(if waiting {
                "Typesetting…".to_owned()
            } else {
                "Open a page to preview it.".to_owned()
            })
            .into_any_element(),
            Some(Err(error)) => {
                message(format!("The page cannot be typeset: {error}")).into_any_element()
            }
            Some(Ok(pages)) => {
                let tops = page_tops(pages);
                let scroll = self.scroll.clone();
                v_flex()
                    .p(px(PAGE_PADDING))
                    .gap(px(PAGE_GAP))
                    .items_center()
                    .children(pages.iter().zip(&self.images).enumerate().map(
                        |(index, (page, image))| {
                            let anchors = page.anchors.clone();
                            let top = tops[index];
                            let scroll = scroll.clone();
                            let on_click = on_click.clone();
                            div()
                                .id(("pdf-page", index))
                                .flex_none()
                                .w(px(page.width as f32))
                                .h(px(page.height as f32))
                                .bg(gpui::white())
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(img(image.clone()).size_full())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                                            let container = scroll.bounds();
                                            let y = event.position.y
                                                - container.origin.y
                                                - scroll.offset().y
                                                - px(top);
                                            if let Some((path, offset)) =
                                                anchor_at(&anchors, f32::from(y))
                                            {
                                                on_click(view, path, offset, window, cx);
                                            }
                                        },
                                    ),
                                )
                        },
                    ))
                    .into_any_element()
            }
        };
        div()
            .id("pdf-preview-scroll")
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .bg(cx.theme().muted)
            .track_scroll(&self.scroll)
            .child(content)
    }
}

/// Each page's SVG drawn for the GPU, as BGRA pixels.
fn rasterise(pages: &[String]) -> Rasterised {
    pages
        .iter()
        .map(|svg| {
            let decoded = decode(svg.as_bytes(), "svg", Limits::DEFAULT)
                .map_err(|error| error.to_string())?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(
                decoded.pixels,
            )])))
        })
        .collect()
}

/// Where each page's top is in the scrolled content, in pixels: one point of the page is one
/// pixel on screen.
pub(crate) fn page_tops(pages: &[PreviewPage]) -> Vec<f32> {
    let mut top = PAGE_PADDING;
    pages
        .iter()
        .map(|page| {
            let this = top;
            top += page.height as f32 + PAGE_GAP;
            this
        })
        .collect()
}

/// The source of the line of text at `y` points down a page: the line it falls in, else the
/// last line above it, else the first line on the page.
pub(crate) fn anchor_at(anchors: &[adoc_engine::PageAnchor], y: f32) -> Option<(PathBuf, usize)> {
    let y = f64::from(y);
    anchors
        .iter()
        .find(|anchor| anchor.top <= y && y <= anchor.bottom)
        .or_else(|| anchors.iter().rev().find(|anchor| anchor.top <= y))
        .or_else(|| anchors.first())
        .map(|anchor| (anchor.path.clone(), anchor.offset))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use adoc_engine::PageAnchor;

    use super::anchor_at;

    fn anchor(top: f64, bottom: f64, offset: usize) -> PageAnchor {
        PageAnchor {
            top,
            bottom,
            path: PathBuf::from("/doc/page.adoc"),
            offset,
        }
    }

    #[test]
    fn a_click_goes_to_the_line_under_it_or_above_it() {
        let anchors = [
            anchor(50.0, 62.0, 0),
            anchor(80.0, 92.0, 10),
            anchor(100.0, 112.0, 20),
        ];
        assert_eq!(anchor_at(&anchors, 85.0).unwrap().1, 10);
        // Between lines: the line above.
        assert_eq!(anchor_at(&anchors, 95.0).unwrap().1, 10);
        // Above every line: the first.
        assert_eq!(anchor_at(&anchors, 10.0).unwrap().1, 0);
        assert_eq!(anchor_at(&anchors, 500.0).unwrap().1, 20);
        assert!(anchor_at(&[], 10.0).is_none());
    }
}
