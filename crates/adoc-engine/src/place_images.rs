//! Pasting or dropping an image into a page (#73). `edit.place_images` copies each image where
//! the page can name it, unless the page can name it where it is, and writes the macros as one
//! undo step that takes back the files it made too. The edit is worked out before anything is
//! written, so whatever is refused leaves nothing behind. New images go in the `images` family
//! of the page's module; outside Antora in the page's `imagesdir`, else in `images` beside it.
//!
//! The copy happens on the caller's thread, as a new page's file does, so its size is limited
//! ([`MAX_PLACED_IMAGE`]): what undo must be able to write again is kept in memory too.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use adoc_antora::{AntoraResolver, ResourceFamily};
use adoc_index::normalize_path;
use adoc_model::{NewBlock, Step};

use crate::{
    commands::{ids, register, spec},
    engine::UndoMode,
    is_image_path,
    new_page::{create_file, CreatedFile},
    search::{antora_id, slash_path},
    visual::refused,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, TextEdit, UndoGroup, VisualArgs, VisualValue,
};

/// The largest image placed in a page, in bytes.
pub const MAX_PLACED_IMAGE: u64 = 64 * 1024 * 1024;

/// An image to place in a page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageSource {
    /// A file dropped on the editor or copied from a file manager.
    File(PathBuf),
    /// An image from the clipboard, with the name typed for it and the extension of its format.
    Pasted {
        name: String,
        extension: String,
        bytes: Arc<[u8]>,
    },
}

/// Where a page's new images go.
struct ImageFolder {
    folder: PathBuf,
    /// What a macro in the page writes before a file name in `folder`.
    prefix: &'static str,
    /// The folder relative targets start from, outside Antora.
    base: Option<PathBuf>,
}

/// What placing some images comes to: the macros' targets, and the files to write.
#[derive(Default)]
struct Plan {
    targets: Vec<String>,
    writes: Vec<(PathBuf, Arc<[u8]>)>,
}

impl Plan {
    /// The file name under which `bytes` will be in `folder`: where it is already, or else the
    /// first one free, numbered from 2.
    fn place(&mut self, folder: &Path, stem: &str, extension: &str, bytes: Arc<[u8]>) -> String {
        for number in 1.. {
            let name = if number == 1 {
                format!("{stem}.{extension}")
            } else {
                format!("{stem}-{number}.{extension}")
            };
            let path = folder.join(&name);
            if let Some((_, planned)) = self.writes.iter().find(|(file, _)| *file == path) {
                if **planned == *bytes {
                    return name;
                }
                continue;
            }
            match fs::symlink_metadata(&path) {
                Err(_) => {
                    self.writes.push((path, bytes));
                    return name;
                }
                Ok(metadata) => {
                    let same = metadata.is_file()
                        && metadata.len() == bytes.len() as u64
                        && fs::read(&path).is_ok_and(|on_disk| *on_disk == *bytes);
                    if same {
                        return name;
                    }
                }
            }
        }
        unreachable!("a number is free before they run out")
    }

    /// The block to write: `alt` belongs to a single image, and without it the caret is left
    /// where the alt text goes.
    fn block(&self, alt: &str) -> NewBlock {
        match self.targets.as_slice() {
            [target] if !alt.is_empty() => NewBlock::Image {
                target: target.clone(),
                alt: alt.to_owned(),
            },
            targets => NewBlock::Images(targets.to_vec()),
        }
    }
}

pub(crate) fn register_place_images(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::EDIT_PLACE_IMAGES,
            "Place Images",
            CommandCategory::Edit,
            None,
            CommandContext::ActiveDocument,
        ),
        place_images,
    );
}

fn place_images(engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
    match args {
        CommandArgs::Images { id, range, images } => {
            let plan = engine.plan_images(id, &images)?;
            let text = engine
                .document(id)
                .ok_or(EngineError::UnknownDocument(id))?
                .text_string();
            let done = adoc_model::insert_block_in_source(&text, range, &plan.block(""))
                .map_err(refused)?;
            let edits = done
                .edits
                .into_iter()
                .map(|edit| TextEdit::new(edit.range, edit.text))
                .collect();
            let selection = done.selection;
            engine.with_files_of(plan, |engine, group| {
                engine.apply_edits(id, edits, UndoMode::Group(group))?;
                Ok(CommandOutcome::Selection(selection))
            })
        }
        CommandArgs::Visual(args) => place_visually(engine, args),
        _ => Err(EngineError::InvalidArguments {
            command: ids::EDIT_PLACE_IMAGES,
            expected: "images and where to place them",
        }),
    }
}

fn place_visually(engine: &mut Engine, args: VisualArgs) -> Result<CommandOutcome, EngineError> {
    let VisualValue::Images { images, alt } = &args.value else {
        return Err(EngineError::InvalidArguments {
            command: ids::EDIT_PLACE_IMAGES,
            expected: "images and where to place them",
        });
    };
    let plan = engine.plan_images(args.id, images)?;
    let visual = engine.visual_for(&args)?;
    let step = Step::InsertBlock {
        at: args.selection.end,
        block: plan.block(alt),
    };
    let result = visual.model.apply(&step).map_err(refused)?;
    let source = visual.model.source.clone();
    engine.with_files_of(plan, |engine, group| {
        engine.apply_visual(args.id, &source, result, UndoMode::Group(group))
    })
}

impl Engine {
    /// A name free for an image of format `extension` pasted into document `id`, made from the
    /// page's own, for the front end to offer. `None` when the page can take no image.
    #[must_use]
    pub fn suggested_image_name(&self, id: DocumentId, extension: &str) -> Option<String> {
        let page = normalize_path(&self.document(id)?.path);
        let home = self.image_folder(&page).ok()?;
        let stem = page
            .file_stem()
            .map(|stem| file_stem(&stem.to_string_lossy()))
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "image".to_owned());
        (1..).map(|number| format!("{stem}-{number}")).find(|name| {
            fs::symlink_metadata(home.folder.join(format!("{name}.{extension}"))).is_err()
        })
    }

    /// Where an image of format `extension` pasted into document `id` as `name` would go. The
    /// name is made fit for a macro and keeps to the folder. Refuses a name with nothing left,
    /// and one in use: it was chosen, so it is not numbered as a dropped file's is.
    pub fn pasted_image_path(
        &self,
        id: DocumentId,
        name: &str,
        extension: &str,
    ) -> Result<PathBuf, EngineError> {
        let document = self.document(id).ok_or(EngineError::UnknownDocument(id))?;
        let home = self.image_folder(&normalize_path(&document.path))?;
        let path = home.folder.join(pasted_file_name(name, extension)?);
        if fs::symlink_metadata(&path).is_ok() {
            return Err(EngineError::Refused(format!(
                "{} already exists",
                path.display()
            )));
        }
        Ok(path)
    }

    /// Where the new images of `page` go.
    fn image_folder(&self, page: &Path) -> Result<ImageFolder, EngineError> {
        let antora = &self.workspace.antora;
        if let Some(context) = antora.context_for_path(page) {
            let module = antora
                .modules_of(&context.component, context.version.as_deref())
                .find(|module| module.name == context.module)
                .ok_or_else(|| {
                    EngineError::NotFound(format!("the module of {}", page.display()))
                })?;
            return Ok(ImageFolder {
                folder: self.listable(&module.root.join(ResourceFamily::Image.directory()))?,
                prefix: "",
                base: None,
            });
        }
        let document = &self
            .workspace
            .index
            .file(page)
            .ok_or_else(|| EngineError::NotFound(format!("{} in the index", page.display())))?
            .document;
        let base = adoc_index::resolve_image_target(document, page, "image.png")
            .and_then(|probe| probe.parent().map(Path::to_path_buf))
            .ok_or_else(|| {
                EngineError::Refused(format!(
                    "the imagesdir of {} is not a folder, so a new image has nowhere to go",
                    page.display()
                ))
            })?;
        let declared = document
            .attributes
            .iter()
            .any(|attribute| attribute.name == "imagesdir");
        let (folder, prefix) = if declared {
            (base.clone(), "")
        } else {
            (base.join(ResourceFamily::Image.directory()), "images/")
        };
        Ok(ImageFolder {
            folder: self.listable(&folder)?,
            prefix,
            base: Some(base),
        })
    }

    /// What placing `images` in document `id` comes to; nothing is written.
    fn plan_images(&self, id: DocumentId, images: &[ImageSource]) -> Result<Plan, EngineError> {
        let document = self.document(id).ok_or(EngineError::UnknownDocument(id))?;
        if !document.asciidoc {
            return Err(EngineError::Refused(format!(
                "{} is not an AsciiDoc page",
                document.path.display()
            )));
        }
        let page = normalize_path(&document.path);
        let home = self.image_folder(&page)?;
        let mut plan = Plan::default();
        for image in images {
            match image {
                ImageSource::Pasted {
                    name,
                    extension,
                    bytes,
                } => {
                    check_size(bytes.len() as u64, name)?;
                    let path = self.pasted_image_path(id, name, extension)?;
                    let name = pasted_file_name(name, extension)?;
                    plan.writes.push((path, bytes.clone()));
                    plan.targets.push(format!("{}{name}", home.prefix));
                }
                ImageSource::File(path) => {
                    let path = normalize_path(path);
                    if !is_image_path(&path) {
                        return Err(EngineError::Refused(format!(
                            "{} is not an image",
                            path.display()
                        )));
                    }
                    if let Some(target) = self.target_of_existing(&page, &home, &path) {
                        plan.targets.push(writable_target(target, &path)?);
                        continue;
                    }
                    let bytes = read_image(&path)?;
                    let stem = path
                        .file_stem()
                        .map_or_else(String::new, |stem| file_stem(&stem.to_string_lossy()));
                    let stem = if stem.is_empty() { "image" } else { &stem };
                    let extension = path.extension().map_or_else(String::new, |extension| {
                        extension.to_string_lossy().into_owned()
                    });
                    let name = plan.place(&home.folder, stem, &extension, bytes);
                    plan.targets.push(format!("{}{name}", home.prefix));
                }
            }
        }
        Ok(plan)
    }

    /// How `page` can name the image at `path` where it is, if it can: its resource id for an
    /// image the catalog holds, a path under the folder targets start from outside Antora.
    fn target_of_existing(&self, page: &Path, home: &ImageFolder, path: &Path) -> Option<String> {
        if !path.is_file() {
            return None;
        }
        if let Some(base) = &home.base {
            return path.strip_prefix(base).ok().map(slash_path);
        }
        let antora = &self.workspace.antora;
        let context = antora.context_for_path(page)?;
        let resource = antora.resource_for_path(path)?;
        // A file of another family does not resolve as an image, so it is copied.
        let target = antora_id(&context, &resource.coordinate, "");
        let id = adoc_lang::images::antora_image_id(&target).ok()??;
        let found = AntoraResolver::resolve(antora, &id, &context).ok()?;
        (normalize_path(&found.source_path) == path).then_some(target)
    }

    /// Writes what `plan` writes and runs `apply`, as one undo group that takes the files back
    /// with it. If either fails, no file is left.
    fn with_files_of(
        &mut self,
        plan: Plan,
        apply: impl FnOnce(&mut Self, UndoGroup) -> Result<CommandOutcome, EngineError>,
    ) -> Result<CommandOutcome, EngineError> {
        let paths: Vec<PathBuf> = plan.writes.iter().map(|(path, _)| path.clone()).collect();
        let mut written = 0;
        let mut failed = None;
        for (path, bytes) in &plan.writes {
            match create_file(path, bytes) {
                Ok(()) => written += 1,
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        let group = self.new_group();
        let outcome = match failed {
            Some(error) => Err(error),
            None => {
                self.resources_changed(&paths);
                apply(self, group)
            }
        };
        match outcome {
            Ok(outcome) => {
                let created = plan
                    .writes
                    .into_iter()
                    .map(|(path, bytes)| CreatedFile::binary(path, bytes))
                    .collect();
                self.group_created.insert(group, created);
                Ok(outcome)
            }
            Err(error) => {
                for path in &paths[..written] {
                    let _ = fs::remove_file(path);
                }
                self.resources_changed(&paths[..written]);
                Err(error)
            }
        }
    }
}

/// The file name for an image pasted as `name`, an extension typed too left out.
fn pasted_file_name(name: &str, extension: &str) -> Result<String, EngineError> {
    if !is_image_path(Path::new(&format!("image.{extension}"))) {
        return Err(EngineError::Refused(format!(
            "`.{extension}` is not an image format"
        )));
    }
    let name = name.trim();
    let typed = name
        .rsplit_once('.')
        .filter(|(_, typed)| typed.eq_ignore_ascii_case(extension))
        .map_or(name, |(stem, _)| stem);
    let stem = file_stem(typed);
    if stem.is_empty() {
        return Err(EngineError::Refused(
            "the image needs a name with a letter or digit in it".to_owned(),
        ));
    }
    Ok(format!("{stem}.{extension}"))
}

/// `name` as the start of a file name a macro can write as it is: letters, digits, `_` and `.`,
/// with one hyphen for each run of anything else, none at either end. Nothing in it can leave
/// a folder, or be read as an attribute, a resource id or the end of the target.
fn file_stem(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '.') {
            stem.push(character);
        } else if !stem.ends_with('-') {
            stem.push('-');
        }
    }
    let stem = stem.trim_matches(|character| matches!(character, '-' | '.'));
    // `..` and the like are not a name, whatever else surrounds them.
    let mut clean = String::with_capacity(stem.len());
    for character in stem.chars() {
        if character != '.' || !clean.ends_with('.') {
            clean.push(character);
        }
    }
    clean
}

/// `target` when a macro can hold it: it names a file already there, so it was not chosen here.
fn writable_target(target: String, path: &Path) -> Result<String, EngineError> {
    if target.contains(|character: char| {
        character.is_whitespace() || matches!(character, '[' | ']' | '{' | '}')
    }) {
        return Err(EngineError::Refused(format!(
            "an image macro cannot name {}; rename the file first",
            path.display()
        )));
    }
    Ok(target)
}

fn check_size(size: u64, what: impl std::fmt::Display) -> Result<(), EngineError> {
    if size > MAX_PLACED_IMAGE {
        return Err(EngineError::Refused(format!(
            "{what} is larger than the {} MiB an image placed in a page may be",
            MAX_PLACED_IMAGE / (1024 * 1024)
        )));
    }
    Ok(())
}

/// The contents of the image file at `path`, which must be a file itself, of no more than
/// [`MAX_PLACED_IMAGE`] bytes. A pipe or device would never finish reading, and a link could
/// copy whatever it points at, under an image's name, into the workspace.
fn read_image(path: &Path) -> Result<Arc<[u8]>, EngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| EngineError::io(path, error))?;
    if !metadata.is_file() {
        return Err(EngineError::Refused(format!(
            "{} is not a regular file, so it was not copied",
            path.display()
        )));
    }
    check_size(metadata.len(), path.display())?;
    let bytes = fs::read(path).map_err(|error| EngineError::io(path, error))?;
    Ok(bytes.into())
}
