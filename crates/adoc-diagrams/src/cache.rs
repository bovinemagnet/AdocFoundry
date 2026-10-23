use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::DiagramRequest;

/// Rendered diagrams on disk, one SVG per checksum.
#[derive(Clone, Debug)]
pub struct DiagramCache {
    dir: PathBuf,
}

impl DiagramCache {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `<workspace>/.cache/adocfoundry/diagrams`.
    #[must_use]
    pub fn for_workspace(root: &Path) -> Self {
        Self::new(root.join(".cache").join("adocfoundry").join("diagrams"))
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn path(&self, request: &DiagramRequest) -> PathBuf {
        self.dir.join(format!("{}.svg", request.checksum()))
    }

    #[must_use]
    pub fn get(&self, request: &DiagramRequest) -> Option<PathBuf> {
        let path = self.path(request);
        path.is_file().then_some(path)
    }

    /// Stores `svg` for `request`. The file is written beside its final name and renamed, so a
    /// reader never sees half an image.
    ///
    /// # Errors
    ///
    /// When the cache directory or file cannot be written.
    pub fn put(&self, request: &DiagramRequest, svg: &str) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(request);
        let partial = path.with_extension(format!("svg.{}.tmp", std::process::id()));
        fs::write(&partial, svg)?;
        fs::rename(&partial, &path)?;
        Ok(path)
    }
}
