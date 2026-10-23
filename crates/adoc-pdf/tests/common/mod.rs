//! Helpers shared by the PDF tests: view trees built from fixtures with the local resolver.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use adoc_dom::{AcdcParser, DocumentParser as _, ParseInput};
use adoc_pdf::{load_theme, LoadedTheme, PageView, ThemeSource};
use adoc_render::{build_view, LocalResolver, ViewTree};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The view tree of `text`, as if it were the page at `path`.
pub fn view_of_text(path: &Path, text: &str) -> ViewTree {
    let dom = AcdcParser.parse(&ParseInput::new(path, text));
    build_view(path, &dom, &LocalResolver)
}

pub fn view_of(path: &Path) -> ViewTree {
    let text = std::fs::read_to_string(path).unwrap();
    view_of_text(path, &text)
}

pub fn article(view: ViewTree) -> Vec<PageView> {
    vec![PageView {
        key: "p0".to_owned(),
        view,
        depth: 0,
    }]
}

pub fn default_theme() -> LoadedTheme {
    load_theme(&ThemeSource::Bundled("default".into())).unwrap()
}
