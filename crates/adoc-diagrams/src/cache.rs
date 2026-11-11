use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// How long after an entry was last marked read [`DiagramCache::get`] marks it again: its
/// modification time is the day it was last read, written at most once a day.
pub const MARK_READ_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Rendered diagrams on disk, one SVG per checksum.
///
/// The folder usually lies inside a workspace, which a repository someone else wrote may
/// have filled, so nothing is read, written or removed through a link: the folder, and each
/// folder above it up to the workspace, must be a folder of its own, and only regular files
/// named as the cache names them are read or removed.
#[derive(Clone, Debug)]
pub struct DiagramCache {
    dir: PathBuf,
    /// The workspace the folder is in, up to which no folder may be a link.
    root: Option<PathBuf>,
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
        Self {
            dir: dir.into(),
            root: None,
        }
    }

    /// `<workspace>/.cache/adocfoundry/diagrams`, where none of `.cache`, `adocfoundry` and
    /// `diagrams` may be a link.
    #[must_use]
    pub fn for_workspace(root: &Path) -> Self {
        Self {
            dir: root.join(".cache").join("adocfoundry").join("diagrams"),
            root: Some(root.to_path_buf()),
        }
    }

    /// Whether the folder, and every folder above it inside the workspace, is a folder of its
    /// own rather than a link: false while it does not exist.
    fn is_own_folder(&self) -> bool {
        let real = |path: &Path| {
            fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
        };
        let mut folder = self.dir.as_path();
        loop {
            if !real(folder) {
                return false;
            }
            match (&self.root, folder.parent()) {
                (Some(root), Some(parent)) if parent != root && parent.starts_with(root) => {
                    folder = parent;
                }
                _ => return true,
            }
        }
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
        // A regular file in a folder of its own: a link is never read as an image.
        let metadata = fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.file_type().is_file())?;
        if !self.is_own_folder() {
            return None;
        }
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

    /// Stores `svg` for the request with this checksum. The file is written as a new file
    /// beside its final name and renamed, so a reader never sees half an image, and a link at
    /// either name is replaced, never written through.
    ///
    /// # Errors
    ///
    /// When the cache directory or file cannot be written, or the folder is reached through a
    /// link.
    pub fn put(&self, checksum: &str, svg: &str) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        if !self.is_own_folder() {
            return Err(io::Error::other(format!(
                "the diagram cache {} is reached through a link, so nothing is written there",
                self.dir.display()
            )));
        }
        let path = self.path(checksum);
        let partial = path.with_extension(format!("svg.{}.tmp", std::process::id()));
        // Whatever is at the partial name, a link included, goes; the file is made anew.
        let _ = fs::remove_file(&partial);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        file.write_all(svg.as_bytes())?;
        drop(file);
        fs::rename(&partial, &path)?;
        Ok(path)
    }

    /// Removes the files not read (nor written) for `unused_for`, the images and any partly
    /// written file left behind, except those whose checksum is in `keep`, the images in use.
    /// Returns how many were removed. A folder that is missing or cannot be read, and a file
    /// that cannot be removed, are left as they are.
    /// Only the cache's own files are removed: regular files, never links, named
    /// `<checksum>.svg` or `<checksum>.svg.<pid>.tmp`, in a folder that is not reached
    /// through a link.
    #[must_use]
    pub fn prune(&self, unused_for: Duration, keep: &HashSet<String>) -> usize {
        if !self.is_own_folder() {
            return 0;
        }
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in entries.flatten() {
            // The entry itself, not what a link leads to.
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some(checksum) = name.to_str().and_then(cache_file_checksum) else {
                continue;
            };
            let unused = fs::symlink_metadata(entry.path())
                .ok()
                .filter(|metadata| metadata.file_type().is_file())
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > unused_for);
            if keep.contains(checksum) || !unused {
                continue;
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// How many files the folder holds and their size: nothing for a folder that is missing
    /// or cannot be read.
    #[must_use]
    pub fn usage(&self) -> CacheUsage {
        if !self.is_own_folder() {
            return CacheUsage::default();
        }
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return CacheUsage::default();
        };
        entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.metadata().ok())
            .fold(CacheUsage::default(), |usage, metadata| CacheUsage {
                entries: usage.entries + 1,
                bytes: usage.bytes + metadata.len(),
            })
    }
}

/// The checksum of a file the cache writes: `<checksum>.svg`, or `<checksum>.svg.<pid>.tmp`
/// while it is being written, the checksum 64 lower-case hexadecimal digits. `None` for any
/// other name.
fn cache_file_checksum(name: &str) -> Option<&str> {
    let (checksum, rest) = name.split_once('.')?;
    let hex = checksum.len() == 64
        && checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let ours = rest == "svg"
        || rest
            .strip_prefix("svg.")
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()));
    (hex && ours).then_some(checksum)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        time::{Duration, SystemTime},
    };

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// A checksum as the cache names its files by, made from `seed`.
    fn sum(seed: &str) -> String {
        format!(
            "{:0>64}",
            seed.bytes()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }

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
        let old = entry(&cache, &sum("old"), 40 * DAY);
        let new = entry(&cache, &sum("new"), 2 * DAY);
        let shown = entry(&cache, &sum("shown"), 40 * DAY);
        let partial = cache.dir().join(format!("{}.svg.123.tmp", sum("stray")));
        fs::write(&partial, "<sv").unwrap();
        set_age(&partial, 40 * DAY);

        let keep: HashSet<String> = [sum("shown")].into();
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
        let stale = entry(&cache, &sum("stale"), 40 * DAY);
        let fresh = entry(&cache, &sum("fresh"), DAY / 2);
        assert_eq!(cache.get(&sum("stale")), Some(stale.clone()));
        assert!(age(&stale) < DAY, "read now: {:?}", age(&stale));
        // Read within the day: not written to again.
        assert_eq!(cache.get(&sum("fresh")), Some(fresh.clone()));
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
        let old = entry(&cache, &sum("old"), 40 * DAY);
        fs::set_permissions(cache.dir(), fs::Permissions::from_mode(0o000)).unwrap();
        let removed = cache.prune(DAY, &HashSet::new());
        fs::set_permissions(cache.dir(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(removed, 0);
        assert!(old.exists());
    }

    #[test]
    fn only_the_cache_s_own_files_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiagramCache::for_workspace(dir.path());
        let ours = entry(&cache, &sum("ours"), 40 * DAY);
        let theirs = [
            cache.dir().join("notes.txt"),
            cache.dir().join("short.svg"),
            cache.dir().join(format!("{}.png", sum("image"))),
            cache.dir().join(format!("{}.svg.x.tmp", sum("odd"))),
        ];
        for path in &theirs {
            fs::write(path, "kept").unwrap();
            set_age(path, 40 * DAY);
        }
        fs::create_dir(cache.dir().join(format!("{}.svg", sum("folder")))).unwrap();
        assert_eq!(cache.prune(DAY, &HashSet::new()), 1);
        assert!(!ours.exists());
        for path in &theirs {
            assert!(path.exists(), "{}", path.display());
        }
    }

    /// An outside folder holding a file named as the cache names its images, last read long ago.
    #[cfg(unix)]
    fn outside_victim(dir: &Path) -> (PathBuf, PathBuf) {
        let outside = dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let victim = outside.join(format!("{}.svg", sum("victim")));
        fs::write(&victim, "precious").unwrap();
        set_age(&victim, 40 * DAY);
        (outside, victim)
    }

    #[cfg(unix)]
    #[test]
    fn a_cache_folder_reached_through_a_link_is_not_pruned_or_written() {
        use std::os::unix::fs::symlink;

        for linked in [
            ".cache",
            ".cache/adocfoundry",
            ".cache/adocfoundry/diagrams",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (outside, victim) = outside_victim(dir.path());
            let root = dir.path().join("workspace");
            let link = root.join(linked);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside, &link).unwrap();
            let cache = DiagramCache::for_workspace(&root);
            // The folder below the link, made where the link leads.
            fs::create_dir_all(cache.dir()).unwrap();
            let victim_there = cache.dir().join(victim.file_name().unwrap());
            if victim_there != link.join(victim.file_name().unwrap()) {
                fs::write(&victim_there, "precious").unwrap();
                set_age(&victim_there, 40 * DAY);
            }

            assert_eq!(cache.prune(DAY, &HashSet::new()), 0, "{linked}");
            assert!(victim_there.exists() && victim.exists(), "{linked}");
            assert!(cache.put(&sum("new"), "<svg/>").is_err(), "{linked}");
            assert_eq!(cache.get(&sum("victim")), None, "{linked}");
            assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_cache_is_neither_followed_nor_written_through() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let (_outside, victim) = outside_victim(dir.path());
        let cache = DiagramCache::for_workspace(&dir.path().join("workspace"));
        fs::create_dir_all(cache.dir()).unwrap();
        let link = cache.path(&sum("link"));
        symlink(&victim, &link).unwrap();
        let before = fs::metadata(&victim).unwrap().modified().unwrap();

        assert_eq!(cache.prune(DAY, &HashSet::new()), 0);
        assert!(victim.exists());
        // Not read as an image, nor marked read through the link.
        assert_eq!(cache.get(&sum("link")), None);
        assert_eq!(fs::metadata(&victim).unwrap().modified().unwrap(), before);

        // A link at the image's name, or at the name it is written under first, is replaced.
        let partial = cache
            .path(&sum("partial"))
            .with_extension(format!("svg.{}.tmp", std::process::id()));
        symlink(&victim, &partial).unwrap();
        cache.put(&sum("partial"), "<svg>mine</svg>").unwrap();
        cache.put(&sum("link"), "<svg>mine</svg>").unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        assert!(!fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), "<svg>mine</svg>");
    }
}
