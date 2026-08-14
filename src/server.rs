use std::fmt;

#[derive(Debug, Eq, PartialEq)]
pub enum ServerError {
    UnsupportedTransport,
    UnknownArgument(String),
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTransport => formatter.write_str(
                "stdio transport is not implemented yet; use --version to verify the binary",
            ),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument `{argument}`"),
        }
    }
}

impl std::error::Error for ServerError {}

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
        [arg] if arg == "--stdio" => Err(ServerError::UnsupportedTransport),
        [argument, ..] => Err(ServerError::UnknownArgument(argument.clone())),
    }
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
    fn rejects_stdio_until_protocol_transport_is_connected() {
        assert_eq!(
            run(["--stdio".to_owned()]),
            Err(ServerError::UnsupportedTransport)
        );
    }
}
