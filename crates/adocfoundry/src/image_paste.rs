//! Pasting and dropping images into a page (#73): what the clipboard or a drop holds, and the
//! dialog naming a pasted image. The engine's `edit.place_images` does the rest; the source and
//! visual editors say where.

use std::{path::PathBuf, sync::Arc};

use adoc_engine::{is_image_path, DocumentId, ImageSource};
use gpui::{
    canvas, div, prelude::FluentBuilder as _, px, App, AppContext as _, ClipboardEntry,
    ClipboardItem, DispatchPhase, Entity, ExternalPaths, InteractiveElement as _, IntoElement,
    MouseMoveEvent, ParentElement as _, PlatformInput, Styled as _, Window,
};
use gpui_component::{
    input::{Input, InputState},
    v_flex, ActiveTheme as _, WindowExt as _,
};

use crate::app::AppState;

/// An image on the clipboard.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PastedImage {
    /// Image files copied in a file manager, which go in as dropped ones do.
    Files(Vec<PathBuf>),
    /// The picture itself, which needs a name.
    Picture(Picture),
}

/// A picture from the clipboard, and the extension of its format.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Picture {
    bytes: Arc<[u8]>,
    extension: &'static str,
}

/// The image `item` holds, if it is to be pasted as one. Text comes before a picture, as the
/// platforms' clipboards rank them, so what an office program copies as both still pastes as
/// text. Files that are not all images are left to paste as their paths.
pub(crate) fn pasted_image(item: &ClipboardItem) -> Option<PastedImage> {
    let entries = item.entries();
    for entry in entries {
        if let ClipboardEntry::ExternalPaths(paths) = entry {
            let paths = paths.paths();
            if !paths.is_empty() && paths.iter().all(|path| is_image_path(path)) {
                return Some(PastedImage::Files(paths.to_vec()));
            }
        }
    }
    let text = entries
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::String(text) if !text.text().is_empty()));
    if text {
        return None;
    }
    entries.iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => {
            let extension = image.format.extension();
            is_image_path(std::path::Path::new(&format!("image.{extension}"))).then(|| {
                PastedImage::Picture(Picture {
                    bytes: image.bytes.as_slice().into(),
                    extension,
                })
            })
        }
        _ => None,
    })
}

/// Lets a file dragged in from another program be dropped straight after typing. GPUI turns such
/// a drag into mouse events, but only the platform's own mouse events end its keyboard mode, in
/// which no element counts as hovered and `on_drop` never fires, so the drop would be lost. The
/// drag's first move therefore goes round again as a mouse event of the platform's.
pub(crate) fn drags_from_outside_use_the_mouse() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, _| {
            window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && window.last_input_was_keyboard()
                    && cx.has_active_drag()
                {
                    let event = event.clone();
                    window.defer(cx, move |window, cx| {
                        window.dispatch_event(PlatformInput::MouseMove(event), cx);
                    });
                }
            });
        },
    )
    .absolute()
    .size_0()
}

/// The images among files dropped on an editor, saying what was left out.
pub(crate) fn dropped_images(
    paths: &ExternalPaths,
    window: &mut Window,
    cx: &mut App,
) -> Vec<ImageSource> {
    let (images, others): (Vec<&PathBuf>, Vec<&PathBuf>) =
        paths.paths().iter().partition(|path| is_image_path(path));
    if let Some(first) = others.first() {
        let name = first.file_name().map_or_else(
            || first.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let message = match others.len() {
            1 => format!("{name} is not an image, so it was not added."),
            more => format!(
                "{name} and {} more are not images, so they were not added.",
                more - 1
            ),
        };
        window.push_notification(message, cx);
    }
    images
        .into_iter()
        .map(|path| ImageSource::File(path.clone()))
        .collect()
}

/// Asks what to call a picture pasted into `document`, and for its alt text when `ask_alt` (the
/// visual editor, where there is no text to type it into afterwards), then hands the image to
/// `place`. The name offered is the field's placeholder, kept if nothing is typed; a name in
/// use is refused as it is typed.
pub(crate) fn ask_name(
    app: &Entity<AppState>,
    document: DocumentId,
    picture: Picture,
    ask_alt: bool,
    place: impl Fn(ImageSource, String, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let Picture { bytes, extension } = picture;
    let offered = app
        .read(cx)
        .engine
        .suggested_image_name(document, extension)
        .unwrap_or_default();
    let name = cx.new(|cx| InputState::new(window, cx).placeholder(offered.clone()));
    let alt = cx.new(|cx| InputState::new(window, cx));
    let first = name.clone();
    let app = app.clone();
    let place = Arc::new(place);
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let offered = offered.clone();
        let chosen = move |name: &Entity<InputState>, cx: &App| {
            let typed = name.read(cx).value().trim().to_owned();
            if typed.is_empty() {
                offered.clone()
            } else {
                typed
            }
        };
        let path = app
            .read(cx)
            .engine
            .pasted_image_path(document, &chosen(&name, cx), extension);
        let status = match &path {
            Ok(path) => div()
                .debug_selector(|| "paste-image-path".to_owned())
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Saves {}", path.display())),
            Err(problem) => div()
                .debug_selector(|| "paste-image-problem".to_owned())
                .text_xs()
                .text_color(cx.theme().danger)
                .child(problem.to_string()),
        };
        let (name, alt_input, app, place, bytes) = (
            name.clone(),
            alt.clone(),
            app.clone(),
            place.clone(),
            bytes.clone(),
        );
        dialog
            .title("Paste Image")
            .width(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .child("File name")
                    .child(Input::new(&name))
                    .child(status)
                    .when(ask_alt, |fields| {
                        fields.child("Alt text").child(Input::new(&alt_input))
                    }),
            )
            .confirm()
            .ok_text("Add")
            .on_ok(move |_, window, cx| {
                let chosen = chosen(&name, cx);
                let free = app
                    .read(cx)
                    .engine
                    .pasted_image_path(document, &chosen, extension)
                    .is_ok();
                if !free {
                    return false;
                }
                let image = ImageSource::Pasted {
                    name: chosen,
                    extension: extension.to_owned(),
                    bytes: bytes.clone(),
                };
                let alt = alt_input.read(cx).value().to_string();
                place(image, alt, window, cx);
                true
            })
    });
    first.update(cx, |input, cx| input.focus(window, cx));
}

#[cfg(test)]
mod tests {
    use gpui::{Image, ImageFormat};

    use super::*;

    fn picture(format: ImageFormat) -> ClipboardEntry {
        ClipboardEntry::Image(Image::from_bytes(format, b"picture".to_vec()))
    }

    fn text(text: &str) -> ClipboardEntry {
        ClipboardEntry::String(gpui::ClipboardString::new(text.to_owned()))
    }

    fn files(names: &[&str]) -> ClipboardEntry {
        ClipboardEntry::ExternalPaths(ExternalPaths(names.iter().map(PathBuf::from).collect()))
    }

    fn item(entries: Vec<ClipboardEntry>) -> ClipboardItem {
        ClipboardItem { entries }
    }

    #[test]
    fn a_picture_alone_is_an_image() {
        assert_eq!(
            pasted_image(&item(vec![picture(ImageFormat::Jpeg)])),
            Some(PastedImage::Picture(Picture {
                bytes: Arc::from(&b"picture"[..]),
                extension: "jpg",
            }))
        );
    }

    #[test]
    fn text_comes_before_a_picture() {
        let both = item(vec![picture(ImageFormat::Png), text("words")]);
        assert_eq!(pasted_image(&both), None);
    }

    #[test]
    fn an_empty_string_does_not_hide_a_picture() {
        let both = item(vec![text(""), picture(ImageFormat::Png)]);
        assert!(matches!(pasted_image(&both), Some(PastedImage::Picture(_))));
    }

    #[test]
    fn a_format_no_page_can_show_is_not_an_image() {
        assert_eq!(pasted_image(&item(vec![picture(ImageFormat::Pnm)])), None);
    }

    #[test]
    fn copied_image_files_are_files_whatever_text_comes_with_them() {
        let copied = item(vec![files(&["/a/one.png", "/a/two.JPG"]), text("one.png")]);
        assert_eq!(
            pasted_image(&copied),
            Some(PastedImage::Files(vec![
                PathBuf::from("/a/one.png"),
                PathBuf::from("/a/two.JPG"),
            ]))
        );
    }

    #[test]
    fn copied_files_that_are_not_all_images_paste_as_their_paths() {
        let copied = item(vec![files(&["/a/one.png", "/a/notes.txt"])]);
        assert_eq!(pasted_image(&copied), None);
    }

    #[test]
    fn text_is_not_an_image() {
        assert_eq!(
            pasted_image(&ClipboardItem::new_string("words".into())),
            None
        );
    }
}
