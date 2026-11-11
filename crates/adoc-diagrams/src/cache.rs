use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// How long after an entry was last marked read [`DiagramCache::get`] marks it again: its
/// modification time is the day it was last read, written at most once a day.
pub const MARK_READ_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Rendered diagrams on disk, one SVG per checksum.
#[derive(Clone, Debug)]
pub struct DiagramCache {
    dir: PathBuf,
}

/// How many files the cache folder holds and their size together.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheUsage {
    pub entries: usize,
    pub bytes: u64,
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

    /// The image cached under `checksum`, marked read when it was last marked more than
    /// [`MARK_READ_AFTER`] ago, so [`DiagramCache::prune`] can tell read from written.
    #[must_use]
    pub fn get(&self, checksum: &str) -> Option<PathBuf> {
        let path = self.path(checksum);
        let metadata = fs::metadata(&path).ok().filter(fs::Metadata::is_file)?;
        let marked = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if marked.is_some_and(|age| age > MARK_READ_AFTER) {
            // Best effort: a cache that cannot be written to is read all the same.
            let _ = fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(SystemTime::now()));
        }
        Some(path)
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

    /// Removes the files not read (nor written) for `unused_for`, the images and any partly
    /// written file left behind, except those whose checksum is in `keep`, the images in use.
    /// Returns how many were removed. A folder that is missing or cannot be read, and a file
    /// that cannot be removed, are left as they are.
    #[must_use]
    pub fn prune(&self, unused_for: Duration, keep: &HashSet<String>) -> usize {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let now = SystemTime::now();
        let unused = |path: &Path| {
            fs::metadata(path)
                .ok()
                .filter(fs::Metadata::is_file)
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > unused_for)
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            // `<checksum>.svg`, or `<checksum>.svg.<pid>.tmp` while being written.
            let checksum = name.to_string_lossy();
            let checksum = checksum.split('.').next().unwrap_or_default();
            if keep.contains(checksum) || !unused(&path) {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// How many files the folder holds and their size: nothing for a folder that is missing
    /// or cannot be read.
    #[must_use]
    pub fn usage(&self) -> CacheUsage {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return CacheUsage::default();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(fs::Metadata::is_file)
            .fold(CacheUsage::default(), |usage, metadata| CacheUsage {
                entries: usage.entries + 1,
                bytes: usage.bytes + metadata.len(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        time::{Duration, SystemTime},
    };

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// Writes an entry last read `age` ago.
    fn entry(cache: &DiagramCache, checksum: &str, age: Duration) -> PathBuf {
        let path = cache.put(checksum, "<svg/>").unwrap();
        set_age(&path, age);
        path
    }

    fn set_age(path: &Path, age: Duration) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    fn age(path: &Path) -> Duration {
        let modified = fs::metadata(path).unwrap().modified().unwrap();
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
    }

    #[test]
    fn pruning_removes_entries_not_read_for_the_period() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiagramCache::for_workspace(dir.path());
        let old = entry(&cache, "old", 40 * DAY);
        let new = entry(&cache, "new", 2 * DAY);
        let shown = entry(&cache, "shown", 40 * DAY);
        let partial = cache.dir().join("stray.svg.123.tmp");
        fs::write(&partial, "<sv").unwrap();
        set_age(&partial, 40 * DAY);

        let keep: HashSet<String> = ["shown".to_owned()].into();
        let removed = cache.prune(30 * DAY, &keep);
        assert_eq!(removed, 2);
        assert!(!old.exists() && !partial.exists());
        assert!(new.exists(), "read within the period");
        assert!(shown.exists(), "an entry in use is kept, however old");
        assert_eq!(
            cache.usage(),
            CacheUsage {
                entries: 2,
                bytes: 12
            }
        );
    }

    #[test]
    fn reading_an_entry_marks_it_read_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiagramCache::for_workspace(dir.path());
        let stale = entry(&cache, "stale", 40 * DAY);
        let fresh = entry(&cache, "fresh", DAY / 2);
        assert_eq!(cache.get("stale"), Some(stale.clone()));
        assert!(age(&stale) < DAY, "read now: {:?}", age(&stale));
        // Read within the day: not written to again.
        assert_eq!(cache.get("fresh"), Some(fresh.clone()));
        assert!(age(&fresh) > DAY / 4, "{:?}", age(&fresh));

        assert_eq!(cache.prune(30 * DAY, &HashSet::new()), 0);
        assert!(stale.exists());
    }

    #[test]
    fn a_missing_folder_or_a_file_in_its_place_prunes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = DiagramCache::for_workspace(&dir.path().join("nowhere"));
        assert_eq!(missing.prune(DAY, &HashSet::new()), 0);
        assert_eq!(missing.usage(), CacheUsage::default());

        let blocked = DiagramCache::new(dir.path().join("file"));
        fs::write(blocked.dir(), "not a folder").unwrap();
        assert_eq!(blocked.prune(DAY, &HashSet::new()), 0);
        assert_eq!(blocked.usage(), CacheUsage::default());
        assert!(blocked.dir().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_prunes_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let cache = DiagramCache::for_workspace(dir.path());
        let old = entry(&cache, "old", 40 * DAY);
        fs::set_permissions(cache.dir(), fs::Permissions::from_mode(0o000)).unwrap();
        let removed = cache.prune(DAY, &HashSet::new());
        fs::set_permissions(cache.dir(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(removed, 0);
        assert!(old.exists());
    }
}
