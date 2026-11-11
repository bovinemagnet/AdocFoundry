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
    /// `path` is under the named `filter`, which staging does not run.
    FilterNotRun {
        path: PathBuf,
        filter: String,
    },
    /// Git has stopped part-way through the named operation (`a merge`, `a rebase`,
    /// `a cherry-pick`, `a revert`, `git am`), which committing would have to conclude.
    InProgress(&'static str),
    /// The index holds unresolved conflicts at these paths.
    Unmerged(Vec<PathBuf>),
    /// `path` is a folder in `HEAD`, which unstaging takes only a file at a time.
    Folder(PathBuf),
    /// `path` has a component Git never stores, such as `.git` in any of its spellings.
    Unstorable {
        path: PathBuf,
        reason: String,
    },
    /// `spec` names no commit in the repository.
    UnknownRevision {
        spec: String,
        message: String,
    },
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
            Self::FilterNotRun { path, filter } => write!(
                formatter,
                "{} uses Git's `{filter}` filter, which AdocFoundry does not run: stage it with git add",
                path.display()
            ),
            Self::InProgress(operation) => write!(
                formatter,
                "a {operation} is in progress: finish or abort it with Git before committing here"
            ),
            Self::Unmerged(paths) => {
                formatter.write_str("resolve the conflicts in ")?;
                for (n, path) in paths.iter().enumerate() {
                    let separator = if n == 0 { "" } else { ", " };
                    write!(formatter, "{separator}{}", path.display())?;
                }
                formatter.write_str(" and stage them before committing")
            }
            Self::Folder(path) => write!(
                formatter,
                "{} is a folder: unstage the files in it one at a time",
                path.display()
            ),
            Self::Unstorable { path, reason } => {
                write!(formatter, "{} cannot be stored in Git: {reason}", path.display())
            }
            Self::UnknownRevision { spec, message } => {
                write!(formatter, "`{spec}` names no commit here: {message}")
            }
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
