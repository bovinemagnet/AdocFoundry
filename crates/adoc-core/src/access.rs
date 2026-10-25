//! Which files the engine and its renderers may read on a front end's behalf. The desktop app,
//! the CLI and the language server read what their user can; the MCP server, driven by a model,
//! confines every read to its workspace roots, including the ones a page makes for itself
//! (includes, images, themes, fonts).

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FileAccess {
    /// Any file the process can read.
    #[default]
    Any,
    /// Only files inside these roots, lexically and once symbolic links are followed.
    Within(Vec<PathBuf>),
}

impl FileAccess {
    /// Whether `path` may be read. Of a path that does not exist yet, the part that exists is
    /// checked with its links followed, and the rest lexically.
    #[must_use]
    pub fn permits(&self, path: &Path) -> bool {
        let Self::Within(roots) = self else {
            return true;
        };
        let normal = lexical(path);
        let Some(root) = roots.iter().find(|root| normal.starts_with(lexical(root))) else {
            return false;
        };
        let mut existing = normal.as_path();
        while !existing.exists() {
            match existing.parent() {
                Some(parent) => existing = parent,
                None => return false,
            }
        }
        match (fs::canonicalize(existing), fs::canonicalize(root)) {
            (Ok(real), Ok(real_root)) => real.starts_with(real_root),
            _ => false,
        }
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

/// `path` with `.` and `..` resolved lexically, as `adoc_index::normalize_path` does.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
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
        }
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
