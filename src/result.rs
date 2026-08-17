use std::{fmt, io};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderOutput {
    pub html: String,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum RenderError {
    ExecutableNotFound(String),
    ProcessFailed { status: Option<i32>, stderr: String },
    Io(io::Error),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExecutableNotFound(executable) => {
                write!(
                    formatter,
                    "AsciiDoc preview requires Asciidoctor; executable `{executable}` was not found"
                )
            }
            Self::ProcessFailed { status, stderr } => {
                write!(
                    formatter,
                    "AsciiDoc renderer failed with status {status:?}: {stderr}"
                )
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::ExecutableNotFound(_) | Self::ProcessFailed { .. } => None,
        }
    }
}
