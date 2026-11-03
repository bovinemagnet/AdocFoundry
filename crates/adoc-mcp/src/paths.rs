//! Every path a tool is given must lie inside a workspace root, lexically and once links are
//! followed, or the call is refused.

use std::path::{Path, PathBuf};

use adoc_engine::FileAccess;

use crate::responses::{ErrorCode, Failure};

/// `path`, absolute or relative to the first root, normalised; an error when it lies outside
/// every root, or reaches outside through a symbolic link (`outside_workspace`).
pub fn confine(roots: &[PathBuf], path: &str) -> Result<PathBuf, Failure> {
    let given = Path::new(path);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        roots
            .first()
            .ok_or_else(|| Failure::internal("there is no workspace root"))?
            .join(given)
    };
    let normal = adoc_engine::normalize_path(&joined);
    // The engine checks its own reads the same way, so a tool's path and what the page it
    // names reads are held to one rule.
    if !FileAccess::Within(roots.to_vec()).permits(&normal) {
        return Err(Failure::new(
            ErrorCode::OutsideWorkspace,
            format!("{path} is outside the workspace roots"),
        ));
    }
    Ok(normal)
}

#[cfg(test)]
mod tests {
    use super::confine;
    use crate::responses::ErrorCode;

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
        assert_eq!(
            confine(&roots, "../outside.adoc").unwrap_err().code,
            ErrorCode::OutsideWorkspace
        );
        assert!(confine(&roots, "/etc/passwd").is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(directory.path(), root.join("escape")).unwrap();
            assert!(confine(&roots, "escape/secret.adoc").is_err());
        }
    }
}
