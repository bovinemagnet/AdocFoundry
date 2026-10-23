use std::{fmt, io};

#[derive(Debug)]
pub enum ServerError {
    UnknownArgument(String),
    Protocol(lsp_server::ProtocolError),
    Json(serde_json::Error),
    Io(io::Error),
    ChannelClosed,
    InvalidChange(String),
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument `{argument}`"),
            Self::Protocol(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::ChannelClosed => formatter.write_str("LSP connection closed unexpectedly"),
            Self::InvalidChange(message) => write!(formatter, "invalid document change: {message}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::UnknownArgument(_) | Self::ChannelClosed | Self::InvalidChange(_) => None,
        }
    }
}

impl From<lsp_server::ProtocolError> for ServerError {
    fn from(error: lsp_server::ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<serde_json::Error> for ServerError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run(arguments: impl IntoIterator<Item = String>) -> Result<(), ServerError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => {
            print_help();
            Ok(())
        }
        [arg] if arg == "--help" || arg == "-h" => {
            print_help();
            Ok(())
        }
        [arg] if arg == "--version" || arg == "-V" => {
            println!("adoc-ls {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [arg] if arg == "--stdio" => run_stdio(),
        [argument, ..] => Err(ServerError::UnknownArgument(argument.clone())),
    }
}

fn run_stdio() -> Result<(), ServerError> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let server_result = crate::protocol::run_connection(&connection);
    drop(connection);
    let io_result = io_threads.join();
    server_result?;
    io_result?;
    Ok(())
}

fn print_help() {
    println!(
        "adoc-ls {}\n\nUSAGE:\n    adoc-ls --stdio\n    adoc-ls --version",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::{run, ServerError};

    #[test]
    fn rejects_unknown_arguments() {
        assert!(matches!(
            run(["--unknown".to_owned()]),
            Err(ServerError::UnknownArgument(argument)) if argument == "--unknown"
        ));
    }
}
