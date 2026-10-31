use std::{
    fs, io,
    path::{Path, PathBuf},
};

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

    /// Where the image of the request with this [`crate::DiagramRequest::checksum`] is kept.
    #[must_use]
    pub fn path(&self, checksum: &str) -> PathBuf {
        self.dir.join(format!("{checksum}.svg"))
    }

    #[must_use]
    pub fn get(&self, checksum: &str) -> Option<PathBuf> {
        let path = self.path(checksum);
        path.is_file().then_some(path)
    }

    /// Stores `svg` for the request with this checksum. The file is written beside its final name and renamed, so a
    /// reader never sees half an image.
    ///
    /// # Errors
    ///
    /// When the cache directory or file cannot be written.
    pub fn put(&self, checksum: &str, svg: &str) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(checksum);
        let partial = path.with_extension(format!("svg.{}.tmp", std::process::id()));
        fs::write(&partial, svg)?;
        fs::rename(&partial, &path)?;
        Ok(path)
    }
}
