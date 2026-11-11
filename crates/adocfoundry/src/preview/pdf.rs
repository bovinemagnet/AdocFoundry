//! The preview's PDF pages: the engine's typeset pages of the preview target, rasterised from
//! SVG on a background task and drawn one under another at the zoom chosen (#420). A click on a
//! page reveals the source of the line of text under it.
//!
//! Only the pages near the view are drawn, at the scale the zoom and the display ask for (at
//! most [`SHARP_PIXELS`] each); a page is drawn again when that scale changes, and let go once it
//! leaves the view (#499). A new typesetting reuses the drawing of each page whose SVG did not
//! change. The view lets the GPU go of every drawing when it goes.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use adoc_engine::{PdfPreview, PreviewPage};
use gpui::{
    canvas, div, img, point, prelude::FluentBuilder as _, px, AppContext as _, Context,
    DispatchPhase, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    Point, RenderImage, ScrollEvent, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Task, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _,
};

use crate::image_decode::svg_to_bgra;

/// Space around and between the pages, in pixels at any zoom.
pub(crate) const PAGE_PADDING: f32 = 16.0;
pub(crate) const PAGE_GAP: f32 = 16.0;

/// The scales Zoom In and Zoom Out step through, 1 being a point to a pixel.
const ZOOM_STEPS: [f32; 12] = [
    0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0, 4.0,
];
/// The smallest and largest scales, whatever fits or a pinch asks for.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 4.0;
/// The most pixels a page is drawn at for the GPU, so zooming in on a large page cannot take
/// gigabytes: 64 MB of pixels, an A4 page at about 5.6 pixels to the point.
const SHARP_PIXELS: f32 = 16.0 * 1024.0 * 1024.0;
/// How long the zoom and the scroll position stay still before pages already drawn are drawn
/// again at the new scale.
const REDRAW_AFTER: Duration = Duration::from_millis(120);

/// How the pages are scaled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PdfZoom {
    /// The widest page fills the view's width. It follows the view's size.
    #[default]
    FitWidth,
    /// The largest page fits whole in the view. It follows the view's size.
    FitPage,
    /// A scale the user chose, 1 being a point to a pixel.
    Scale(f32),
}

/// A page drawn for the GPU, at `scale` pixels to the point.
#[derive(Clone)]
struct Drawn {
    image: Arc<RenderImage>,
    scale: f32,
}

/// The PDF pages on screen and those being rasterised.
#[derive(Default)]
pub(crate) struct PdfPages {
    /// The typeset preview the pages show.
    pub(crate) shown: Option<Arc<PdfPreview>>,
    /// The pages near the view, drawn, by index.
    drawn: HashMap<usize, Drawn>,
    generation: u64,
    _typeset: Option<Task<()>>,
    /// The drawings asked for, and the task making them.
    drawing: Vec<(usize, f32)>,
    _drawing: Option<Task<()>>,
    /// The display's pixels to a pixel of the view, as last drawn.
    display: Option<f32>,
    pub(crate) zoom: PdfZoom,
    pub(crate) scroll: ScrollHandle,
}

/// The key context of the PDF preview, where the zoom keys work.
pub const PDF_CONTEXT: &str = "PdfPreview";

/// A change of zoom asked for by a key, a menu or the palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomStep {
    In,
    Out,
    To(PdfZoom),
}

/// What a zoom keeps where it is: a point in the view, from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Focus {
    Centre,
    At(Point<Pixels>),
}

impl PdfPages {
    /// Draws the pages of `latest` near the view on a background task, unless it is already
    /// shown, reusing the drawing of each page whose SVG did not change; `finish` shows it.
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
            self.show(None, HashMap::new(), cx);
            return;
        };
        let Ok(pages) = &latest.pages else {
            self.show(Some(latest), HashMap::new(), cx);
            return;
        };
        // The pages in view are shown whole when the new ones are: drawn here, not after.
        let scale = scale_of(self.zoom, self.viewport(), pages);
        let wanted = scale * self.display.unwrap_or(1.0);
        let mut kept = HashMap::new();
        let mut jobs = Vec::new();
        for index in self.near(pages, scale) {
            let page = &pages[index];
            let target = draw_scale(page, wanted);
            let unchanged = self
                .pages()
                .get(index)
                .is_some_and(|old| old.svg == page.svg);
            match self.drawn.get(&index) {
                Some(drawn) if unchanged && same_scale(drawn.scale, target) => {
                    kept.insert(index, drawn.clone());
                }
                _ => jobs.push((index, page.svg.clone(), target)),
            }
        }
        let task = cx.background_spawn(async move { draw(jobs) });
        self._typeset = Some(cx.spawn(async move |this, cx| {
            let drawn = task.await;
            let _ = this.update(cx, |view, cx| {
                let pages = finish(view);
                if pages.generation == generation {
                    kept.extend(drawn);
                    pages.show(Some(latest), kept, cx);
                }
            });
        }));
    }

    fn show<V: 'static>(
        &mut self,
        preview: Option<Arc<PdfPreview>>,
        drawn: HashMap<usize, Drawn>,
        cx: &mut Context<V>,
    ) {
        for (index, old) in std::mem::replace(&mut self.drawn, drawn) {
            let kept = self
                .drawn
                .get(&index)
                .is_some_and(|new| Arc::ptr_eq(&new.image, &old.image));
            if !kept {
                cx.drop_image(old.image, None);
            }
        }
        self.drawing.clear();
        self._drawing = None;
        self.shown = preview;
        cx.notify();
    }

    /// Lets the GPU go of every page drawn, as the view showing them goes.
    pub(crate) fn release(&mut self, cx: &mut gpui::App) {
        #[cfg(test)]
        RELEASED.with(|released| released.set(released.get() + self.drawn.len()));
        for (_, drawn) in self.drawn.drain() {
            cx.drop_image(drawn.image, None);
        }
    }

    /// The pages of `pages` near the view at `scale`.
    fn near(&self, pages: &[PreviewPage], scale: f32) -> Vec<usize> {
        let (_, height) = self.viewport();
        let top = -f32::from(self.scroll.offset().y);
        near_pages(pages, scale, top, height)
    }

    /// The pages shown, if they were typeset.
    fn pages(&self) -> &[PreviewPage] {
        self.shown
            .as_ref()
            .and_then(|shown| shown.pages.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }

    /// The view's size, as last laid out.
    fn viewport(&self) -> (f32, f32) {
        let size = self.scroll.bounds().size;
        (f32::from(size.width), f32::from(size.height))
    }

    /// The scale the pages are drawn at, 1 being a point to a pixel.
    pub(crate) fn scale(&self) -> f32 {
        scale_of(self.zoom, self.viewport(), self.pages())
    }

    /// The pages drawn, and the scale of each.
    #[cfg(test)]
    pub(crate) fn drawn_pages(&self) -> Vec<(usize, f32)> {
        let mut drawn: Vec<_> = self
            .drawn
            .iter()
            .map(|(index, drawn)| (*index, drawn.scale))
            .collect();
        drawn.sort_by_key(|(index, _)| *index);
        drawn
    }

    /// The scale as a percentage, as the toolbar shows it.
    pub(crate) fn zoom_percent(&self) -> u32 {
        (self.scale() * 100.0).round() as u32
    }

    /// Changes the zoom, keeping the point of the pages at `focus` where it is.
    pub(crate) fn set_zoom<V: 'static>(
        &mut self,
        zoom: PdfZoom,
        focus: Focus,
        cx: &mut Context<V>,
    ) {
        let viewport = self.viewport();
        let pages = self.pages();
        let before = scale_of(self.zoom, viewport, pages);
        let after = scale_of(zoom, viewport, pages);
        let focus = match focus {
            Focus::Centre => (viewport.0 / 2.0, viewport.1 / 2.0),
            Focus::At(at) => (f32::from(at.x), f32::from(at.y)),
        };
        let offset = self.scroll.offset();
        let scrolled = (-f32::from(offset.x), -f32::from(offset.y));
        let (x, y) = keep_place(pages, viewport, scrolled, focus, before, after);
        self.zoom = zoom;
        self.scroll.set_offset(point(px(-x), px(-y)));
        cx.notify();
    }

    pub(crate) fn zoom_in<V: 'static>(&mut self, cx: &mut Context<V>) {
        self.set_zoom(PdfZoom::Scale(step_in(self.scale())), Focus::Centre, cx);
    }

    pub(crate) fn zoom_out<V: 'static>(&mut self, cx: &mut Context<V>) {
        self.set_zoom(PdfZoom::Scale(step_out(self.scale())), Focus::Centre, cx);
    }

    /// Zooms as a key, a menu or the palette asks, about the centre of the view.
    pub(crate) fn step<V: 'static>(&mut self, step: ZoomStep, cx: &mut Context<V>) {
        match step {
            ZoomStep::In => self.zoom_in(cx),
            ZoomStep::Out => self.zoom_out(cx),
            ZoomStep::To(zoom) => self.set_zoom(zoom, Focus::Centre, cx),
        }
    }

    /// Multiplies the scale by `factor`, keeping the point of the pages at `at` where it is: a
    /// pinch, or a scroll with the platform's modifier.
    fn zoom_by<V: 'static>(&mut self, factor: f32, at: Point<Pixels>, cx: &mut Context<V>) {
        let scale = (self.scale() * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let origin = self.scroll.bounds().origin;
        self.set_zoom(PdfZoom::Scale(scale), Focus::At(at - origin), cx);
    }

    /// Asks for drawings of the pages near the view at the scale the zoom and the display ask
    /// for: at once for a page not drawn yet, after [`REDRAW_AFTER`] for one drawn at another
    /// scale. Lets go of the drawings of pages that left the view.
    fn draw_near<V: 'static>(
        &mut self,
        scale: f32,
        window: &Window,
        cx: &mut Context<V>,
        finish: fn(&mut V) -> &mut PdfPages,
    ) {
        let Some(shown) = self.shown.clone() else {
            return;
        };
        let Ok(pages) = &shown.pages else {
            return;
        };
        let near = self.near(pages, scale);
        let wanted = scale * window.scale_factor();
        let mut plan = Vec::new();
        for (index, page) in pages.iter().enumerate() {
            let drawn = self.drawn.get(&index).map(|drawn| drawn.scale);
            let target = near.contains(&index).then(|| draw_scale(page, wanted));
            match (target, drawn) {
                (Some(target), Some(scale)) if same_scale(scale, target) => {}
                (Some(target), _) => plan.push((index, target)),
                (None, Some(_)) => {
                    if let Some(old) = self.drawn.remove(&index) {
                        cx.drop_image(old.image, None);
                    }
                }
                (None, None) => {}
            }
        }
        if plan == self.drawing {
            return;
        }
        self.drawing = plan.clone();
        if plan.is_empty() {
            self._drawing = None;
            return;
        }
        let redrawing = plan.iter().all(|(index, _)| self.drawn.contains_key(index));
        let jobs: Vec<(usize, String, f32)> = plan
            .iter()
            .map(|&(index, scale)| (index, pages[index].svg.clone(), scale))
            .collect();
        let timer = redrawing.then(|| cx.background_executor().timer(REDRAW_AFTER));
        self._drawing = Some(cx.spawn(async move |this, cx| {
            if let Some(timer) = timer {
                timer.await;
            }
            let drawn = cx.background_spawn(async move { draw(jobs) }).await;
            let _ = this.update(cx, |view, cx| {
                let pages = finish(view);
                if !pages
                    .shown
                    .as_ref()
                    .is_some_and(|now| Arc::ptr_eq(now, &shown))
                {
                    return;
                }
                for (index, drawn) in drawn {
                    if let Some(old) = pages.drawn.insert(index, drawn) {
                        cx.drop_image(old.image, None);
                    }
                }
                pages.drawing.clear();
                cx.notify();
            });
        }));
    }

    /// The pages, or what stands in for them, a toolbar for the zoom, and a handler for clicks
    /// on them. `finish` reaches these pages from the view that owns them.
    pub(crate) fn render<V: 'static>(
        &mut self,
        waiting: bool,
        on_click: impl Fn(&mut V, PathBuf, usize, &mut Window, &mut Context<V>) + Clone + 'static,
        finish: fn(&mut V) -> &mut PdfPages,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let message = |text: String, cx: &Context<V>| {
            div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(text))
        };
        // Known before the first pages come, so they are drawn for the display at once.
        self.display = Some(window.scale_factor());
        let scale = self.scale();
        let content = match self.shown.clone().as_ref().map(|shown| &shown.pages) {
            None => message(
                if waiting {
                    "Typesetting…".to_owned()
                } else {
                    "Open a page to preview it.".to_owned()
                },
                cx,
            )
            .into_any_element(),
            Some(Err(error)) => {
                message(format!("The page cannot be typeset: {error}"), cx).into_any_element()
            }
            Some(Ok(pages)) => {
                self.draw_near(scale, window, cx, finish);
                let tops = page_tops(pages, scale);
                let scroll = self.scroll.clone();
                v_flex()
                    .p(px(PAGE_PADDING))
                    .gap(px(PAGE_GAP))
                    .items_center()
                    .children(pages.iter().enumerate().map(|(index, page)| {
                        let image = self.drawn.get(&index).map(|drawn| drawn.image.clone());
                        let anchors = page.anchors.clone();
                        let top = tops[index];
                        let scroll = scroll.clone();
                        let on_click = on_click.clone();
                        div()
                            .id(("pdf-page", index))
                            .flex_none()
                            .w(px(page.width as f32 * scale))
                            .h(px(page.height as f32 * scale))
                            .bg(gpui::white())
                            .border_1()
                            .border_color(cx.theme().border)
                            .when_some(image, |page, image| page.child(img(image).size_full()))
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
                                            anchor_at(&anchors, f32::from(y) / scale)
                                        {
                                            on_click(view, path, offset, window, cx);
                                        }
                                    },
                                ),
                            )
                    }))
                    .into_any_element()
            }
        };
        let this = cx.entity().downgrade();
        let scroll = self.scroll.clone();
        let measured = std::rc::Rc::new(std::cell::Cell::new(self.scroll.bounds().size));
        v_flex()
            .flex_1()
            .min_h_0()
            .child(self.toolbar(finish, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("pdf-preview-scroll")
                            .debug_selector(|| "pdf-preview-scroll".to_owned())
                            .size_full()
                            .overflow_scroll()
                            .bg(cx.theme().muted)
                            .track_scroll(&self.scroll)
                            .on_pinch(cx.listener(move |view, event: &gpui::PinchEvent, _, cx| {
                                finish(view).zoom_by(1.0 + event.delta, event.position, cx);
                            }))
                            .child(content),
                    )
                    .child(
                        // Over the view, which it does not scroll with: measures it, so a fit
                        // follows its size, and zooms on a scroll with the platform's
                        // modifier before the view scrolls.
                        canvas(
                            move |_, window, _| {
                                let size = scroll.bounds().size;
                                if size != measured.get() {
                                    measured.set(size);
                                    window.refresh();
                                }
                            },
                            move |bounds, _, window, _| {
                                let this = this.clone();
                                window.on_mouse_event(
                                    move |event: &ScrollEvent, phase, window, cx| {
                                        if phase != DispatchPhase::Capture
                                            || !event.modifiers.secondary(cx)
                                            || !bounds.contains(&event.position)
                                        {
                                            return;
                                        }
                                        cx.stop_propagation();
                                        let delta = event.delta.pixel_delta(window.line_height()).y;
                                        let factor = wheel_factor(f32::from(delta));
                                        let at = event.position;
                                        let _ = this.update(cx, |view, cx| {
                                            finish(view).zoom_by(factor, at, cx);
                                        });
                                    },
                                );
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            )
    }

    /// The zoom's buttons: out, the scale, in, fit the width, fit the page, and actual size.
    fn toolbar<V: 'static>(
        &self,
        finish: fn(&mut V) -> &mut PdfPages,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let shown = self.shown.is_some();
        let button = |id: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .disabled(!shown)
                .debug_selector(move || id.to_owned())
        };
        let zoom = |zoom: PdfZoom| {
            cx.listener(move |view: &mut V, _, _, cx| {
                finish(view).set_zoom(zoom, Focus::Centre, cx);
            })
        };
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .justify_end()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .child(
                button("pdf-zoom-out")
                    .icon(IconName::Minus)
                    .tooltip("Zoom Out")
                    .on_click(cx.listener(move |view: &mut V, _, _, cx| {
                        finish(view).zoom_out(cx);
                    })),
            )
            .child(
                div()
                    .debug_selector(|| "pdf-zoom".to_owned())
                    .w(px(40.))
                    .text_center()
                    .when(shown, |zoom| {
                        zoom.child(format!("{}%", self.zoom_percent()))
                    }),
            )
            .child(
                button("pdf-zoom-in")
                    .icon(IconName::Plus)
                    .tooltip("Zoom In")
                    .on_click(cx.listener(move |view: &mut V, _, _, cx| {
                        finish(view).zoom_in(cx);
                    })),
            )
            .child(
                button("pdf-fit-width")
                    .label("Width")
                    .tooltip("Fit Width")
                    .selected(self.zoom == PdfZoom::FitWidth)
                    .on_click(zoom(PdfZoom::FitWidth)),
            )
            .child(
                button("pdf-fit-page")
                    .label("Page")
                    .tooltip("Fit to Window")
                    .selected(self.zoom == PdfZoom::FitPage)
                    .on_click(zoom(PdfZoom::FitPage)),
            )
            .child(
                button("pdf-actual-size")
                    .label("100%")
                    .tooltip("Actual Size")
                    .on_click(zoom(PdfZoom::Scale(1.0))),
            )
    }
}

#[cfg(test)]
thread_local! {
    /// How many drawn pages views let go of as they went, in a test.
    pub(crate) static RELEASED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Each `(index, svg, scale)` drawn for the GPU, as BGRA pixels; a page that cannot be drawn is
/// left out, and shows blank.
fn draw(jobs: Vec<(usize, String, f32)>) -> HashMap<usize, Drawn> {
    jobs.into_iter()
        .filter_map(|(index, svg, scale)| {
            let pixels = svg_to_bgra(&svg, scale, SHARP_PIXELS).ok()?;
            let image = RenderImage::new(vec![image::Frame::new(pixels)]);
            Some((
                index,
                Drawn {
                    image: Arc::new(image),
                    scale,
                },
            ))
        })
        .collect()
}

/// The scale `zoom` draws `pages` at in a view of `viewport` pixels, 1 when there is nothing
/// to fit.
pub(crate) fn scale_of(zoom: PdfZoom, viewport: (f32, f32), pages: &[PreviewPage]) -> f32 {
    let widest = pages
        .iter()
        .map(|page| page.width as f32)
        .fold(0.0, f32::max);
    let tallest = pages
        .iter()
        .map(|page| page.height as f32)
        .fold(0.0, f32::max);
    let room = (
        viewport.0 - 2.0 * PAGE_PADDING,
        viewport.1 - 2.0 * PAGE_PADDING,
    );
    let fit = |scale: f32| {
        if scale.is_finite() && scale > 0.0 {
            scale.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            1.0
        }
    };
    match zoom {
        PdfZoom::Scale(scale) => scale,
        _ if widest <= 0.0 || room.0 <= 0.0 => 1.0,
        PdfZoom::FitWidth => fit(room.0 / widest),
        PdfZoom::FitPage if room.1 <= 0.0 => fit(room.0 / widest),
        PdfZoom::FitPage => fit((room.0 / widest).min(room.1 / tallest)),
    }
}

/// The next of [`ZOOM_STEPS`] above `scale`.
fn step_in(scale: f32) -> f32 {
    ZOOM_STEPS
        .into_iter()
        .find(|step| *step > scale * 1.01)
        .unwrap_or(MAX_ZOOM)
}

/// The next of [`ZOOM_STEPS`] below `scale`, or a step further down below the first.
fn step_out(scale: f32) -> f32 {
    ZOOM_STEPS
        .into_iter()
        .rev()
        .find(|step| *step < scale * 0.99)
        .unwrap_or((scale / 1.25).max(MIN_ZOOM))
}

/// How much one scroll with the platform's modifier zooms: scrolling up zooms in.
fn wheel_factor(delta_y: f32) -> f32 {
    (delta_y * 0.005).exp().clamp(0.5, 2.0)
}

/// Where each page's top is in the scrolled content, in pixels, at `scale`.
pub(crate) fn page_tops(pages: &[PreviewPage], scale: f32) -> Vec<f32> {
    let mut top = PAGE_PADDING;
    pages
        .iter()
        .map(|page| {
            let this = top;
            top += page.height as f32 * scale + PAGE_GAP;
            this
        })
        .collect()
}

/// The size of the scrolled content at `scale` in a view of `viewport`: the pages are centred
/// across whichever is wider.
fn content_size(pages: &[PreviewPage], viewport: (f32, f32), scale: f32) -> (f32, f32) {
    let widest = pages
        .iter()
        .map(|page| page.width as f32)
        .fold(0.0, f32::max);
    let height = page_tops(pages, scale)
        .last()
        .zip(pages.last())
        .map_or(2.0 * PAGE_PADDING, |(top, page)| {
            top + page.height as f32 * scale + PAGE_PADDING
        });
    (
        (widest * scale + 2.0 * PAGE_PADDING).max(viewport.0),
        height,
    )
}

/// The scroll position, from the top left, that keeps the point of the pages at `focus` in the
/// view where it is when the scale goes from `before` to `after`.
fn keep_place(
    pages: &[PreviewPage],
    viewport: (f32, f32),
    scrolled: (f32, f32),
    focus: (f32, f32),
    before: f32,
    after: f32,
) -> (f32, f32) {
    let (width_before, _) = content_size(pages, viewport, before);
    let (width_after, height_after) = content_size(pages, viewport, after);
    // Across: from the centre the pages are drawn about.
    let across = (scrolled.0 + focus.0 - width_before / 2.0) / before;
    let x = width_after / 2.0 + across * after - focus.0;
    // Down: the same point of the same page.
    let y = scrolled.1 + focus.1;
    let tops_before = page_tops(pages, before);
    let tops_after = page_tops(pages, after);
    let y = match tops_before.iter().rposition(|top| *top <= y) {
        Some(page) => tops_after[page] + (y - tops_before[page]) / before * after - focus.1,
        // In the padding above the first page, which does not scale.
        None => y - focus.1,
    };
    (
        x.clamp(0.0, (width_after - viewport.0).max(0.0)),
        y.clamp(0.0, (height_after - viewport.1).max(0.0)),
    )
}

/// The pages within a view's height of the part of the content in view, from `top` down
/// `height` pixels, at `scale`.
fn near_pages(pages: &[PreviewPage], scale: f32, top: f32, height: f32) -> Vec<usize> {
    let (from, to) = (top - height, top + 2.0 * height);
    page_tops(pages, scale)
        .into_iter()
        .zip(pages)
        .enumerate()
        .filter(|(_, (page_top, page))| {
            *page_top < to && page_top + page.height as f32 * scale > from
        })
        .map(|(index, _)| index)
        .collect()
}

/// The scale to draw `page` at for `wanted` pixels to the point, at most [`SHARP_PIXELS`] in
/// all.
fn draw_scale(page: &PreviewPage, wanted: f32) -> f32 {
    let (width, height) = (page.width as f32, page.height as f32);
    let most = (SHARP_PIXELS / (width * height).max(1.0)).sqrt();
    wanted.min(most)
}

/// Whether a drawing at `drawn` pixels to the point will do for `wanted`.
fn same_scale(drawn: f32, wanted: f32) -> bool {
    (drawn / wanted - 1.0).abs() < 0.05
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

    use adoc_engine::{PageAnchor, PreviewPage};

    use super::*;

    fn anchor(top: f64, bottom: f64, offset: usize) -> PageAnchor {
        PageAnchor {
            top,
            bottom,
            path: PathBuf::from("/doc/page.adoc"),
            offset,
        }
    }

    /// `count` A4 pages.
    fn a4(count: usize) -> Vec<PreviewPage> {
        (0..count)
            .map(|_| PreviewPage {
                width: 595.0,
                height: 842.0,
                svg: String::new(),
                anchors: Vec::new(),
            })
            .collect()
    }

    fn close(left: f32, right: f32) -> bool {
        (left - right).abs() < 0.01
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

    #[test]
    fn the_fits_follow_the_view() {
        let pages = a4(2);
        // 627 pixels wide leaves 595 for the page: a point to a pixel.
        assert!(close(
            scale_of(PdfZoom::FitWidth, (627.0, 400.0), &pages),
            1.0
        ));
        assert!(close(
            scale_of(PdfZoom::FitWidth, (1222.0, 400.0), &pages),
            2.0
        ));
        // The whole page: the height decides in a short view.
        assert!(close(
            scale_of(PdfZoom::FitPage, (1222.0, 453.0), &pages),
            0.5
        ));
        assert!(close(
            scale_of(PdfZoom::FitPage, (329.5, 2000.0), &pages),
            0.5
        ));
        assert_eq!(scale_of(PdfZoom::Scale(1.5), (100.0, 100.0), &pages), 1.5);
        // Nothing laid out yet, or nothing to fit.
        assert_eq!(scale_of(PdfZoom::FitWidth, (0.0, 0.0), &pages), 1.0);
        assert_eq!(scale_of(PdfZoom::FitPage, (500.0, 500.0), &[]), 1.0);
    }

    #[test]
    fn zooming_steps_through_the_scales_and_stops_at_the_ends() {
        assert_eq!(step_in(1.0), 1.1);
        assert_eq!(step_in(1.05), 1.1);
        assert_eq!(step_out(1.0), 0.9);
        assert_eq!(step_out(0.95), 0.9);
        assert_eq!(step_in(4.0), MAX_ZOOM);
        assert_eq!(step_out(0.5), 0.4);
        assert_eq!(step_out(0.1), MIN_ZOOM);
        assert!(wheel_factor(20.0) > 1.0 && wheel_factor(-20.0) < 1.0);
    }

    #[test]
    fn pages_and_clicks_scale_with_the_zoom() {
        let pages = a4(3);
        assert_eq!(page_tops(&pages, 1.0), [16.0, 874.0, 1732.0]);
        assert_eq!(page_tops(&pages, 0.5), [16.0, 453.0, 890.0]);
        assert_eq!(content_size(&pages, (300.0, 300.0), 0.5), (329.5, 1327.0));
        assert_eq!(content_size(&pages, (900.0, 300.0), 0.5).0, 900.0);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_focus() {
        let pages = a4(3);
        let viewport = (627.0, 600.0);
        // The second page's point 100 down, 50 from its left, is at the focus.
        let tops = page_tops(&pages, 1.0);
        let scrolled = (0.0, tops[1] + 100.0 - 300.0);
        let focus = (16.0 + 50.0, 300.0);
        let (x, y) = keep_place(&pages, viewport, scrolled, focus, 1.0, 2.0);
        let tops_after = page_tops(&pages, 2.0);
        assert!(close(y + focus.1, tops_after[1] + 200.0));
        // The page is 1222 wide at 2, its left at 16.
        assert!(close(x + focus.0, 16.0 + 100.0));
        // Never past the content's ends.
        assert_eq!(
            keep_place(&pages, viewport, (0.0, 0.0), (0.0, 0.0), 1.0, 0.5),
            (0.0, 0.0)
        );
    }

    #[test]
    fn only_pages_near_the_view_are_drawn_and_never_too_large() {
        let pages = a4(50);
        let tops = page_tops(&pages, 1.0);
        // Scrolled to the fifth page, 600 high: the fourth to the sixth, of fifty.
        assert_eq!(near_pages(&pages, 1.0, tops[4], 600.0), [3, 4, 5]);
        assert_eq!(near_pages(&pages, 1.0, 0.0, 600.0), [0, 1]);
        // Drawn at the scale asked for, however small.
        assert_eq!(draw_scale(&pages[0], 0.75), 0.75);
        assert_eq!(draw_scale(&pages[0], 4.0), 4.0);
        // 400% on a display of two: capped by the pixels allowed.
        let capped = draw_scale(&pages[0], 8.0);
        assert!(capped < 8.0 && 595.0 * 842.0 * capped * capped <= SHARP_PIXELS * 1.001);
        assert!(same_scale(2.0, 2.04) && !same_scale(2.0, 2.2));
    }
}
