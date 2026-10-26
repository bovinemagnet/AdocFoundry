//! Directory listings for the Files view: the folders and files beneath the workspace roots, as
//! they are on disk, whatever their type and whether or not Antora or Git knows them.
//!
//! A folder is listed only when asked for, one level deep, on a worker, and kept until the
//! watcher says it changed. Listing reads names and types, never contents. Entries named in
//! `workspace.ignore` are listed but hidden unless Show Excluded Files is on. Symbolic links are
//! shown as links and never followed, so a link cannot lead outside the roots or round in a loop.
//! A result for a workspace no longer open, or overtaken by a newer request, is dropped.

use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    thread,
};

use adoc_index::normalize_path;

use crate::{workspace::WorkerMessage, Engine, EngineError, EngineEvent};

/// What a directory entry is. A link is not followed: `to_directory` says what it points at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Directory,
    File,
    Link { to_directory: bool },
}

/// One entry of a listed folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    /// The name as shown; [`DirectoryEntry::path`] holds it exactly.
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Named in `workspace.ignore`: shown only with Show Excluded Files.
    pub excluded: bool,
}

/// Why a folder could not be listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryError {
    PermissionDenied,
    /// Missing, not a folder, or another failure, with what the system said.
    Unavailable(String),
}

/// A folder's listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Listing {
    Loading,
    Loaded(Vec<DirectoryEntry>),
    Failed(DirectoryError),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Files {
    listings: BTreeMap<PathBuf, Listing>,
    /// The request each folder's listing must answer; older answers are dropped.
    requests: BTreeMap<PathBuf, u64>,
    next_request: u64,
    show_excluded: bool,
}

impl Files {
    /// Forgets every listing, for a newly opened workspace.
    pub(crate) fn clear(&mut self) {
        self.listings.clear();
        self.requests.clear();
    }
}

/// Orders entries as the Files view shows them: folders (and links to folders) first, then by
/// name, ignoring case and reading runs of digits as numbers, with the exact name breaking ties.
fn compare(left: &DirectoryEntry, right: &DirectoryEntry) -> Ordering {
    let folder = |entry: &DirectoryEntry| {
        !matches!(
            entry.kind,
            EntryKind::Directory | EntryKind::Link { to_directory: true }
        )
    };
    folder(left)
        .cmp(&folder(right))
        .then_with(|| natural(&left.name, &right.name))
        .then_with(|| left.path.as_os_str().cmp(right.path.as_os_str()))
}

/// Compares names case-insensitively, with runs of digits compared as numbers.
fn natural(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let take = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(digit);
                        chars.next();
                    }
                    digits
                };
                let (a, b) = (take(&mut left), take(&mut right));
                let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(a), Some(b)) => {
                let order = a.to_lowercase().cmp(b.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                left.next();
                right.next();
            }
        }
    }
}

/// Lists `dir` one level deep: names and types only.
fn read_listing(dir: &Path, ignore: &[String]) -> Listing {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Listing::Failed(failure(&error)),
    };
    let mut listed = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        // `DirEntry::file_type` does not follow links.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_symlink() {
            EntryKind::Link {
                to_directory: fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()),
            }
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let excluded = ignore.contains(&name);
        listed.push(DirectoryEntry {
            name,
            path: normalize_path(&path),
            kind,
            excluded,
        });
    }
    listed.sort_by(compare);
    Listing::Loaded(listed)
}

fn failure(error: &io::Error) -> DirectoryError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => DirectoryError::PermissionDenied,
        _ => DirectoryError::Unavailable(error.to_string()),
    }
}

impl Engine {
    /// `dir`'s listing: `None` until it is requested. Excluded entries are left out unless
    /// Show Excluded Files is on.
    #[must_use]
    pub fn directory(&self, dir: &Path) -> Option<Listing> {
        let listing = self.files.listings.get(&normalize_path(dir))?;
        Some(match listing {
            Listing::Loaded(entries) if !self.files.show_excluded => Listing::Loaded(
                entries
                    .iter()
                    .filter(|entry| !entry.excluded)
                    .cloned()
                    .collect(),
            ),
            other => other.clone(),
        })
    }

    /// `dir`'s listing with its excluded entries, whether or not they are shown: for revealing
    /// a file beneath an excluded folder.
    #[must_use]
    pub fn directory_with_excluded(&self, dir: &Path) -> Option<Listing> {
        self.files.listings.get(&normalize_path(dir)).cloned()
    }

    /// Lists `dir`, a folder beneath a workspace root, unless it is listed or being listed.
    /// [`EngineEvent::DirectoryListed`] says when the listing is ready.
    pub fn request_directory(&mut self, dir: &Path) -> Result<(), EngineError> {
        let dir = self.listable(dir)?;
        if self.files.listings.contains_key(&dir) {
            return Ok(());
        }
        self.list_directory(dir);
        Ok(())
    }

    /// Lists `dir` again, as after a failure or a change the watcher missed.
    pub fn refresh_directory(&mut self, dir: &Path) -> Result<(), EngineError> {
        let dir = self.listable(dir)?;
        self.list_directory(dir);
        Ok(())
    }

    /// Lists again every folder listed so far.
    pub fn refresh_directories(&mut self) {
        let listed: Vec<PathBuf> = self.files.listings.keys().cloned().collect();
        for dir in listed {
            self.list_directory(dir);
        }
    }

    /// Whether entries named in `workspace.ignore` are shown.
    #[must_use]
    pub fn show_excluded(&self) -> bool {
        self.files.show_excluded
    }

    /// Shows or hides entries named in `workspace.ignore`. Nothing else changes: the settings,
    /// the index and Antora's resources stay as they are.
    pub fn set_show_excluded(&mut self, show: bool) {
        if self.files.show_excluded == show {
            return;
        }
        self.files.show_excluded = show;
        let listed: Vec<PathBuf> = self.files.listings.keys().cloned().collect();
        for dir in listed {
            self.emit(EngineEvent::DirectoryListed(dir));
        }
    }

    /// `dir`, normalised, if it is a root or a folder beneath one, reached without a link.
    fn listable(&self, dir: &Path) -> Result<PathBuf, EngineError> {
        let dir = normalize_path(dir);
        let Some(root) = self.roots.iter().find(|root| dir.starts_with(root)) else {
            return Err(EngineError::Refused(format!(
                "{} is outside the workspace folders",
                dir.display()
            )));
        };
        // Every folder between the root and `dir` is a folder, not a link to one.
        let mut step = dir.as_path();
        while step != root.as_path() {
            if fs::symlink_metadata(step).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err(EngineError::Refused(format!(
                    "{} is a link, which the Files view does not follow",
                    step.display()
                )));
            }
            let Some(parent) = step.parent() else { break };
            step = parent;
        }
        Ok(dir)
    }

    fn list_directory(&mut self, dir: PathBuf) {
        self.files.next_request += 1;
        let request = self.files.next_request;
        self.files.requests.insert(dir.clone(), request);
        // A folder listed before keeps its entries until the new listing arrives.
        self.files
            .listings
            .entry(dir.clone())
            .or_insert(Listing::Loading);
        let ignore = self.settings.workspace.ignore.clone();
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        thread::spawn(move || {
            let listing = read_listing(&dir, &ignore);
            let _ = sender.send(WorkerMessage::DirectoryListed {
                request,
                dir,
                listing,
            });
        });
    }

    pub(crate) fn directory_listed(&mut self, request: u64, dir: PathBuf, listing: Listing) {
        if self.files.requests.get(&dir) != Some(&request) {
            return; // A newer request, or another workspace, owns this folder now.
        }
        self.files.requests.remove(&dir);
        self.files.listings.insert(dir.clone(), listing);
        self.emit(EngineEvent::DirectoryListed(dir));
    }

    /// Lists again the folders holding `changed` paths, and those paths if they are folders.
    pub(crate) fn directories_changed(&mut self, changed: &[PathBuf]) {
        let mut stale: Vec<PathBuf> = Vec::new();
        for path in changed {
            for dir in [Some(path.as_path()), path.parent()].into_iter().flatten() {
                if self.files.listings.contains_key(dir) && !stale.iter().any(|seen| seen == dir) {
                    stale.push(dir.to_path_buf());
                }
            }
        }
        for dir in stale {
            self.list_directory(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_naturally_without_case() {
        let mut names = vec!["file10", "File2", "file2", "a", "B", "file1b", "file01"];
        names.sort_by(|left, right| natural(left, right).then_with(|| left.cmp(right)));
        assert_eq!(
            names,
            ["a", "B", "file01", "file1b", "File2", "file2", "file10"]
        );
    }

    #[test]
    fn folders_come_first_and_distinct_names_stay_distinct() {
        let entry = |name: &str, kind| DirectoryEntry {
            name: name.to_owned(),
            path: PathBuf::from(format!("/r/{name}")),
            kind,
            excluded: false,
        };
        let mut entries = [
            entry("b.txt", EntryKind::File),
            entry("Readme", EntryKind::File),
            entry("readme", EntryKind::File),
            entry("zdir", EntryKind::Directory),
            entry("link", EntryKind::Link { to_directory: true }),
            entry(
                "alias",
                EntryKind::Link {
                    to_directory: false,
                },
            ),
        ];
        entries.sort_by(compare);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            ["link", "zdir", "alias", "b.txt", "Readme", "readme"]
        );
    }
}
