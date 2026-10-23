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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_directory: bool,
}

/// One shallow directory listing for path completion, sorted by name.
///
/// Completion needs targets the index does not hold — `query.sql`, `diagram.png` — so this
/// is the one filesystem read on the completion path. An unreadable directory lists
/// nothing rather than failing: completion never reports an error.
#[must_use]
pub fn list_directory(directory: &Path) -> Vec<DirectoryEntry> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut listed = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let is_directory = file_type.is_dir();
        if is_directory && DEFAULT_IGNORED_DIRECTORIES.contains(&name.as_str()) {
            continue;
        }
        listed.push(DirectoryEntry { name, is_directory });
    }
    listed.sort_by(|left, right| left.name.cmp(&right.name));
    listed
}

pub fn normalize_path(path: &Path) -> PathBuf {
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

    #[test]
    fn lists_a_directory_for_completion() {
        use super::list_directory;

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component/modules/ROOT");

        let names: Vec<_> = list_directory(&root)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "attachments".to_owned(),
                "examples".to_owned(),
                "images".to_owned(),
                "nav.adoc".to_owned(),
                "pages".to_owned(),
                "partials".to_owned(),
            ]
        );
        assert!(list_directory(&root.join("absent")).is_empty());
    }

    #[test]
    fn lists_non_asciidoc_files_the_index_does_not_hold() {
        use super::list_directory;

        let examples = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component/modules/ROOT/examples");

        let names: Vec<_> = list_directory(&examples)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(names, vec!["sample.json".to_owned()]);
    }
}
