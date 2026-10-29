//! Images open in tabs of their own. An image is never text: it has no buffer, is never parsed,
//! indexed or diagnosed, and nothing can save over it. The engine keeps only what identifies it
//! (its path) and a revision that changes whenever the file may have, from the watcher or an
//! explicit reload; the front end reads and decodes the file, and reads it again on a new
//! revision. Images share the tab order and the preview tab with pages ([`crate::TabItem`]).

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use adoc_index::normalize_path;

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent, OpenDisposition, TabItem,
};

/// The extensions, in lower case, of the files opened as images. HEIC, HEIF and AVIF are listed
/// so they open in an image tab that says they cannot be shown and offers Open Externally,
/// rather than failing as text.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "svg", "avif", "heic", "heif",
];

/// Whether the file at `path` opens as an image, by its extension in any case. The viewer checks
/// the content: a text file named `picture.png` opens as an image that fails to decode.
#[must_use]
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// Identifies an open image for the lifetime of the engine.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageId(pub(crate) u64);

impl ImageId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An image the engine has open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenImage {
    pub id: ImageId,
    pub path: PathBuf,
    /// Changes whenever the file may have changed; the front end reads the file again.
    pub revision: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Images {
    open: BTreeMap<ImageId, OpenImage>,
    next: u64,
    /// The active image, when the active tab is one; then no document is active.
    pub(crate) active: Option<ImageId>,
}

impl Engine {
    #[must_use]
    pub fn image(&self, id: ImageId) -> Option<&OpenImage> {
        self.images.open.get(&id)
    }

    pub fn images(&self) -> impl Iterator<Item = &OpenImage> {
        self.images.open.values()
    }

    /// The open image for `path`, if any.
    #[must_use]
    pub fn image_for_path(&self, path: &Path) -> Option<ImageId> {
        let path = normalize_path(path);
        self.images
            .open
            .values()
            .find(|image| image.path == path)
            .map(|image| image.id)
    }

    /// The active image: `Some` only while the active tab is an image, when
    /// [`Engine::active`] is `None`.
    #[must_use]
    pub fn active_image(&self) -> Option<ImageId> {
        self.images.active
    }

    /// Makes image `id` the active tab. No document is active while it is.
    pub fn set_active_image(&mut self, id: ImageId) {
        if self.images.open.contains_key(&id) && self.images.active != Some(id) {
            self.images.active = Some(id);
            if let Some(left) = self.active.take() {
                self.auto_save_left(left);
            }
            self.emit(EngineEvent::NavigationChanged);
            self.sync_preview_target();
        }
    }

    /// Opens the image at `path` as `disposition` says and activates it, as
    /// [`Engine::open_as`] opens a page. An image already open is activated as it is. A path
    /// that is not a file is refused before anything changes; the file is not read here.
    pub fn open_image_as(
        &mut self,
        path: &Path,
        disposition: OpenDisposition,
    ) -> Result<ImageId, EngineError> {
        let path = normalize_path(path);
        if let Some(id) = self.image_for_path(&path) {
            self.set_active_image(id);
            if disposition == OpenDisposition::Persistent {
                self.keep_open(id);
            }
            return Ok(id);
        }
        let metadata = fs::metadata(&path).map_err(|error| EngineError::io(&path, error))?;
        if !metadata.is_file() {
            return Err(EngineError::Refused(format!(
                "{} is not a file",
                path.display()
            )));
        }
        let browsing = disposition == OpenDisposition::Preview && self.preview_tabs_on();
        let old = if browsing {
            self.tabs.preview.take()
        } else {
            None
        };
        let id = ImageId(self.images.next);
        self.images.next += 1;
        self.images.open.insert(
            id,
            OpenImage {
                id,
                path,
                revision: 0,
            },
        );
        self.tabs.order.push(TabItem::Image(id));
        self.emit(EngineEvent::ImageOpened(id));
        if browsing {
            self.take_preview_tab(old, TabItem::Image(id))?;
        } else {
            self.set_active_image(id);
        }
        Ok(id)
    }

    /// Closes image `id`.
    pub fn close_image(&mut self, id: ImageId) -> Result<(), EngineError> {
        self.images
            .open
            .remove(&id)
            .ok_or_else(|| EngineError::NotFound(format!("image {} is not open", id.get())))?;
        let item = TabItem::Image(id);
        self.tabs.order.retain(|open| *open != item);
        if self.tabs.preview == Some(item) {
            self.tabs.preview = None;
        }
        if self.images.active == Some(id) {
            self.images.active = None;
            self.activate_last_tab();
            self.sync_preview_target();
        }
        self.emit(EngineEvent::ImageClosed(id));
        Ok(())
    }

    /// Gives image `id` a new revision, so the front end reads the file again.
    pub fn reload_image(&mut self, id: ImageId) -> Result<(), EngineError> {
        let image = self
            .images
            .open
            .get_mut(&id)
            .ok_or_else(|| EngineError::NotFound(format!("image {} is not open", id.get())))?;
        image.revision += 1;
        self.emit(EngineEvent::ImageChanged(id));
        Ok(())
    }

    /// Gives a new revision to the open images among the `changed` paths the watcher reported.
    pub(crate) fn images_changed(&mut self, changed: &[PathBuf]) {
        let stale: Vec<ImageId> = self
            .images
            .open
            .values()
            .filter(|image| changed.contains(&image.path))
            .map(|image| image.id)
            .collect();
        for id in stale {
            let _ = self.reload_image(id);
        }
    }
}

pub(crate) fn register_images(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::IMAGE_RELOAD,
            "Reload Image",
            CommandCategory::View,
            None,
            CommandContext::ActiveImage,
        ),
        |engine, args| {
            let id = match args {
                CommandArgs::Image(id) => id,
                _ => engine
                    .active_image()
                    .ok_or_else(|| EngineError::NotFound("no image to reload".to_owned()))?,
            };
            engine.reload_image(id).map(|()| CommandOutcome::Done)
        },
    );
}
