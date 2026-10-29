use std::{fmt, io, path::PathBuf};

use crate::{CommandId, DocumentId};

#[derive(Debug)]
pub enum EngineError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    UnknownDocument(DocumentId),
    UnknownCommand(CommandId),
    InvalidArguments {
        command: CommandId,
        expected: &'static str,
    },
    InvalidEdit(String),
    InvalidRequest(String),
    NotFound(String),
    Settings(String),
    /// A visual edit that cannot be made without changing text the user did not edit.
    Refused(String),
    /// The file changed on disk since the engine last read or wrote it; saving would lose that
    /// change. `document.reload` takes the file's version, `document.overwrite` the buffer's.
    SaveConflict {
        path: PathBuf,
    },
    /// A file a preview edits changed after the preview was made; it must be made again.
    PreviewStale {
        path: PathBuf,
    },
    /// A change spanning several documents is undone and redone only as a whole, and `path`,
    /// one of them, was edited after it (or, with `redo`, after its undo).
    GroupBlocked {
        path: PathBuf,
        redo: bool,
    },
}

impl EngineError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::UnknownDocument(id) => write!(formatter, "no open document with id {}", id.get()),
            Self::UnknownCommand(id) => write!(formatter, "no command `{id}`"),
            Self::InvalidArguments { command, expected } => {
                write!(formatter, "`{command}` expects {expected}")
            }
            Self::InvalidEdit(message) => write!(formatter, "invalid edit: {message}"),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::NotFound(message) => formatter.write_str(message),
            Self::Settings(message) => write!(formatter, "settings: {message}"),
            Self::Refused(message) => formatter.write_str(message),
            Self::SaveConflict { path } => write!(
                formatter,
                "{} changed on disk since it was opened or saved; reload it or overwrite it",
                path.display()
            ),
            Self::PreviewStale { path } => write!(
                formatter,
                "{} changed after the preview was made; make the preview again",
                path.display()
            ),
            Self::GroupBlocked { path, redo: false } => write!(
                formatter,
                "{} was edited after this change to several files; undo its later edits first",
                path.display()
            ),
            Self::GroupBlocked { path, redo: true } => write!(
                formatter,
                "{} was edited after this change to several files was undone, so it cannot be redone from here",
                path.display()
            ),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
