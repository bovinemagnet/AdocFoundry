//! Which files the engine and its renderers may read on a front end's behalf. The desktop app,
//! the CLI and the language server read what their user can; the MCP server, driven by a model,
//! confines every read to its workspace roots, including the ones a page makes for itself
//! (includes, images, themes, fonts).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::normalize_path;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FileAccess {
    /// Any file the process can read.
    #[default]
    Any,
    /// Only files inside these roots, lexically and once symbolic links are followed.
    Within(Vec<PathBuf>),
}

impl FileAccess {
    /// Whether `path` may be read or written. Of a path that does not exist yet, the part that
    /// exists is checked with its links followed, and the rest lexically. A symbolic link is
    /// always part of what exists, so a link to nothing is refused rather than taken for a file
    /// to be made: writing to it would create its target, wherever that is.
    #[must_use]
    pub fn permits(&self, path: &Path) -> bool {
        let Self::Within(roots) = self else {
            return true;
        };
        // Absolute, so a relative path and a relative root are compared from the same place,
        // and a leading `..` is never resolved against nothing.
        let Ok(given) = std::path::absolute(path) else {
            return false;
        };
        let normal = normalize_path(&given);
        let mut holding = roots
            .iter()
            .filter(|root| {
                std::path::absolute(root)
                    .is_ok_and(|root| normal.starts_with(normalize_path(&root)))
            })
            .peekable();
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
        holding
            .any(|root| fs::canonicalize(root).is_ok_and(|real_root| real.starts_with(real_root)))
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
        let access = FileAccess::Within(vec![root.clone()]);

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
        let src = FileAccess::Within(vec![PathBuf::from("src")]);
        assert!(src.permits(Path::new("src/lib.rs")));
        assert!(src.permits(Path::new("./src/../src/lib.rs")));
        assert!(!src.permits(Path::new("../../src/lib.rs")));
        assert!(src.read_to_string(Path::new("../../src/lib.rs")).is_err());

        let here = FileAccess::Within(vec![PathBuf::from(".")]);
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
                FileAccess::Within(roots.clone()).permits(&page),
                "{roots:?}"
            );
        }
        assert!(!FileAccess::Within(vec![workspace.clone()]).permits(&page));
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
