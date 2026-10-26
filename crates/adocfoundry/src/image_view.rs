//! The image viewer: what an image tab shows. It reads and decodes the file on a background
//! thread whenever the engine gives the image a new revision (on opening, a change on disk, or
//! Reload), and shows it on a canvas that fits it to the window, zooms and pans, over a
//! checkerboard where it is transparent. A result for any load but the latest is dropped, and
//! the GPU's copy of a replaced or closed image is released.
//!
//! Actual size (100%) is one image pixel to one screen pixel, so a screenshot appears at the size
//! it was taken, on any display; an SVG's actual size is its own width and height, in the
//! interface's units. An image first opens whole, centred, and never enlarged past actual size.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use adoc_engine::{commands::ids, CommandArgs, EngineEvent, ImageId};
use gpui::{
    actions, canvas, div, fill, img, point, prelude::FluentBuilder as _, px, size, App,
    AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Pixels, Point, Render, RenderImage, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{ContextMenuExt as _, PopupMenuItem},
    v_flex, ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _,
};

use crate::{
    app::AppState,
    editor::bindings::{platform_binding, ReloadImage, RevealInFiles},
    image_decode::{byte_size, decode_file, DecodeError, ImageInfo, Limits},
};

actions!(adocfoundry, [ZoomIn, ZoomOut, ActualSize, FitToWindow]);

/// The key context of the image viewer, where its zoom keys work.
pub const IMAGE_CONTEXT: &str = "ImageViewer";

/// Each Zoom In or Zoom Out multiplies or divides the scale by this.
const ZOOM_STEP: f32 = 1.25;
/// The smallest and largest scales, where 1 is actual size.
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 32.0;
/// The side of a checkerboard square, behind transparent pixels.
const CHECKER: f32 = 8.0;

/// How the image is scaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// Whole and centred, never enlarged past actual size: how an image first opens. It follows
    /// the window's size.
    Initial,
    /// Fit to Window: whole and as large as the canvas allows. It follows the window's size.
    Fit,
    /// A scale the user chose, kept as the window changes size.
    Scale(f32),
}

/// A decoded image on screen.
struct Shown {
    image: Arc<RenderImage>,
    info: ImageInfo,
}

/// Where loading the file stands.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    Loading,
    Ready,
    /// The last load failed. A picture from an earlier load may still be shown, as stale.
    Failed(DecodeError),
}

type LoadResult = Result<(ImageInfo, Arc<RenderImage>), DecodeError>;

pub struct ImageView {
    app: Entity<AppState>,
    id: ImageId,
    path: PathBuf,
    focus_handle: FocusHandle,
    shown: Option<Shown>,
    state: LoadState,
    /// Counts loads begun; only the latest one's result is shown.
    generation: u64,
    _loading: Option<Task<()>>,
    zoom: Zoom,
    scroll: ScrollHandle,
    /// The canvas's size when it was last laid out, to fit the image to.
    viewport: Rc<Cell<gpui::Size<Pixels>>>,
    /// Where a drag that pans the image last was.
    drag: Option<Point<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl ImageView {
    pub fn new(app: Entity<AppState>, id: ImageId, cx: &mut Context<Self>) -> Self {
        let path = app
            .read(cx)
            .engine
            .image(id)
            .map(|image| image.path.clone())
            .unwrap_or_default();
        let subscriptions = vec![
            cx.subscribe(&app, move |this, _, event: &EngineEvent, cx| {
                if *event == EngineEvent::ImageChanged(id) {
                    this.load(cx);
                }
            }),
            // The GPU keeps an image until told to let it go.
            cx.on_release(|this, cx| {
                if let Some(shown) = this.shown.take() {
                    cx.drop_image(shown.image, None);
                }
            }),
        ];
        let mut view = Self {
            app,
            id,
            path,
            focus_handle: cx.focus_handle(),
            shown: None,
            state: LoadState::Loading,
            generation: 0,
            _loading: None,
            zoom: Zoom::Initial,
            scroll: ScrollHandle::new(),
            viewport: Rc::default(),
            drag: None,
            _subscriptions: subscriptions,
        };
        view.load(cx);
        view
    }

    #[must_use]
    pub fn image(&self) -> ImageId {
        self.id
    }

    #[cfg(test)]
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// What is known of the image shown, if one is.
    #[cfg(test)]
    pub fn info(&self) -> Option<&ImageInfo> {
        self.shown.as_ref().map(|shown| &shown.info)
    }

    #[cfg(test)]
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// Reads and decodes the file again, on a background thread; a load already under way is
    /// dropped.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state = LoadState::Loading;
        let path = self.path.clone();
        let task = cx.background_spawn(async move { load_file(&path) });
        self._loading = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| this.finish_load(generation, result, cx));
        }));
        cx.notify();
    }

    /// Shows the result of load `generation`, unless a later load has begun since.
    pub(crate) fn finish_load(
        &mut self,
        generation: u64,
        result: LoadResult,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok((info, image)) => {
                let dimensions_changed = self.shown.as_ref().is_some_and(|shown| {
                    (shown.info.width, shown.info.height) != (info.width, info.height)
                });
                if let Some(old) = self.shown.replace(Shown { image, info }) {
                    cx.drop_image(old.image, None);
                }
                if dimensions_changed {
                    self.scroll.set_offset(point(px(0.), px(0.)));
                }
                self.state = LoadState::Ready;
            }
            Err(error) => self.state = LoadState::Failed(error),
        }
        cx.notify();
    }

    /// The load to finish next, for tests that finish loads themselves.
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Loads the image again, as Reload Image does, for tests.
    #[cfg(test)]
    pub fn reload_for_test(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    /// The image's size at actual size, in the interface's units.
    fn natural_size(&self, scale_factor: f32) -> Option<gpui::Size<Pixels>> {
        let info = &self.shown.as_ref()?.info;
        Some(match info.intrinsic {
            Some((width, height)) => size(px(width), px(height)),
            None => size(
                px(info.width as f32 / scale_factor),
                px(info.height as f32 / scale_factor),
            ),
        })
    }

    /// The scale the image is drawn at, 1 being actual size.
    fn scale(&self, scale_factor: f32) -> f32 {
        let fit = || {
            let viewport = self.viewport.get();
            let natural = self.natural_size(scale_factor)?;
            if viewport.width <= px(0.) || viewport.height <= px(0.) {
                return None;
            }
            Some(
                (viewport.width / natural.width)
                    .min(viewport.height / natural.height)
                    .clamp(MIN_ZOOM, MAX_ZOOM),
            )
        };
        match self.zoom {
            Zoom::Scale(scale) => scale,
            Zoom::Initial => fit().map_or(1.0, |fit| fit.min(1.0)),
            Zoom::Fit => fit().unwrap_or(1.0),
        }
    }

    /// The scale as a percentage, as the toolbar shows it.
    pub fn zoom_percent(&self, window: &Window) -> u32 {
        (self.scale(window.scale_factor()) * 100.0).round() as u32
    }

    /// Changes the zoom, keeping the point at the centre of the canvas where it is.
    fn set_zoom(&mut self, zoom: Zoom, window: &Window, cx: &mut Context<Self>) {
        let scale_factor = window.scale_factor();
        let Some(natural) = self.natural_size(scale_factor) else {
            return;
        };
        let viewport = self.viewport.get();
        let content = |scale: f32| {
            size(
                (natural.width * scale).max(viewport.width),
                (natural.height * scale).max(viewport.height),
            )
        };
        let before = content(self.scale(scale_factor));
        let offset = self.scroll.offset();
        let centre = point(
            (viewport.width / 2. - offset.x) / before.width,
            (viewport.height / 2. - offset.y) / before.height,
        );
        self.zoom = zoom;
        let after = content(self.scale(scale_factor));
        let x = (after.width * centre.x - viewport.width / 2.)
            .clamp(px(0.), (after.width - viewport.width).max(px(0.)));
        let y = (after.height * centre.y - viewport.height / 2.)
            .clamp(px(0.), (after.height - viewport.height).max(px(0.)));
        self.scroll.set_offset(point(-x, -y));
        cx.notify();
    }

    fn zoom_by(&mut self, factor: f32, window: &Window, cx: &mut Context<Self>) {
        let scale = (self.scale(window.scale_factor()) * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.set_zoom(Zoom::Scale(scale), window, cx);
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(ZOOM_STEP, window, cx);
    }

    fn on_zoom_out(&mut self, _: &ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_by(1.0 / ZOOM_STEP, window, cx);
    }

    fn on_actual_size(&mut self, _: &ActualSize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Scale(1.0), window, cx);
    }

    fn on_fit(&mut self, _: &FitToWindow, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Fit, window, cx);
    }

    /// Asks the engine for a new revision, which loads the file again.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let id = self.id;
        self.app.update(cx, |app, cx| {
            let _ = app.invoke(ids::IMAGE_RELOAD, CommandArgs::Image(id), cx);
        });
    }

    fn on_reload(&mut self, _: &ReloadImage, _: &mut Window, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    fn start_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.drag = Some(event.position);
    }

    fn drag_to(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let max = self.scroll.max_offset();
        let offset = self.scroll.offset() + (event.position - from);
        self.scroll.set_offset(point(
            offset.x.clamp(-max.x, px(0.)),
            offset.y.clamp(-max.y, px(0.)),
        ));
        self.drag = Some(event.position);
        cx.notify();
    }

    fn name(&self) -> String {
        file_name(&self.path)
    }

    fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let info = self.shown.as_ref().map(|shown| describe(&shown.info));
        let shown = self.shown.is_some();
        let percent = self.zoom_percent(window);
        let fitted = matches!(self.zoom, Zoom::Fit);
        let button = |id: &'static str| Button::new(id).ghost().xsmall().disabled(!shown);
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(
                div()
                    .debug_selector(|| "image-info".to_owned())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(info.unwrap_or_default()),
            )
            .child(
                button("image-zoom-out")
                    .icon(IconName::Minus)
                    .tooltip("Zoom Out")
                    .debug_selector(|| "image-zoom-out".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_zoom_out(&ZoomOut, window, cx);
                    })),
            )
            .child(
                div()
                    .debug_selector(|| "image-zoom".to_owned())
                    .w(px(48.))
                    .text_center()
                    .when(shown, |zoom| zoom.child(format!("{percent}%"))),
            )
            .child(
                button("image-zoom-in")
                    .icon(IconName::Plus)
                    .tooltip("Zoom In")
                    .debug_selector(|| "image-zoom-in".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_zoom_in(&ZoomIn, window, cx);
                    })),
            )
            .child(
                button("image-fit")
                    .label("Fit")
                    .selected(fitted)
                    .tooltip("Fit to Window")
                    .debug_selector(|| "image-fit".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_fit(&FitToWindow, window, cx);
                    })),
            )
            .child(
                button("image-actual-size")
                    .label("100%")
                    .tooltip("Actual Size")
                    .debug_selector(|| "image-actual-size".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_actual_size(&ActualSize, window, cx);
                    })),
            )
            .child(
                Button::new("image-reload")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Reload Image")
                    .debug_selector(|| "image-reload".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(open_externally_button("image-open-externally", &self.path))
    }

    fn render_canvas(
        &self,
        shown: &Shown,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let scale_factor = window.scale_factor();
        let natural = self.natural_size(scale_factor).unwrap_or_default();
        let scale = self.scale(scale_factor);
        let display = size(natural.width * scale, natural.height * scale);
        let viewport = self.viewport.clone();
        let scroll = self.scroll.clone();
        let content = {
            let viewport = viewport.get();
            size(
                display.width.max(viewport.width),
                display.height.max(viewport.height),
            )
        };
        let (light, dark) = checker_colours(cx);
        let transparent = shown.info.has_alpha;
        let path = self.path.clone();
        div()
            .id("image-canvas")
            .debug_selector(|| "image-canvas".to_owned())
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::start_drag))
            .on_mouse_move(cx.listener(Self::drag_to))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .child(
                // Measures the canvas: a new size fits the image again on the next frame.
                canvas(
                    move |_, window, _| {
                        let measured = scroll.bounds().size;
                        if measured != viewport.get() {
                            viewport.set(measured);
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(
                div()
                    .w(content.width)
                    .h(content.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .debug_selector(|| "image-picture".to_owned())
                            .relative()
                            .flex_none()
                            .w(display.width)
                            .h(display.height)
                            .when(transparent, |picture| {
                                picture.child(
                                    canvas(
                                        |_, _, _| {},
                                        move |bounds, _, window, _| {
                                            paint_checkerboard(bounds, light, dark, window);
                                        },
                                    )
                                    .absolute()
                                    .size_full(),
                                )
                            })
                            .child(img(shown.image.clone()).absolute().size_full()),
                    ),
            )
            .context_menu(move |menu, _, _| {
                let full = path.to_string_lossy().into_owned();
                let shown = path.clone();
                let external = path.clone();
                menu.menu("Zoom In", Box::new(ZoomIn))
                    .menu("Zoom Out", Box::new(ZoomOut))
                    .menu("Fit to Window", Box::new(FitToWindow))
                    .menu("Actual Size", Box::new(ActualSize))
                    .separator()
                    .menu("Reload Image", Box::new(ReloadImage))
                    .item(
                        PopupMenuItem::new("Open Externally").on_click(move |_, _, cx| {
                            crate::files_view::open_externally(&external, cx);
                        }),
                    )
                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
                    }))
                    .menu("Reveal in Files", Box::new(RevealInFiles))
                    .item(
                        PopupMenuItem::new(crate::files_view::reveal_label()).on_click(
                            move |_, _, cx| {
                                cx.reveal_path(&shown);
                            },
                        ),
                    )
            })
            .into_any_element()
    }

    fn render_message(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let name = self.name();
        let body = v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_4()
            .text_color(theme.muted_foreground);
        match &self.state {
            LoadState::Failed(error) => body
                .debug_selector(|| "image-error".to_owned())
                .child(
                    div()
                        .text_color(theme.foreground)
                        .child(format!("{name} cannot be shown")),
                )
                .child(div().text_sm().text_center().child(error.to_string()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("image-retry")
                                .label("Retry")
                                .small()
                                .debug_selector(|| "image-retry".to_owned())
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        )
                        .child(
                            Button::new("image-error-open-externally")
                                .label("Open Externally")
                                .small()
                                .debug_selector(|| "image-error-open-externally".to_owned())
                                .on_click({
                                    let path = self.path.clone();
                                    move |_, _, cx| crate::files_view::open_externally(&path, cx)
                                }),
                        ),
                )
                .into_any_element(),
            _ => body
                .debug_selector(|| "image-loading".to_owned())
                .child(format!("Loading {name}…"))
                .into_any_element(),
        }
    }
}

/// Reads and decodes `path` for the GPU, as BGRA pixels in one frame.
fn load_file(path: &Path) -> LoadResult {
    let decoded = decode_file(path, Limits::DEFAULT)?;
    let image = RenderImage::new(vec![image::Frame::new(decoded.pixels)]);
    Ok((decoded.info, Arc::new(image)))
}

/// What the toolbar says of an image: its format, size and file size.
fn describe(info: &ImageInfo) -> String {
    let dimensions = match info.intrinsic {
        Some((width, height)) => format!("{} × {}", width.round(), height.round()),
        None => format!("{} × {} px", info.width, info.height),
    };
    format!(
        "{} · {dimensions} · {}",
        info.format,
        byte_size(info.file_bytes)
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn open_externally_button(id: &'static str, path: &Path) -> Button {
    let path = path.to_path_buf();
    Button::new(id)
        .icon(IconName::ExternalLink)
        .ghost()
        .xsmall()
        .tooltip("Open Externally")
        .debug_selector(move || id.to_owned())
        .on_click(move |_, _, cx| crate::files_view::open_externally(&path, cx))
}

/// The checkerboard's two colours, quiet in the light theme and the dark.
fn checker_colours(cx: &App) -> (Hsla, Hsla) {
    let theme = cx.theme();
    if theme.is_dark() {
        (gpui::hsla(0., 0., 0.20, 1.), gpui::hsla(0., 0., 0.26, 1.))
    } else {
        (gpui::hsla(0., 0., 1., 1.), gpui::hsla(0., 0., 0.88, 1.))
    }
}

/// Paints the checkerboard behind the picture at `bounds`, only where it can be seen.
fn paint_checkerboard(bounds: Bounds<Pixels>, light: Hsla, dark: Hsla, window: &mut Window) {
    let visible = bounds.intersect(&window.content_mask().bounds);
    if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
        return;
    }
    window.paint_quad(fill(visible, light));
    let cell = px(CHECKER);
    let first_column = ((visible.left() - bounds.left()) / cell).floor() as i64;
    let last_column = ((visible.right() - bounds.left()) / cell).ceil() as i64;
    let first_row = ((visible.top() - bounds.top()) / cell).floor() as i64;
    let last_row = ((visible.bottom() - bounds.top()) / cell).ceil() as i64;
    for row in first_row..last_row {
        for column in first_column..last_column {
            if (row + column) % 2 == 0 {
                continue;
            }
            let square = Bounds::new(
                point(
                    bounds.left() + cell * column as f32,
                    bounds.top() + cell * row as f32,
                ),
                size(cell, cell),
            )
            .intersect(&visible);
            window.paint_quad(fill(square, dark));
        }
    }
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stale = matches!(self.state, LoadState::Failed(_)) && self.shown.is_some();
        let body = match &self.shown {
            Some(shown) => self.render_canvas(shown, window, cx),
            None => self.render_message(cx),
        };
        let theme = cx.theme();
        let banner = match (&self.state, stale) {
            (LoadState::Failed(error), true) => Some(
                h_flex()
                    .debug_selector(|| "image-stale".to_owned())
                    .px_2()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .bg(theme.warning.opacity(0.15))
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{error} Showing the version that last loaded.")),
                    )
                    .child(
                        Button::new("image-stale-retry")
                            .label("Retry")
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            ),
            _ => None,
        };
        v_flex()
            .id("image-viewer")
            .key_context(IMAGE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_actual_size))
            .on_action(cx.listener(Self::on_fit))
            .on_action(cx.listener(Self::on_reload))
            .child(self.render_toolbar(window, cx))
            .children(banner)
            .child(body)
    }
}

/// The viewer's zoom keys, as in macOS Preview: Command (Control elsewhere) with `=` and `-`
/// to zoom, `0` for actual size and `9` to fit.
pub fn bind(cx: &mut App) {
    let context = Some(IMAGE_CONTEXT);
    cx.bind_keys([
        KeyBinding::new(&platform_binding("cmd-="), ZoomIn, context),
        KeyBinding::new(&platform_binding("cmd-+"), ZoomIn, context),
        KeyBinding::new(&platform_binding("cmd--"), ZoomOut, context),
        KeyBinding::new(&platform_binding("cmd-0"), ActualSize, context),
        KeyBinding::new(&platform_binding("cmd-9"), FitToWindow, context),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolbar_names_format_size_and_file_size() {
        let info = ImageInfo {
            format: "PNG",
            width: 24,
            height: 16,
            file_bytes: 222,
            intrinsic: None,
            has_alpha: true,
        };
        assert_eq!(describe(&info), "PNG · 24 × 16 px · 222 bytes");
        let svg = ImageInfo {
            format: "SVG",
            width: 2048,
            height: 1024,
            file_bytes: 1536,
            intrinsic: Some((120.0, 60.0)),
            has_alpha: true,
        };
        assert_eq!(describe(&svg), "SVG · 120 × 60 · 1.5 KB");
    }
}
