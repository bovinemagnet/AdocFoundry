//! Every path a tool is given must lie inside a workspace root, lexically and once links are
//! followed, or the call is refused.

use std::{
    fs,
    path::{Path, PathBuf},
};

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
    let refused = || format!("{path} is outside the workspace roots");
    let root = roots
        .iter()
        .find(|root| normal.starts_with(root))
        .ok_or_else(refused)?;
    // Links are followed only for the part that exists; the rest is checked lexically above.
    let mut existing = normal.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(refused)?;
    }
    let real = fs::canonicalize(existing).map_err(|error| format!("{path}: {error}"))?;
    let real_root = fs::canonicalize(root).map_err(|error| format!("{path}: {error}"))?;
    if !real.starts_with(&real_root) {
        return Err(refused());
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
