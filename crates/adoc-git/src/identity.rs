//! The user's own identity from their global Git configuration, with no repository: who writes a
//! comment on a page outside any repository, or before one is found.

use std::path::PathBuf;

/// `user.name` and, when set, `user.email` from the user's global and system Git
/// configuration, as Git reads them outside a repository. `None` without a name.
#[must_use]
pub fn global_identity() -> Option<(String, Option<String>)> {
    gix::config::File::from_globals()
        .ok()
        .and_then(|file| identity_of(&file))
}

/// `user.name` and `user.email` from the configuration files given, later files winning: what
/// [`global_identity`] reads, from files a test chooses. Unreadable files are skipped.
#[must_use]
pub fn identity_in(files: &[PathBuf]) -> Option<(String, Option<String>)> {
    let mut config = gix::config::File::default();
    for path in files {
        if let Ok(file) =
            gix::config::File::from_path_no_includes(path.clone(), gix::config::Source::User)
        {
            // A file that cannot be added is skipped, as one that cannot be read is.
            let _ = config.append(file);
        }
    }
    identity_of(&config)
}

fn identity_of(config: &gix::config::File) -> Option<(String, Option<String>)> {
    let value = |key: &str| {
        config
            .string(key)
            .map(|value| value.to_string().trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    Some((value("user.name")?, value("user.email")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_read_from_the_files_given() {
        let directory = tempfile::tempdir().unwrap();
        let both = directory.path().join("both");
        std::fs::write(&both, "[user]\n\tname = Ada Lovelace\n\temail = ada@example.org\n").unwrap();
        let name_only = directory.path().join("name");
        std::fs::write(&name_only, "[user]\n\tname = Grace Hopper\n").unwrap();
        let empty = directory.path().join("empty");
        std::fs::write(&empty, "[core]\n\tbare = false\n").unwrap();

        assert_eq!(
            identity_in(std::slice::from_ref(&both)),
            Some((
                "Ada Lovelace".to_owned(),
                Some("ada@example.org".to_owned())
            ))
        );
        assert_eq!(
            identity_in(std::slice::from_ref(&name_only)),
            Some(("Grace Hopper".to_owned(), None))
        );
        assert_eq!(identity_in(&[empty]), None);
        assert_eq!(identity_in(&[directory.path().join("missing")]), None);
        // A later file wins, as Git's later scopes do.
        assert_eq!(
            identity_in(&[both, name_only]).map(|(name, _)| name),
            Some("Grace Hopper".to_owned())
        );
    }
}
