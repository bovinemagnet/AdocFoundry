//! Every path a tool is given must lie inside a workspace root, lexically and once links are
//! followed, or the call is refused.

use std::path::{Path, PathBuf};

use adoc_engine::FileAccess;

/// `path`, absolute or relative to the first root, normalised; an error when it lies outside
/// every root, or reaches outside through a symbolic link.
pub fn confine(roots: &[PathBuf], path: &str) -> Result<PathBuf, String> {
    let given = Path::new(path);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        roots
            .first()
            .ok_or_else(|| "there is no workspace root".to_owned())?
            .join(given)
    };
    let normal = adoc_engine::normalize_path(&joined);
    // The engine checks its own reads the same way, so a tool's path and what the page it
    // names reads are held to one rule.
    if !FileAccess::Within(roots.to_vec()).permits(&normal) {
        return Err(format!("{path} is outside the workspace roots"));
    }
    Ok(normal)
}

#[cfg(test)]
mod tests {
    use super::confine;

    #[test]
    fn paths_stay_inside_the_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = adoc_engine::normalize_path(&directory.path().join("docs"));
        std::fs::create_dir_all(root.join("pages")).unwrap();
        let roots = vec![root.clone()];

        assert_eq!(
            confine(&roots, "pages/new.adoc").unwrap(),
            root.join("pages/new.adoc")
        );
        assert!(confine(&roots, "../outside.adoc").is_err());
        assert!(confine(&roots, "/etc/passwd").is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.path(), root.join("escape")).unwrap();
            assert!(confine(&roots, "escape/secret.adoc").is_err());
        }
    }
}
