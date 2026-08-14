use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub const DEFAULT_IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".zed",
    "node_modules",
    "build",
    "target",
    "dist",
    ".idea",
    ".gradle",
];

#[must_use]
pub fn is_asciidoc_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "adoc" | "asciidoc" | "ad"))
}

pub(crate) fn collect_asciidoc_files(path: &Path, output: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_file() {
        if is_asciidoc_path(path) {
            output.push(normalize_path(path));
        }
        return Ok(());
    }

    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let entry_path = entry.path();
        if file_type.is_dir() {
            let ignored = entry
                .file_name()
                .to_str()
                .is_some_and(|name| DEFAULT_IGNORED_DIRECTORIES.contains(&name));
            if !ignored {
                collect_asciidoc_files(&entry_path, output)?;
            }
        } else if file_type.is_file() && is_asciidoc_path(&entry_path) {
            output.push(normalize_path(&entry_path));
        }
    }
    Ok(())
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_asciidoc_path, normalize_path};

    #[test]
    fn recognizes_supported_suffixes() {
        assert!(is_asciidoc_path(Path::new("guide.adoc")));
        assert!(is_asciidoc_path(Path::new("guide.asciidoc")));
        assert!(is_asciidoc_path(Path::new("guide.ad")));
        assert!(!is_asciidoc_path(Path::new("guide.md")));
    }

    #[test]
    fn normalizes_dot_components() {
        assert_eq!(
            normalize_path(Path::new("docs/./pages/../index.adoc")),
            Path::new("docs/index.adoc")
        );
    }
}
