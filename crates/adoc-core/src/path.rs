//! Paths resolved lexically, without touching the file system.

use std::path::{Component, Path, PathBuf};

/// `path` with `.` and `..` resolved lexically, without touching the file system, so unsaved
/// files work. A relative path keeps every leading `..` it cannot resolve, and `..` above the
/// root is the root.
#[must_use]
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                // `a/..` is nothing.
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                // Above the root is the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                // A relative path keeps every leading `..`.
                _ => normalized.push(component.as_os_str()),
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    // `.` and `docs/..` name the current directory; an empty path names nothing.
    if normalized.as_os_str().is_empty() && !path.as_os_str().is_empty() {
        normalized.push(Component::CurDir.as_os_str());
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::normalize_path;

    #[test]
    fn normalizes_dot_components() {
        assert_eq!(
            normalize_path(Path::new("docs/./pages/../index.adoc")),
            Path::new("docs/index.adoc")
        );
    }

    #[test]
    fn keeps_leading_parent_components() {
        for (path, expected) in [
            ("../a", "../a"),
            ("../../a", "../../a"),
            ("../../../a/b", "../../../a/b"),
            ("a/../../b", "../b"),
            ("../a/../b", "../b"),
            // Above the root is the root.
            ("/..", "/"),
            ("/a/../../b", "/b"),
        ] {
            assert_eq!(
                normalize_path(Path::new(path)),
                Path::new(expected),
                "{path}"
            );
        }
    }

    #[test]
    fn keeps_the_current_directory() {
        for path in [".", "./", "docs/..", "./docs/./.."] {
            assert_eq!(normalize_path(Path::new(path)), Path::new("."), "{path}");
        }
        assert_eq!(normalize_path(Path::new("")), Path::new(""));
    }
}
