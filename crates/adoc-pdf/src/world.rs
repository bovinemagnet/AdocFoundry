//! The Typst `World` a document compiles in: the generated source, the theme's fonts, and the
//! image files the document uses. Nothing else is reachable: there are no packages and no
//! access to the file system beyond the files listed.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use typst::{
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
    Library, LibraryExt as _, World,
};

use crate::fonts::FontSet;

fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| LazyHash::new(Library::default()))
}

/// The id of a file in the project, at a virtual path such as `/main.typ`.
///
/// Typst interns ids for the life of the process, so callers keep paths stable between
/// exports: the same file always gets the same virtual path.
pub(crate) fn file_id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path)
        .or_else(|_| VirtualPath::new("/invalid"))
        .expect("a valid virtual path");
    RootedPath::new(VirtualRoot::Project, vpath).intern()
}

/// The generated document's virtual path.
pub(crate) const MAIN: &str = "/main.typ";

pub(crate) struct PdfWorld {
    main: Source,
    fonts: Vec<Font>,
    book: LazyHash<FontBook>,
    /// Virtual path to the file it stands for.
    files: HashMap<FileId, PathBuf>,
    today: Option<Datetime>,
}

impl PdfWorld {
    /// A world compiling `source` with `fonts` and the files mapped by `files` (virtual path,
    /// real path).
    pub(crate) fn new(
        source: String,
        fonts: FontSet,
        files: &[(String, PathBuf)],
        today: Option<Datetime>,
    ) -> Self {
        let book = LazyHash::new(FontBook::from_infos(fonts.infos));
        Self {
            main: Source::new(file_id(MAIN), source),
            fonts: fonts.fonts,
            book,
            files: files
                .iter()
                .map(|(virtual_path, path)| (file_id(virtual_path), path.clone()))
                .collect(),
            today,
        }
    }
}

impl World for PdfWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotSource)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main.id() {
            return Ok(Bytes::from_string(self.main.clone()));
        }
        let path = self
            .files
            .get(&id)
            .ok_or_else(|| FileError::NotFound(id.vpath().get_with_slash().into()))?;
        read(path)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        self.today
    }
}

fn read(path: &Path) -> FileResult<Bytes> {
    std::fs::read(path)
        .map(Bytes::new)
        .map_err(|error| FileError::from_io(error, path))
}
