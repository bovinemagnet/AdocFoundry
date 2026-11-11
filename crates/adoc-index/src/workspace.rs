use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub use adoc_core::{normalize_path, DEFAULT_IGNORED_DIRECTORIES};

#[must_use]
pub fn is_asciidoc_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "adoc" | "asciidoc" | "ad"))
}

/// Collects the AsciiDoc files beneath `path`, a folder beneath `root`, never entering a link
/// or a folder `ignore` leaves out ([`adoc_core::is_ignored_beneath`]). `on_directory` sees each folder entered before it is read; one that cannot be read is
/// added to `unreadable` and left out.
pub(crate) fn collect_asciidoc_files(
    root: &Path,
    path: &Path,
    ignore: &[impl AsRef<str>],
    on_directory: &mut dyn FnMut(&Path),
    output: &mut Vec<PathBuf>,
    unreadable: &mut Vec<PathBuf>,
) {
    if path.is_file() {
        if is_asciidoc_path(path) {
            output.push(normalize_path(path));
        }
        return;
    }

    on_directory(path);
    adoc_core::probe::hit(adoc_core::probe::Probe::IndexWalk);
    let Ok(mut entries) = fs::read_dir(path).and_then(Iterator::collect::<io::Result<Vec<_>>>)
    else {
        unreadable.push(normalize_path(path));
        return;
    };
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let entry_path = entry.path();
        if file_type.is_dir() {
            // The folders above were entered, so only a folder named in `ignore` needs the rule.
            let ignored = entry
                .file_name()
                .to_str()
                .is_some_and(|name| ignore.iter().any(|ignored| ignored.as_ref() == name))
                && entry_path.strip_prefix(root).is_ok_and(|beneath| {
                    adoc_core::is_ignored_beneath(beneath, ignore, |folder| {
                        root.join(folder).join("antora.yml").is_file()
                    })
                });
            if !ignored {
                collect_asciidoc_files(root, &entry_path, ignore, on_directory, output, unreadable);
            }
        } else if file_type.is_file() && is_asciidoc_path(&entry_path) {
            output.push(normalize_path(&entry_path));
        }
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_asciidoc_path;

    #[test]
    fn recognizes_supported_suffixes() {
        assert!(is_asciidoc_path(Path::new("guide.adoc")));
        assert!(is_asciidoc_path(Path::new("guide.asciidoc")));
        assert!(is_asciidoc_path(Path::new("guide.ad")));
        assert!(!is_asciidoc_path(Path::new("guide.md")));
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
