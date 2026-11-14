//! Which files the engine and its renderers may read on a front end's behalf. The desktop app,
//! the CLI and the language server read what their user can; the MCP server, driven by a model,
//! confines every read to its workspace roots, including the ones a page makes for itself
//! (includes, images, themes, fonts).

use std::{
    borrow::Cow,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use crate::normalize_path;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FileAccess {
    /// Any file the process can read.
    #[default]
    Any,
    /// Only files inside these roots, lexically and once symbolic links are followed. Made by
    /// [`FileAccess::within`].
    Within(ConfinedRoots),
}

/// The roots of a [`FileAccess::Within`], made absolute, normalised and resolved once, when the
/// access is made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfinedRoots(Vec<Root>);

#[derive(Clone, Debug)]
struct Root {
    /// The root made absolute against the current directory: what the system resolves.
    absolute: PathBuf,
    /// `absolute` normalised: what a path is compared with lexically.
    normal: PathBuf,
    /// `absolute` with its links followed, if it was there when the access was made.
    real: Option<PathBuf>,
}

impl PartialEq for Root {
    fn eq(&self, other: &Self) -> bool {
        self.absolute == other.absolute
    }
}

impl Eq for Root {}

impl Root {
    /// `None` for a root that cannot be made absolute, which holds nothing.
    fn new(root: PathBuf) -> Option<Self> {
        let absolute = std::path::absolute(root).ok()?;
        Some(Self {
            normal: normalize_path(&absolute),
            real: fs::canonicalize(&absolute).ok(),
            absolute,
        })
    }

    /// The root with its links followed: as resolved when the access was made, or now for a
    /// root that was not there then.
    fn real(&self) -> Option<Cow<'_, Path>> {
        match &self.real {
            Some(real) => Some(Cow::Borrowed(real)),
            None => fs::canonicalize(&self.absolute).ok().map(Cow::Owned),
        }
    }

    /// Whether `normal`, absolute and normalised, lies inside the root lexically: as spelt, or
    /// in other letters where the file system ignores case.
    fn holds(&self, normal: &Path) -> bool {
        normal.starts_with(&self.normal) || self.holds_in_other_letters(normal)
    }

    /// Whether `normal` begins with the root spelt in other letters, each folder so spelt being
    /// the root's own entry, as it is where the file system ignores case. Where case matters
    /// the other spelling is another entry, or nothing, and is refused, even a link to the root.
    fn holds_in_other_letters(&self, normal: &Path) -> bool {
        let mut path = normal.components();
        let mut given = PathBuf::new();
        let mut own = PathBuf::new();
        for component in self.normal.components() {
            let Some(other) = path.next() else {
                return false;
            };
            given.push(other);
            own.push(component);
            if other != component
                && !(same_letters(other.as_os_str(), component.as_os_str())
                    && same_entry(&given, &own))
            {
                return false;
            }
        }
        true
    }
}

/// Whether `a` and `b` differ, if at all, only in case.
fn same_letters(a: &OsStr, b: &OsStr) -> bool {
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => a == b,
    }
}

/// Whether `given` and `own` are one entry of the file system: both there, both links or
/// neither, and leading to the same place.
fn same_entry(given: &Path, own: &Path) -> bool {
    let (Ok(given_type), Ok(own_type)) = (fs::symlink_metadata(given), fs::symlink_metadata(own))
    else {
        return false;
    };
    given_type.file_type().is_symlink() == own_type.file_type().is_symlink()
        && matches!(
            (fs::canonicalize(given), fs::canonicalize(own)),
            (Ok(given), Ok(own)) if given == own
        )
}

impl FileAccess {
    /// Access confined to `roots`. Each is made absolute against the current directory,
    /// normalised and resolved now; a root that is not there yet is resolved whenever a path is
    /// checked, until it is.
    #[must_use]
    pub fn within(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::Within(ConfinedRoots(
            roots.into_iter().filter_map(Root::new).collect(),
        ))
    }

    /// Whether `path` may be read or written. Of a path that does not exist yet, the part that
    /// exists is checked with its links followed, and the rest lexically. A symbolic link is
    /// always part of what exists, so a link to nothing is refused rather than taken for a file
    /// to be made: writing to it would create its target, wherever that is. Where the file
    /// system ignores case, a path may spell a root in other letters.
    #[must_use]
    pub fn permits(&self, path: &Path) -> bool {
        let Self::Within(ConfinedRoots(roots)) = self else {
            return true;
        };
        // Absolute, so a relative path and a relative root are compared from the same place,
        // and a leading `..` is never resolved against nothing.
        let Ok(given) = std::path::absolute(path) else {
            return false;
        };
        let normal = normalize_path(&given);
        let mut holding = roots.iter().filter(|root| root.holds(&normal)).peekable();
        if holding.peek().is_none() {
            return false;
        }
        // The path as given, not its lexical form: the system resolves a `..` after a link
        // from where the link leads.
        let mut existing = given.as_path();
        // `symlink_metadata`, not `exists`, which follows links and so takes a link to
        // nothing for a path that is not there yet.
        while fs::symlink_metadata(existing).is_err() {
            match existing.parent() {
                Some(parent) => existing = parent,
                None => return false,
            }
        }
        let Ok(real) = fs::canonicalize(existing) else {
            return false;
        };
        // Any root holding the path both ways: a linked folder inside a root may be a root of
        // its own.
        holding.any(|root| {
            root.real()
                .is_some_and(|real_root| real.starts_with(real_root))
        })
    }

    /// Reads `path`, or refuses with `PermissionDenied` when it is not permitted.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.check(path)?;
        fs::read(path)
    }

    /// Reads `path` as text, or refuses with `PermissionDenied` when it is not permitted.
    pub fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.check(path)?;
        fs::read_to_string(path)
    }

    fn check(&self, path: &Path) -> io::Result<()> {
        if self.permits(path) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is outside the workspace roots", path.display()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_permits_everything() {
        assert!(FileAccess::Any.permits(Path::new("/etc/hosts")));
    }

    #[test]
    fn within_permits_only_the_roots() {
        let directory = tempfile_dir();
        let root = directory.join("docs");
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::write(directory.join("outside.adoc"), "outside").unwrap();
        fs::write(root.join("pages/page.adoc"), "inside").unwrap();
        let access = FileAccess::within(vec![root.clone()]);

        assert!(access.permits(&root.join("pages/page.adoc")));
        assert!(
            access.permits(&root.join("pages/new.adoc")),
            "a file not made yet"
        );
        assert!(!access.permits(&root.join("pages/../../outside.adoc")));
        assert!(!access.permits(&directory.join("outside.adoc")));
        assert!(!access.permits(Path::new("/etc/hosts")));
        assert_eq!(
            access
                .read_to_string(&root.join("pages/page.adoc"))
                .unwrap(),
            "inside"
        );
        let refused = access.read(&directory.join("outside.adoc")).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.join("outside.adoc"), root.join("link.adoc"))
                .unwrap();
            std::os::unix::fs::symlink(&directory, root.join("escape")).unwrap();
            fs::create_dir_all(directory.join("sub")).unwrap();
            std::os::unix::fs::symlink(directory.join("sub"), root.join("deep")).unwrap();
            assert!(
                !access.permits(&root.join("deep/../outside.adoc")),
                "`..` after a link climbs from where the link leads"
            );
            assert!(
                !access.permits(&root.join("deep/../new.pdf")),
                "a new file `..` from a link"
            );
            assert!(
                !access.permits(&root.join("link.adoc")),
                "a link to a file outside"
            );
            assert!(
                !access.permits(&root.join("escape/outside.adoc")),
                "through a directory link"
            );
            assert!(
                !access.permits(&root.join("escape/new.adoc")),
                "a new file through a link"
            );

            // A link to nothing is a link, not a file to be made: writing to it would create
            // its target, outside the roots.
            std::os::unix::fs::symlink(directory.join("missing.pdf"), root.join("out.pdf"))
                .unwrap();
            assert!(
                !access.permits(&root.join("out.pdf")),
                "a dangling link to outside"
            );
            std::os::unix::fs::symlink(directory.join("gone"), root.join("gone")).unwrap();
            assert!(
                !access.permits(&root.join("gone/new.adoc")),
                "a new file through a dangling folder link"
            );
            assert!(
                !directory.join("missing.pdf").exists() && !directory.join("gone").exists(),
                "checking creates nothing"
            );
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_relative_root_refuses_paths_climbing_out_of_it() {
        // Issue #528. Tests run in the crate's folder, which holds `src/lib.rs`; two folders
        // up is the repository, whose `src` is another folder.
        let src = FileAccess::within(vec![PathBuf::from("src")]);
        assert!(src.permits(Path::new("src/lib.rs")));
        assert!(src.permits(Path::new("./src/../src/lib.rs")));
        assert!(!src.permits(Path::new("../../src/lib.rs")));
        assert!(src.read_to_string(Path::new("../../src/lib.rs")).is_err());

        let here = FileAccess::within(vec![PathBuf::from(".")]);
        assert!(here.permits(Path::new("src/lib.rs")));
        assert!(!here.permits(Path::new("../../src/lib.rs")));
        assert!(!here.permits(Path::new("../outside.pdf")));
    }

    #[cfg(unix)]
    #[test]
    fn any_root_holding_the_path_both_ways_permits_it() {
        // Issue #533: a linked folder inside a root is a root of its own, whatever the order.
        let directory = tempfile_dir();
        let workspace = directory.join("ws");
        let elsewhere = directory.join("elsewhere");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("page.adoc"), "linked").unwrap();
        std::os::unix::fs::symlink(&elsewhere, workspace.join("linked")).unwrap();
        let page = workspace.join("linked/page.adoc");
        for roots in [
            vec![workspace.clone(), workspace.join("linked")],
            vec![workspace.join("linked"), workspace.clone()],
        ] {
            assert!(
                FileAccess::within(roots.clone()).permits(&page),
                "{roots:?}"
            );
        }
        assert!(!FileAccess::within(vec![workspace.clone()]).permits(&page));
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_path_cased_differently_is_permitted_where_the_file_system_ignores_case() {
        // Issue #533: on macOS and Windows `Docs/page.adoc` is the root `docs`'s page.
        let directory = tempfile_dir();
        let root = directory.join("docs");
        fs::create_dir_all(root.join("pages")).unwrap();
        fs::write(root.join("pages/page.adoc"), "inside").unwrap();
        fs::write(directory.join("outside.adoc"), "outside").unwrap();
        let ignores_case = fs::symlink_metadata(directory.join("DOCS")).is_ok();
        let access = FileAccess::within(vec![root.clone()]);

        assert_eq!(
            access.permits(&directory.join("DOCS/Pages/page.adoc")),
            ignores_case
        );
        assert_eq!(
            access.permits(&directory.join("Docs/pages/new.adoc")),
            ignores_case,
            "a file not made yet"
        );
        assert_eq!(
            access
                .read_to_string(&directory.join("Docs/pages/page.adoc"))
                .is_ok(),
            ignores_case
        );
        assert!(!access.permits(&directory.join("Docs/../outside.adoc")));
        assert!(!access.permits(&directory.join("Docs/pages/../../outside.adoc")));
        assert!(!access.permits(&directory.join("outside.adoc")));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&directory, root.join("escape")).unwrap();
            assert!(
                !access.permits(&directory.join("Docs/escape/outside.adoc")),
                "through a directory link"
            );
            assert!(
                !access.permits(&directory.join("Docs/ESCAPE/new.adoc")),
                "a new file through a link"
            );
            // A root that is itself a link, named in other letters.
            let elsewhere = directory.join("elsewhere");
            fs::create_dir_all(&elsewhere).unwrap();
            fs::write(elsewhere.join("page.adoc"), "linked").unwrap();
            std::os::unix::fs::symlink(&elsewhere, directory.join("linked")).unwrap();
            let linked = FileAccess::within(vec![directory.join("linked")]);
            assert!(linked.permits(&directory.join("linked/page.adoc")));
            assert_eq!(
                linked.permits(&directory.join("LINKED/page.adoc")),
                ignores_case
            );
            if !ignores_case {
                // Where case matters, `Docs` beside the root is another folder; a link to the
                // root by that name is outside it.
                std::os::unix::fs::symlink(&root, directory.join("Docs")).unwrap();
                assert!(!access.permits(&directory.join("Docs/pages/page.adoc")));
                assert!(!access.permits(&directory.join("Docs/pages/new.adoc")));
            }
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_root_made_after_the_access_is_resolved_when_asked() {
        let directory = tempfile_dir();
        let root = directory.join("docs");
        let access = FileAccess::within(vec![root.clone()]);
        assert!(!access.permits(&root.join("page.adoc")), "no root yet");

        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("page.adoc"), "inside").unwrap();
        assert!(access.permits(&root.join("page.adoc")));
        assert!(!access.permits(&root.join("../outside.adoc")));
        fs::remove_dir_all(&directory).unwrap();
    }

    /// A fresh directory under the system's temporary directory; `adoc-core` has no dependencies,
    /// so not even `tempfile`.
    fn tempfile_dir() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "adoc-core-access-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        // Canonical, so the lexical check of the roots agrees with the followed one on systems
        // whose temporary directory is itself a link (macOS's /var).
        fs::canonicalize(&directory).unwrap()
    }
}
