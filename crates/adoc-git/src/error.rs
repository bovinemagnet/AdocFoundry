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
    /// Committing needs `user.name` and `user.email`, and the configuration lacks one.
    NoIdentity,
    /// The index holds nothing that `HEAD` does not.
    NothingToCommit,
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
            Self::NoIdentity => formatter.write_str(
                "set user.name and user.email in your Git configuration to commit \
                 (git config --global user.name \"Your Name\"; git config --global user.email you@example.com)",
            ),
            Self::NothingToCommit => formatter.write_str("nothing is staged to commit"),
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
