//! Who changed which file in a collaboration session, kept until the file is committed, so the
//! host's commit can credit them (`docs/prd/PRD-AdocFoundry_CoAuthors.md`).
//!
//! The record lives in memory and, when the engine has a state folder, in [`STATE_FILE`] there.
//! The file is read as untrusted, and written on the engine's writer by laying this engine's
//! changes over what the file holds then, so two windows hosting at once keep each other's.

// Recording needs the `collab` feature and committing the `git` one; built without either,
// part of the record goes unused.
#![cfg_attr(not(all(feature = "collab", feature = "git")), allow(dead_code))]

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// The state file, in the engine's state folder.
pub const STATE_FILE: &str = "coauthors.json";
/// The largest state file read, in bytes.
pub const MAX_STATE: usize = 1024 * 1024;
/// The most files the record holds.
pub const MAX_FILES: usize = 10_000;
/// The most co-authors recorded for one file.
pub const MAX_AUTHORS: usize = 100;
/// The longest name kept, in bytes: as long as a participant's name in a session.
const MAX_NAME: usize = 64;
/// The longest address kept, in bytes.
const MAX_EMAIL: usize = 254;
const FORMAT: u64 = 1;

/// Someone to credit in a commit's `Co-authored-by:` trailer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoAuthor {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Entry {
    name: String,
    email: String,
    /// When their latest recorded change was made, in milliseconds since 1970.
    changed: u64,
}

/// A change to the record, laid over the state file by the writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Change {
    Record {
        path: PathBuf,
        name: String,
        email: String,
        changed: u64,
    },
    Forget {
        path: PathBuf,
        fixed: u64,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Drop {
        path: PathBuf,
    },
}

/// Each file's co-authors, in the order first recorded.
#[derive(Clone, Debug, Default)]
struct Files(BTreeMap<PathBuf, Vec<Entry>>);

impl Files {
    fn authors(&self, path: &Path) -> &[Entry] {
        self.0.get(path).map_or(&[], Vec::as_slice)
    }

    /// Returns whether the address is new to the file.
    fn record(&mut self, path: &Path, name: &str, email: &str, changed: u64) -> bool {
        if !self.0.contains_key(path) && self.0.len() >= MAX_FILES {
            return false;
        }
        let entries = self.0.entry(path.to_path_buf()).or_default();
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.email.eq_ignore_ascii_case(email))
        {
            entry.name = name.to_owned();
            entry.changed = entry.changed.max(changed);
            return false;
        }
        if entries.len() >= MAX_AUTHORS {
            return false;
        }
        entries.push(Entry {
            name: name.to_owned(),
            email: email.to_owned(),
            changed,
        });
        true
    }

    fn forget(&mut self, path: &Path, fixed: u64) -> bool {
        let Some(entries) = self.0.get_mut(path) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|entry| entry.changed > fixed);
        let changed = entries.len() != before;
        if entries.is_empty() {
            self.0.remove(path);
        }
        changed
    }

    fn moved(&mut self, from: &Path, to: &Path) -> bool {
        let Some(entries) = self.0.remove(from) else {
            return false;
        };
        for entry in entries {
            self.record(to, &entry.name, &entry.email, entry.changed);
        }
        true
    }

    fn apply(&mut self, change: &Change) {
        match change {
            Change::Record {
                path,
                name,
                email,
                changed,
            } => {
                self.record(path, name, email, *changed);
            }
            Change::Forget { path, fixed } => {
                self.forget(path, *fixed);
            }
            Change::Move { from, to } => {
                self.moved(from, to);
            }
            Change::Drop { path } => {
                self.0.remove(path);
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StateFile {
    format: u64,
    files: BTreeMap<String, Vec<Entry>>,
}

/// The record held in a state file's text, or `None` when it breaks a limit, does not parse or
/// is of a newer format.
fn parse(text: &str) -> Option<Files> {
    if text.len() > MAX_STATE {
        return None;
    }
    let state: StateFile = serde_json::from_str(text).ok()?;
    if state.format != FORMAT || state.files.len() > MAX_FILES {
        return None;
    }
    let mut files = Files::default();
    for (path, entries) in state.files {
        if entries.len() > MAX_AUTHORS
            || !entries
                .iter()
                .all(|entry| is_name(&entry.name) && is_commit_email(&entry.email))
        {
            return None;
        }
        if !entries.is_empty() {
            files.0.insert(PathBuf::from(path), entries);
        }
    }
    Some(files)
}

fn render(files: &Files) -> Option<String> {
    let state = StateFile {
        format: FORMAT,
        files: files
            .0
            .iter()
            .map(|(path, entries)| Some((path.to_str()?.to_owned(), entries.clone())))
            .collect::<Option<_>>()?,
    };
    serde_json::to_string(&state).ok()
}

/// The state file's text, `None` when there is none, or an error when it cannot be read or is
/// too large to be one this engine wrote.
fn read_state(file: &Path) -> Result<Option<String>, ()> {
    match fs::File::open(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
        Ok(opened) => {
            let mut text = String::new();
            opened
                .take(MAX_STATE as u64 + 1)
                .read_to_string(&mut text)
                .map_err(|_| ())?;
            Ok(Some(text))
        }
    }
}

/// The state file once `changes` are laid over `on_disk` (none when there is no file yet), or
/// `None` when what is on disk is unusable, so nothing is written over it.
pub(crate) fn apply_changes(on_disk: Option<&str>, changes: &[Change]) -> Option<String> {
    let mut files = match on_disk {
        None => Files::default(),
        Some(text) => parse(text)?,
    };
    for change in changes {
        files.apply(change);
    }
    render(&files)
}

/// Writes `changes` to `file`, reading it first: the contents the writer writes.
pub(crate) fn write_changes(file: &Path, changes: &[Change]) -> Option<String> {
    let on_disk = read_state(file).ok()?;
    apply_changes(on_disk.as_deref(), changes)
}

/// Who changed which file, and what is still to be written.
#[derive(Debug)]
pub(crate) struct CoAuthors {
    file: Option<PathBuf>,
    files: Files,
    changes: Vec<Change>,
    /// The state file could not be used: it is left alone, and the record kept in memory.
    pub(crate) unusable: bool,
}

impl CoAuthors {
    /// The record kept in `file`, or in memory only without one.
    pub(crate) fn load(file: Option<PathBuf>) -> Self {
        let (files, unusable) = match file.as_deref().map(read_state) {
            None | Some(Ok(None)) => (Files::default(), false),
            Some(Ok(Some(text))) => match parse(&text) {
                Some(files) => (files, false),
                None => (Files::default(), true),
            },
            Some(Err(())) => (Files::default(), true),
        };
        Self {
            file,
            files,
            changes: Vec::new(),
            unusable,
        }
    }

    pub(crate) fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Records a change by `email` to `path` at `at`, returning whether the state file needs
    /// writing, which is only when the address is new to the file. The time is kept and goes
    /// with the next write.
    pub(crate) fn record(&mut self, path: &Path, name: &str, email: &str, at: u64) -> bool {
        if !is_commit_email(email) {
            return false;
        }
        let name = clean_name(name, email);
        let new = self.files.record(path, &name, email, at);
        let change = Change::Record {
            path: path.to_path_buf(),
            name,
            email: email.to_owned(),
            changed: at,
        };
        // A pending record of the same person and file since the last other change is replaced,
        // so a long session's keystrokes do not pile up.
        let same = self
            .changes
            .iter()
            .rev()
            .take_while(|pending| matches!(pending, Change::Record { .. }))
            .position(|pending| {
                matches!(pending, Change::Record { path: p, email: e, .. }
                    if p == path && e.eq_ignore_ascii_case(email))
            });
        match same {
            Some(back) => {
                let at = self.changes.len() - 1 - back;
                self.changes[at] = change;
            }
            None => self.changes.push(change),
        }
        new
    }

    /// The co-authors of `paths`, each address once, in the order first recorded.
    pub(crate) fn for_paths(&self, paths: &[PathBuf]) -> Vec<CoAuthor> {
        let mut found: Vec<CoAuthor> = Vec::new();
        for entry in paths.iter().flat_map(|path| self.files.authors(path)) {
            match found
                .iter_mut()
                .find(|author| author.email.eq_ignore_ascii_case(&entry.email))
            {
                Some(author) => author.name.clone_from(&entry.name),
                None => found.push(CoAuthor {
                    name: entry.name.clone(),
                    email: entry.email.clone(),
                }),
            }
        }
        found
    }

    /// After a commit: each file it took, with the time its committed content was fixed. Whoever
    /// changed a file later keeps their record, written with their latest time.
    pub(crate) fn forget(&mut self, fixed: &[(PathBuf, u64)]) -> bool {
        let mut changed = false;
        for (path, fixed) in fixed {
            if !self.files.forget(path, *fixed) {
                continue;
            }
            changed = true;
            self.changes.push(Change::Forget {
                path: path.clone(),
                fixed: *fixed,
            });
            for entry in self.files.authors(path) {
                self.changes.push(Change::Record {
                    path: path.clone(),
                    name: entry.name.clone(),
                    email: entry.email.clone(),
                    changed: entry.changed,
                });
            }
        }
        changed
    }

    /// A file renamed: its record follows it.
    pub(crate) fn moved(&mut self, from: &Path, to: &Path) -> bool {
        if !self.files.moved(from, to) {
            return false;
        }
        self.changes.push(Change::Move {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        true
    }

    /// Drops the records of `paths`, whose changes have nothing left to credit.
    pub(crate) fn drop_paths(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        for path in paths {
            if self.files.0.remove(path).is_some() {
                changed = true;
                self.changes.push(Change::Drop { path: path.clone() });
            }
        }
        changed
    }

    /// The files with a record.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.0.keys().map(PathBuf::as_path)
    }

    /// The changes since the last call, for the writer; none when the record is in memory only
    /// or its file is unusable.
    pub(crate) fn take_changes(&mut self) -> Option<Vec<Change>> {
        let changes = std::mem::take(&mut self.changes);
        (self.file.is_some() && !self.unusable && !changes.is_empty()).then_some(changes)
    }
}

/// A file's path once links are followed; for a file that has gone, its folder's real path and
/// its name.
pub(crate) fn real_path(path: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match fs::canonicalize(parent) {
            Ok(parent) => parent.join(name),
            Err(_) => adoc_index::normalize_path(path),
        },
        _ => adoc_index::normalize_path(path),
    }
}

/// Milliseconds since 1970.
pub(crate) fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// An address a `Co-authored-by:` trailer can carry: the session protocol's rule
/// (`adoc_collab::protocol::commit_email`), here too since the engine builds without it.
pub(crate) fn is_commit_email(email: &str) -> bool {
    let refused =
        |c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | '"' | '\\');
    matches!(email.split_once('@'), Some((local, domain))
        if email.len() <= MAX_EMAIL
            && !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && !email.chars().any(refused))
}

fn is_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= MAX_NAME && !name.chars().any(char::is_control)
}

/// A name a trailer can carry: no `<`, `>` or control character, at most [`MAX_NAME`] bytes;
/// the address when nothing is left.
fn clean_name(name: &str, email: &str) -> String {
    let mut clean: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>') && !c.is_control())
        .collect();
    while clean.len() > MAX_NAME {
        clean.pop();
    }
    let clean = clean.trim();
    if clean.is_empty() {
        email.to_owned()
    } else {
        clean.to_owned()
    }
}

/// The address a `Co-authored-by:` line names, if it is one.
fn trailer_address(line: &str) -> Option<&str> {
    let (key, value) = line.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case("co-authored-by") {
        return None;
    }
    let start = value.rfind('<')?;
    let end = value[start..].find('>')? + start;
    Some(value[start + 1..end].trim())
}

fn is_trailer(line: &str) -> bool {
    let Some((key, _)) = line.split_once(": ") else {
        return false;
    };
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `message` with a `Co-authored-by:` trailer for each of `coauthors` not already in it and not
/// one of the `committer`'s addresses, after a blank line, or added to the message's trailers
/// when it ends with some; unchanged when there are none to add.
pub(crate) fn with_trailers(message: &str, coauthors: &[CoAuthor], committer: &[&str]) -> String {
    let mut named: Vec<String> = message
        .lines()
        .filter_map(trailer_address)
        .map(str::to_owned)
        .collect();
    named.extend(committer.iter().map(|email| (*email).to_owned()));
    let mut lines = Vec::new();
    for author in coauthors {
        if !is_commit_email(&author.email)
            || named
                .iter()
                .any(|email| email.eq_ignore_ascii_case(&author.email))
        {
            continue;
        }
        named.push(author.email.clone());
        lines.push(format!(
            "Co-authored-by: {} <{}>",
            clean_name(&author.name, &author.email),
            author.email
        ));
    }
    if lines.is_empty() {
        return message.to_owned();
    }
    let body = message.trim_end();
    let in_block = body
        .rsplit_once("\n\n")
        .is_some_and(|(_, last)| last.lines().all(is_trailer));
    let gap = if in_block { "\n" } else { "\n\n" };
    format!("{body}{gap}{}\n", lines.join("\n"))
}

impl crate::Engine {
    fn coauthors_file(&self) -> Option<PathBuf> {
        self.settings.state_dir().map(|dir| dir.join(STATE_FILE))
    }

    /// The record, read from the state folder on first use.
    pub(crate) fn coauthors(&self) -> &CoAuthors {
        self.coauthors
            .get_or_init(|| CoAuthors::load(self.coauthors_file()))
    }

    pub(crate) fn coauthors_mut(&mut self) -> &mut CoAuthors {
        if self.coauthors.get().is_none() {
            let loaded = CoAuthors::load(self.coauthors_file());
            let _ = self.coauthors.set(loaded);
        }
        self.coauthors.get_mut().expect("loaded above")
    }

    /// The guests recorded as having changed any of `paths` in a collaboration session since
    /// they were last committed, each address once.
    #[must_use]
    pub fn coauthors_of(&self, paths: &[PathBuf]) -> Vec<CoAuthor> {
        let real: Vec<PathBuf> = paths.iter().map(|path| real_path(path)).collect();
        self.coauthors().for_paths(&real)
    }

    /// Writes the record's changes to its state file on the writer, if it has one.
    pub(crate) fn save_coauthors(&mut self) {
        let record = self.coauthors_mut();
        let Some(changes) = record.take_changes() else {
            return;
        };
        let Some(file) = record.file().map(Path::to_path_buf) else {
            return;
        };
        let target = file.clone();
        self.write_later(file, move || write_changes(&target, &changes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> CoAuthors {
        CoAuthors::load(None)
    }

    fn ada() -> CoAuthor {
        CoAuthor {
            name: "Ada".to_owned(),
            email: "ada@example.org".to_owned(),
        }
    }

    #[test]
    fn recording_is_written_only_for_someone_new() {
        let mut store = store();
        let (a, b) = (PathBuf::from("/w/a.adoc"), PathBuf::from("/w/b.adoc"));
        assert!(store.record(&a, "Ada L", "ada@example.org", 10));
        assert!(!store.record(&a, "Ada", "ada@example.org", 20));
        assert!(store.record(&b, "Ada", "ada@example.org", 30));
        assert_eq!(store.for_paths(std::slice::from_ref(&a)), vec![ada()]);
        assert_eq!(store.for_paths(&[a, b]), vec![ada()]);
    }

    #[test]
    fn addresses_are_one_whatever_their_case() {
        let mut store = store();
        let (a, b) = (PathBuf::from("/w/a.adoc"), PathBuf::from("/w/b.adoc"));
        store.record(&a, "Ada", "Ada@Example.org", 10);
        assert!(!store.record(&a, "Ada", "ada@example.org", 11));
        store.record(&b, "Ada", "ada@example.org", 12);
        assert_eq!(store.for_paths(&[a, b]).len(), 1);
    }

    #[test]
    fn forgetting_keeps_later_edits() {
        let mut store = store();
        let a = PathBuf::from("/w/a.adoc");
        store.record(&a, "Ada", "ada@example.org", 10);
        store.record(&a, "Bea", "bea@example.org", 30);
        assert!(store.forget(&[(a.clone(), 20)]));
        assert_eq!(
            store.for_paths(std::slice::from_ref(&a)),
            vec![CoAuthor {
                name: "Bea".to_owned(),
                email: "bea@example.org".to_owned(),
            }]
        );
        assert!(!store.forget(&[(a, 20)]));
    }

    #[test]
    fn changes_are_laid_over_what_is_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(STATE_FILE);
        let on_disk = apply_changes(
            None,
            &[Change::Record {
                path: PathBuf::from("/w/a.adoc"),
                name: "Xan".to_owned(),
                email: "x@example.org".to_owned(),
                changed: 10,
            }],
        )
        .unwrap();
        std::fs::write(&file, &on_disk).unwrap();
        let mut store = CoAuthors::load(Some(file));
        let b = PathBuf::from("/w/b.adoc");
        store.record(&b, "Yan", "y@example.org", 15);
        let changes = store.take_changes().unwrap();
        assert!(store.take_changes().is_none());
        let both = apply_changes(Some(&on_disk), &changes).unwrap();
        let read = parse(&both).unwrap();
        assert_eq!(read.authors(Path::new("/w/a.adoc")).len(), 1);
        assert_eq!(read.authors(&b).len(), 1);
        let forgotten = apply_changes(
            Some(&both),
            &[Change::Forget {
                path: PathBuf::from("/w/a.adoc"),
                fixed: 20,
            }],
        )
        .unwrap();
        assert!(parse(&forgotten)
            .unwrap()
            .authors(Path::new("/w/a.adoc"))
            .is_empty());
    }

    #[test]
    fn an_unusable_state_file_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(STATE_FILE);
        let many = (0..=MAX_AUTHORS)
            .map(|n| format!(r#"{{"name":"P{n}","email":"p{n}@example.org","changed":1}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let unusable = [
            "{not json".to_owned(),
            r#"{"format":2,"files":{}}"#.to_owned(),
            format!(r#"{{"format":1,"files":{{"/w/a.adoc":[]}},"pad":"{}"}}"#, "x".repeat(MAX_STATE)),
            format!(r#"{{"format":1,"files":{{"/w/a.adoc":[{many}]}}}}"#),
            r#"{"format":1,"files":{"/w/a.adoc":[{"name":"Ada","email":"not an address","changed":1}]}}"#.to_owned(),
        ];
        for text in unusable {
            std::fs::write(&file, &text).unwrap();
            let mut store = CoAuthors::load(Some(file.clone()));
            assert!(store.unusable, "{}", &text[..text.len().min(60)]);
            let a = PathBuf::from("/w/a.adoc");
            assert!(store.record(&a, "Ada", "ada@example.org", 1));
            assert_eq!(store.for_paths(&[a]), vec![ada()]);
            assert!(store.take_changes().is_none());
            assert!(apply_changes(Some(&text), &[]).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_gone_file_has_a_real_path() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, directory.path().join("link")).unwrap();
        assert_eq!(
            real_path(&directory.path().join("link/gone.adoc")),
            std::fs::canonicalize(&real).unwrap().join("gone.adoc")
        );
        std::fs::write(real.join("here.adoc"), "").unwrap();
        assert_eq!(
            real_path(&directory.path().join("link/here.adoc")),
            std::fs::canonicalize(real.join("here.adoc")).unwrap()
        );
    }

    #[test]
    fn trailers() {
        let line = "Co-authored-by: Ada <ada@example.org>";
        assert_eq!(with_trailers("Fix\n\nBody", &[], &[]), "Fix\n\nBody");
        assert_eq!(
            with_trailers("Fix\n\nBody", &[ada()], &[]),
            format!("Fix\n\nBody\n\n{line}\n")
        );
        assert_eq!(
            with_trailers("Fix\n", &[ada()], &[]),
            format!("Fix\n\n{line}\n")
        );
        let already = "Fix\n\nCo-authored-by: Ada <ADA@example.org>\n";
        assert_eq!(with_trailers(already, &[ada()], &[]), already);
        assert_eq!(with_trailers("Fix", &[ada()], &["Ada@Example.org"]), "Fix");
        let odd = CoAuthor {
            name: "A<d>a\u{7}".to_owned(),
            email: "ada@example.org".to_owned(),
        };
        assert_eq!(
            with_trailers("Fix", &[odd, ada()], &[]),
            format!("Fix\n\n{line}\n")
        );
    }

    #[test]
    fn trailers_join_an_existing_block() {
        assert_eq!(
            with_trailers("Fix\n\nSigned-off-by: H <h@x>\n", &[ada()], &[]),
            "Fix\n\nSigned-off-by: H <h@x>\nCo-authored-by: Ada <ada@example.org>\n"
        );
        // A subject that looks like a trailer is not a block of them.
        assert_eq!(
            with_trailers("Docs: fix", &[ada()], &[]),
            "Docs: fix\n\nCo-authored-by: Ada <ada@example.org>\n"
        );
    }

    #[cfg(feature = "collab")]
    #[test]
    fn the_address_rule_is_the_protocols() {
        for email in [
            "ada@example.org",
            "a.b+c@d.e",
            "",
            "ada",
            "@x",
            "a@",
            "a@b@c",
            "a b@c",
            "<a@b>",
            "a,b@c",
            "a\"b@c",
            "a\\b@c",
            "a@b\n",
        ] {
            assert_eq!(
                is_commit_email(email),
                adoc_collab::protocol::commit_email(email).is_ok(),
                "{email:?}"
            );
        }
    }
}
