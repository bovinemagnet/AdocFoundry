//! Pasting or dropping an image into a page (#73): `edit.place_images` copies it where the page
//! can name it and writes the macro, as one undo step.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, Engine, EngineError, EngineEvent,
    ImageSource, MarkSet, Settings, VisualArgs, VisualValue, MAX_PLACED_IMAGE,
};
use adoc_render::{ViewBlock, ViewKind};

const WAIT: Duration = Duration::from_secs(10);
const SCREENSHOT: &[u8] = b"\x89PNG screenshot";

struct Workspace {
    _directory: tempfile::TempDir,
    root: PathBuf,
    engine: Engine,
    page: DocumentId,
}

impl Workspace {
    /// A component with modules `ROOT` (holding the page) and `other`, and no `images` folder
    /// in `ROOT`.
    fn antora(text: &str) -> Self {
        Self::antora_with(text, &[])
    }

    /// The same, with `files` as well, in place before the workspace opens.
    fn antora_with(text: &str, files: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        write(
            &root.join("antora.yml"),
            b"name: guide\nversion: ~\ntitle: Guide\n",
        );
        write(&root.join("modules/ROOT/pages/page.adoc"), text.as_bytes());
        write(&root.join("modules/other/images/shared.png"), b"shared");
        for file in files {
            write(&root.join(file), SCREENSHOT);
        }
        let page = root.join("modules/ROOT/pages/page.adoc");
        Self::open(directory, root, page)
    }

    /// A page in a folder of its own, outside Antora.
    fn plain(text: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_index::normalize_path(directory.path());
        write(&root.join("docs/page.adoc"), text.as_bytes());
        Self::open(directory, root.clone(), root.join("docs/page.adoc"))
    }

    fn open(directory: tempfile::TempDir, root: PathBuf, page: PathBuf) -> Self {
        let mut engine = Engine::new(Settings::default().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(WAIT));
        engine.pump();
        let page = engine.open(&page).unwrap();
        Self {
            _directory: directory,
            root,
            engine,
            page,
        }
    }

    fn module_images(&self) -> PathBuf {
        self.root.join("modules/ROOT/images")
    }

    fn text(&self) -> String {
        self.engine.document(self.page).unwrap().text_string()
    }

    /// Places `images` in the source at `at`.
    fn place(&mut self, at: usize, images: Vec<ImageSource>) -> Result<SourceRange, EngineError> {
        let outcome = self.engine.invoke(
            ids::EDIT_PLACE_IMAGES,
            CommandArgs::Images {
                id: self.page,
                range: SourceRange::new(at, at),
                images,
            },
        )?;
        match outcome {
            CommandOutcome::Selection(selection) => Ok(selection),
            other => panic!("expected a selection, got {other:?}"),
        }
    }

    /// Places `images` in the visual editor at `at`.
    fn place_visually(
        &mut self,
        at: usize,
        value: VisualValue,
    ) -> Result<SourceRange, EngineError> {
        let version = self.engine.document(self.page).unwrap().version;
        let outcome = self.engine.invoke(
            ids::EDIT_PLACE_IMAGES,
            CommandArgs::Visual(VisualArgs {
                id: self.page,
                version,
                selection: SourceRange::new(at, at),
                marks: MarkSet::default(),
                value,
            }),
        )?;
        match outcome {
            CommandOutcome::Selection(selection) => Ok(selection),
            other => panic!("expected a selection, got {other:?}"),
        }
    }

    fn show_visual(&mut self) {
        self.engine
            .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(self.page))
            .unwrap();
    }

    /// A file outside the workspace.
    fn outside(&self, name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = adoc_index::normalize_path(directory.path()).join(name);
        write(&path, bytes);
        (directory, path)
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn pasted(name: &str) -> ImageSource {
    ImageSource::Pasted {
        name: name.to_owned(),
        extension: "png".to_owned(),
        bytes: Arc::from(SCREENSHOT),
    }
}

/// The files under `folder`, relative to it.
fn files_under(folder: &Path) -> Vec<String> {
    fn walk(folder: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, folder, &mut out);
    out.sort();
    out
}

fn preview_images(engine: &mut Engine, id: DocumentId) -> Vec<(String, Option<PathBuf>)> {
    fn walk(blocks: Vec<&ViewBlock>, out: &mut Vec<(String, Option<PathBuf>)>) {
        for block in blocks {
            if let ViewKind::Image(image) = &block.kind {
                out.push((image.target.clone(), image.path.clone()));
            }
            walk(block.child_refs(), out);
        }
    }
    engine.request_preview(id);
    engine.pump();
    assert!(engine.wait_for_workers(WAIT));
    engine.pump();
    let preview = engine.rendered_preview(id).expect("a preview");
    let mut out = Vec::new();
    walk(preview.view.blocks.iter().collect(), &mut out);
    out
}

// --- the issue's finish line --------------------------------------------------------------------

#[test]
fn a_screenshot_pasted_into_a_page_shows_in_the_preview_at_once() {
    let mut workspace = Workspace::antora("= Page\n\nWords.\n");
    let end = workspace.text().len();
    let selection = workspace.place(end, vec![pasted("flow")]).unwrap();

    let file = workspace.module_images().join("flow.png");
    assert_eq!(fs::read(&file).unwrap(), SCREENSHOT);
    assert_eq!(workspace.text(), "= Page\n\nWords.\n\nimage::flow.png[]\n");
    // Empty alt text, ready to be typed.
    let brackets = workspace.text().find("[]").unwrap() + 1;
    assert_eq!(selection, SourceRange::new(brackets, brackets));

    // No wait for the watcher: the catalog knows the file the moment the macro names it.
    let page = workspace.page;
    let images = preview_images(&mut workspace.engine, page);
    assert_eq!(images, vec![("flow.png".to_owned(), Some(file))]);
}

#[test]
fn a_placed_image_is_not_reported_as_missing() {
    let mut workspace = Workspace::antora("= Page\n\nWords.\n");
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    workspace.engine.pump();
    assert!(workspace.engine.wait_for_workers(WAIT));
    workspace.engine.pump();
    let problems = workspace.engine.workspace_diagnostics();
    assert!(
        problems
            .iter()
            .all(|(_, diagnostics)| diagnostics.is_empty()),
        "{problems:?}"
    );
}

#[test]
fn the_catalog_change_is_announced() {
    let mut workspace = Workspace::antora("= Page\n");
    let events = workspace.engine.subscribe();
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    assert!(events
        .try_iter()
        .any(|event| matches!(event, EngineEvent::CatalogChanged { .. })));
}

// --- dropping a file ----------------------------------------------------------------------------

#[test]
fn a_dropped_file_is_copied_into_the_module_with_a_name_a_macro_can_write() {
    let mut workspace = Workspace::antora("= Page\n");
    let (_keep, source) = workspace.outside("Screen Shot [1] {2}.PNG", SCREENSHOT);
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(source.clone())])
        .unwrap();
    assert_eq!(
        files_under(&workspace.module_images()),
        ["Screen-Shot-1-2.PNG"]
    );
    assert_eq!(workspace.text(), "= Page\n\nimage::Screen-Shot-1-2.PNG[]\n");
    assert_eq!(fs::read(source).unwrap(), SCREENSHOT, "the original stays");
}

#[test]
fn a_file_already_in_the_module_is_named_without_copying() {
    let mut workspace = Workspace::antora_with("= Page\n", &["modules/ROOT/images/shots/one.png"]);
    let image = workspace.module_images().join("shots/one.png");
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(image)])
        .unwrap();
    assert_eq!(files_under(&workspace.module_images()), ["shots/one.png"]);
    assert_eq!(workspace.text(), "= Page\n\nimage::shots/one.png[]\n");
}

#[test]
fn an_image_of_another_module_is_named_by_its_resource_id() {
    let mut workspace = Workspace::antora("= Page\n");
    let image = workspace.root.join("modules/other/images/shared.png");
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(image.clone())])
        .unwrap();
    assert_eq!(workspace.text(), "= Page\n\nimage::other:shared.png[]\n");
    assert!(!workspace.module_images().exists(), "nothing is copied");
    let page = workspace.page;
    assert_eq!(
        preview_images(&mut workspace.engine, page),
        vec![("other:shared.png".to_owned(), Some(image))]
    );
}

#[test]
fn a_name_taken_by_another_image_gets_a_number_and_the_same_image_is_reused() {
    let mut workspace = Workspace::antora("= Page\n");
    write(
        &workspace.module_images().join("flow.png"),
        b"another picture",
    );
    let (_keep, different) = workspace.outside("flow.png", SCREENSHOT);
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(different.clone())])
        .unwrap();
    assert!(workspace.text().contains("image::flow-2.png[]"));
    assert_eq!(
        fs::read(workspace.module_images().join("flow.png")).unwrap(),
        b"another picture",
        "never overwritten"
    );

    // The same picture again: the copy made already is named, and there is no third file.
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(different)])
        .unwrap();
    assert_eq!(workspace.text().matches("image::flow-2.png[]").count(), 2);
    assert_eq!(
        files_under(&workspace.module_images()),
        ["flow-2.png", "flow.png"]
    );
}

#[test]
fn several_dropped_files_are_one_edit_with_the_caret_in_the_last() {
    let mut workspace = Workspace::antora("= Page\n");
    let (_first, a) = workspace.outside("same.png", b"first");
    let (_second, b) = workspace.outside("same.png", b"second");
    let (_third, c) = workspace.outside("same.png", b"first");
    let end = workspace.text().len();
    let selection = workspace
        .place(
            end,
            vec![
                ImageSource::File(a),
                ImageSource::File(b),
                ImageSource::File(c),
            ],
        )
        .unwrap();
    assert_eq!(
        workspace.text(),
        "= Page\n\nimage::same.png[]\n\nimage::same-2.png[]\n\nimage::same.png[]\n"
    );
    assert_eq!(
        files_under(&workspace.module_images()),
        ["same-2.png", "same.png"]
    );
    let last = workspace.text().rfind("[]").unwrap() + 1;
    assert_eq!(selection, SourceRange::new(last, last));

    assert!(workspace.engine.undo(workspace.page));
    assert_eq!(workspace.text(), "= Page\n");
    assert_eq!(
        files_under(&workspace.module_images()),
        Vec::<String>::new()
    );
}

// --- undo ---------------------------------------------------------------------------------------

#[test]
fn undo_takes_back_the_macro_and_the_file_and_redo_makes_them_again() {
    let mut workspace = Workspace::antora("= Page\n");
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    let file = workspace.module_images().join("flow.png");

    assert!(workspace.engine.undo(workspace.page));
    assert_eq!(workspace.text(), "= Page\n");
    assert!(!file.exists());

    assert!(workspace.engine.redo(workspace.page));
    assert_eq!(workspace.text(), "= Page\n\nimage::flow.png[]\n");
    assert_eq!(fs::read(&file).unwrap(), SCREENSHOT);
    let page = workspace.page;
    assert_eq!(
        preview_images(&mut workspace.engine, page),
        vec![("flow.png".to_owned(), Some(file))]
    );
}

#[test]
fn undo_keeps_a_file_changed_since() {
    let mut workspace = Workspace::antora("= Page\n");
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    let file = workspace.module_images().join("flow.png");
    fs::write(&file, b"edited in another program").unwrap();

    assert!(workspace.engine.undo(workspace.page));
    assert_eq!(workspace.text(), "= Page\n");
    assert_eq!(fs::read(&file).unwrap(), b"edited in another program");
}

#[test]
fn undo_keeps_an_image_it_did_not_copy() {
    let mut workspace = Workspace::antora("= Page\n");
    let image = workspace.root.join("modules/other/images/shared.png");
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(image.clone())])
        .unwrap();
    assert!(workspace.engine.undo(workspace.page));
    assert!(image.is_file());
}

// --- the visual editor --------------------------------------------------------------------------

#[test]
fn the_visual_editor_places_an_image_after_the_caret_block_as_one_undo_step() {
    let mut workspace = Workspace::antora("= Page\n\nFirst.\n\nSecond.\n");
    workspace.show_visual();
    let at = workspace.text().find("First").unwrap();
    let selection = workspace
        .place_visually(
            at,
            VisualValue::Images {
                images: vec![pasted("flow")],
                alt: String::new(),
            },
        )
        .unwrap();
    assert_eq!(
        workspace.text(),
        "= Page\n\nFirst.\n\nimage::flow.png[]\n\nSecond.\n"
    );
    assert!(workspace.module_images().join("flow.png").is_file());
    let brackets = workspace.text().find("[]").unwrap() + 1;
    assert_eq!(selection, SourceRange::new(brackets, brackets));

    assert!(workspace.engine.undo(workspace.page));
    assert_eq!(workspace.text(), "= Page\n\nFirst.\n\nSecond.\n");
    assert!(!workspace.module_images().join("flow.png").exists());
}

#[test]
fn the_visual_editor_writes_alt_text_given_with_the_image() {
    let mut workspace = Workspace::antora("= Page\n\nFirst.\n");
    workspace.show_visual();
    let at = workspace.text().find("First").unwrap();
    workspace
        .place_visually(
            at,
            VisualValue::Images {
                images: vec![pasted("flow")],
                alt: "The flow".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(
        workspace.text(),
        "= Page\n\nFirst.\n\nimage::flow.png[The flow]\n"
    );
}

#[test]
fn a_place_the_visual_editor_refuses_leaves_no_file_behind() {
    let mut workspace = Workspace::antora("= Page\n\n|===\n|cell\n|===\n");
    workspace.show_visual();
    let at = workspace.text().find("cell").unwrap();
    let refused = workspace.place_visually(
        at,
        VisualValue::Images {
            images: vec![pasted("flow")],
            alt: String::new(),
        },
    );
    assert!(refused.is_err());
    assert_eq!(
        files_under(&workspace.module_images()),
        Vec::<String>::new()
    );
    assert_eq!(workspace.text(), "= Page\n\n|===\n|cell\n|===\n");
}

// --- outside Antora -----------------------------------------------------------------------------

#[test]
fn outside_antora_a_new_image_goes_in_an_images_folder_beside_the_page() {
    let mut workspace = Workspace::plain("= Page\n");
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    let file = workspace.root.join("docs/images/flow.png");
    assert!(file.is_file());
    assert_eq!(workspace.text(), "= Page\n\nimage::images/flow.png[]\n");
    let page = workspace.page;
    assert_eq!(
        preview_images(&mut workspace.engine, page),
        vec![("images/flow.png".to_owned(), Some(file))]
    );
}

#[test]
fn outside_antora_a_new_image_goes_in_the_page_s_imagesdir() {
    let mut workspace = Workspace::plain("= Page\n:imagesdir: media/shots\n\nWords.\n");
    let end = workspace.text().len();
    workspace.place(end, vec![pasted("flow")]).unwrap();
    let file = workspace.root.join("docs/media/shots/flow.png");
    assert!(file.is_file());
    assert!(workspace.text().ends_with("\n\nimage::flow.png[]\n"));
    let page = workspace.page;
    assert_eq!(
        preview_images(&mut workspace.engine, page),
        vec![("flow.png".to_owned(), Some(file))]
    );
}

#[test]
fn outside_antora_a_file_the_page_can_reach_is_named_without_copying() {
    let mut workspace = Workspace::plain("= Page\n");
    let image = workspace.root.join("docs/art/one.png");
    write(&image, SCREENSHOT);
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(image)])
        .unwrap();
    assert_eq!(workspace.text(), "= Page\n\nimage::art/one.png[]\n");
    assert_eq!(
        files_under(&workspace.root.join("docs")),
        ["art/one.png", "page.adoc"]
    );
}

// --- what is refused ----------------------------------------------------------------------------

#[test]
fn a_name_cannot_leave_the_folder_or_overwrite_a_file() {
    let mut workspace = Workspace::antora("= Page\n");
    let end = workspace.text().len();

    // Separators are only characters of the name.
    workspace
        .place(end, vec![pasted("../../pages/evil")])
        .unwrap();
    assert_eq!(files_under(&workspace.module_images()), ["pages-evil.png"]);
    assert!(!workspace.root.join("modules/ROOT/pages/evil.png").exists());

    // A name in use is refused rather than numbered: it was typed.
    let error = workspace
        .place(end, vec![pasted("pages-evil")])
        .unwrap_err();
    assert!(error.to_string().contains("already"), "{error}");
    assert_eq!(
        fs::read(workspace.module_images().join("pages-evil.png")).unwrap(),
        SCREENSHOT
    );
    let error = workspace.place(end, vec![pasted("/..//")]).unwrap_err();
    assert!(error.to_string().contains("name"), "{error}");
}

#[test]
fn what_is_not_a_regular_image_file_is_refused_and_changes_nothing() {
    let mut workspace = Workspace::antora("= Page\n");
    let end = workspace.text().len();
    let (_keep, text_file) = workspace.outside("notes.txt", b"words");
    let (_keep_folder, in_folder) = workspace.outside("dir.png/inside", b"");
    let folder = in_folder.parent().unwrap().to_path_buf();
    let (_keep_big, big) = workspace.outside("big.png", b"");
    fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(MAX_PLACED_IMAGE + 1)
        .unwrap();
    let missing = big.with_file_name("gone.png");

    for path in [text_file, folder, big, missing] {
        let refused = workspace.place(end, vec![ImageSource::File(path.clone())]);
        assert!(refused.is_err(), "{}", path.display());
    }
    let pasted_big = ImageSource::Pasted {
        name: "big".to_owned(),
        extension: "png".to_owned(),
        bytes: vec![0; MAX_PLACED_IMAGE as usize + 1].into(),
    };
    assert!(workspace.place(end, vec![pasted_big]).is_err());
    let pasted_text = ImageSource::Pasted {
        name: "notes".to_owned(),
        extension: "txt".to_owned(),
        bytes: Arc::from(&b"words"[..]),
    };
    assert!(workspace.place(end, vec![pasted_text]).is_err());
    assert!(workspace.place(end, Vec::new()).is_err());

    assert_eq!(workspace.text(), "= Page\n");
    assert!(!workspace.module_images().exists());
}

#[test]
fn one_bad_file_of_several_leaves_nothing_behind() {
    let mut workspace = Workspace::antora("= Page\n");
    let (_keep, good) = workspace.outside("good.png", SCREENSHOT);
    let (_keep_bad, bad) = workspace.outside("notes.txt", b"words");
    let end = workspace.text().len();
    let refused = workspace.place(end, vec![ImageSource::File(good), ImageSource::File(bad)]);
    assert!(refused.is_err());
    assert_eq!(workspace.text(), "= Page\n");
    assert!(!workspace.module_images().exists());
}

#[test]
fn an_existing_image_a_macro_cannot_name_is_refused_with_the_reason() {
    let mut workspace = Workspace::plain("= Page\n");
    let image = workspace.root.join("docs/my picture.png");
    write(&image, SCREENSHOT);
    let end = workspace.text().len();
    let error = workspace
        .place(end, vec![ImageSource::File(image)])
        .unwrap_err();
    assert!(error.to_string().contains("my picture.png"), "{error}");
    assert_eq!(workspace.text(), "= Page\n");
}

#[test]
fn a_file_in_the_way_of_the_images_folder_refuses_with_nothing_changed() {
    let mut workspace = Workspace::antora("= Page\n");
    write(&workspace.module_images(), b"not a folder");
    let end = workspace.text().len();
    assert!(workspace.place(end, vec![pasted("flow")]).is_err());
    assert_eq!(workspace.text(), "= Page\n");
    assert_eq!(
        fs::read(workspace.module_images()).unwrap(),
        b"not a folder"
    );
    assert!(!workspace.engine.undo(workspace.page), "nothing to undo");
}

#[test]
fn a_picture_in_another_family_is_copied_rather_than_named() {
    let mut workspace = Workspace::antora_with("= Page\n", &["modules/ROOT/attachments/chart.png"]);
    let attachment = workspace.root.join("modules/ROOT/attachments/chart.png");
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(attachment)])
        .unwrap();
    assert_eq!(workspace.text(), "= Page\n\nimage::chart.png[]\n");
    assert_eq!(files_under(&workspace.module_images()), ["chart.png"]);
}

#[test]
fn a_picture_whose_name_means_another_image_is_not_named_by_it() {
    // `chart.png` from this page is the image, not the attachment dropped.
    let mut workspace = Workspace::antora_with(
        "= Page\n",
        &[
            "modules/ROOT/attachments/chart.png",
            "modules/ROOT/images/chart.png",
        ],
    );
    let image = workspace.module_images().join("chart.png");
    fs::write(&image, b"a different picture").unwrap();
    let attachment = workspace.root.join("modules/ROOT/attachments/chart.png");
    let end = workspace.text().len();
    workspace
        .place(end, vec![ImageSource::File(attachment)])
        .unwrap();
    assert_eq!(workspace.text(), "= Page\n\nimage::chart-2.png[]\n");
    assert_eq!(
        fs::read(workspace.module_images().join("chart-2.png")).unwrap(),
        SCREENSHOT
    );
}

#[cfg(unix)]
#[test]
fn a_pipe_named_like_an_image_is_refused_rather_than_read_for_ever() {
    let mut workspace = Workspace::antora("= Page\n");
    let (_keep, placeholder) = workspace.outside("placeholder", b"");
    let pipe = placeholder.with_file_name("waiting.png");
    let made = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .unwrap();
    assert!(made.success());
    let end = workspace.text().len();
    assert!(workspace.place(end, vec![ImageSource::File(pipe)]).is_err());
    assert_eq!(workspace.text(), "= Page\n");
}

#[test]
fn a_text_file_in_a_module_takes_no_image() {
    // In a module the page's folder is known, so only its kind stands in the way.
    let mut workspace = Workspace::antora("= Page\n");
    let notes = workspace.root.join("modules/ROOT/pages/notes.txt");
    write(&notes, b"words\n");
    workspace.page = workspace.engine.open(&notes).unwrap();
    let error = workspace.place(0, vec![pasted("flow")]).unwrap_err();
    assert!(
        error.to_string().contains("not an AsciiDoc page"),
        "{error}"
    );
    assert!(!workspace.module_images().exists());
}

#[test]
fn a_page_that_is_not_asciidoc_takes_no_image() {
    let mut workspace = Workspace::plain("= Page\n");
    let notes = workspace.root.join("docs/notes.txt");
    write(&notes, b"words\n");
    workspace.page = workspace.engine.open(&notes).unwrap();
    assert!(workspace.place(0, vec![pasted("flow")]).is_err());
    assert!(!workspace.root.join("docs/images").exists());
}

#[test]
fn a_page_outside_the_workspace_takes_no_image() {
    let mut workspace = Workspace::plain("= Page\n");
    let (_keep, elsewhere) = workspace.outside("loose.adoc", b"= Loose\n");
    workspace.page = workspace.engine.open(&elsewhere).unwrap();
    assert!(workspace.place(0, vec![pasted("flow")]).is_err());
    assert!(!elsewhere.with_file_name("images").exists());
}

#[cfg(unix)]
#[test]
fn an_images_folder_that_is_a_link_is_not_written_through() {
    let mut workspace = Workspace::antora("= Page\n");
    let (_keep, elsewhere) = workspace.outside("placeholder", b"");
    std::os::unix::fs::symlink(elsewhere.parent().unwrap(), workspace.module_images()).unwrap();
    let end = workspace.text().len();
    assert!(workspace.place(end, vec![pasted("flow")]).is_err());
    assert_eq!(workspace.text(), "= Page\n");
    assert!(!elsewhere.with_file_name("flow.png").exists());
}

// --- naming a pasted image ----------------------------------------------------------------------

#[test]
fn a_pasted_image_is_offered_a_free_name_from_its_page() {
    let workspace = Workspace::antora("= Page\n");
    let page = workspace.page;
    assert_eq!(
        workspace
            .engine
            .suggested_image_name(page, "png")
            .as_deref(),
        Some("page-1")
    );
    write(&workspace.module_images().join("page-1.png"), b"taken");
    assert_eq!(
        workspace
            .engine
            .suggested_image_name(page, "png")
            .as_deref(),
        Some("page-2")
    );
    // Another format's file does not take the name.
    assert_eq!(
        workspace
            .engine
            .suggested_image_name(page, "jpg")
            .as_deref(),
        Some("page-1")
    );
}

#[test]
fn the_path_of_a_typed_name_is_known_before_anything_is_written() {
    let workspace = Workspace::antora("= Page\n");
    let page = workspace.page;
    let images = workspace.module_images();
    let engine = &workspace.engine;
    assert_eq!(
        engine.pasted_image_path(page, "My Flow", "png").unwrap(),
        images.join("My-Flow.png")
    );
    // The extension may be typed too, in any case.
    assert_eq!(
        engine.pasted_image_path(page, "flow.PNG", "png").unwrap(),
        images.join("flow.png")
    );
    assert_eq!(
        engine.pasted_image_path(page, "v1.2", "png").unwrap(),
        images.join("v1.2.png")
    );
    assert!(engine.pasted_image_path(page, "  ", "png").is_err());
    write(&images.join("taken.png"), b"taken");
    assert!(engine.pasted_image_path(page, "taken", "png").is_err());
    assert!(!images.join("flow.png").exists());
}
