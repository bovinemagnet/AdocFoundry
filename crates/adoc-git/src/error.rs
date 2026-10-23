use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum GitError {
    /// An error from `gix`: a repository that cannot be opened or read.
    Git(String),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// A path outside the repository's working tree.
    OutsideWorkdir(PathBuf),
}

impl GitError {
    pub(crate) fn git(error: impl fmt::Display) -> Self {
        Self::Git(error.to_string())
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(message) => write!(formatter, "git: {message}"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::OutsideWorkdir(path) => write!(
                formatter,
                "{} is not in the repository's working tree",
                path.display()
            ),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
