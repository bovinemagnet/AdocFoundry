//! Kindle export through the engine, over the publication plan PDF and Word export use
//! (`docs/prd/PRD-AdocFoundry_Kindle_Export.md`).

#![cfg(feature = "kindle")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{Engine, EngineError, ExportTarget, KindleExportArgs, KindleFormat, Settings};

const WAIT: Duration = Duration::from_secs(120);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The book fixture copied to a temporary directory and indexed.
fn book_workspace() -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("book");
    copy_tree(&fixture("pdf/book"), &root);
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn page(root: &Path, relative: &str) -> PathBuf {
    root.join("guide/modules").join(relative)
}

/// An output whose extension is not the format's is refused before anything is read, and
/// nothing is written in either format's place.
#[test]
fn kindle_outputs_must_carry_their_format_extension() {
    let (directory, root, mut engine) = book_workspace();
    let target = ExportTarget::File(page(&root, "ROOT/pages/install.adoc"));
    for (format, name) in [
        (KindleFormat::Kfx, "book.azw3"),
        (KindleFormat::Azw3, "book.kfx"),
        (KindleFormat::Azw3, "book.epub"),
        (KindleFormat::Kfx, "no-extension"),
    ] {
        let output = directory.path().join(name);
        let result = engine.export_kindle_blocking(
            KindleExportArgs::new(target.clone(), format, output.clone()),
            WAIT,
        );
        assert!(
            matches!(result, Err(EngineError::InvalidRequest(_))),
            "{name}: {result:?}"
        );
        assert!(!output.exists(), "{name}");
    }
}
