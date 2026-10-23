//! `adocfoundry-cli`: the headless front end over `adoc-engine`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("adocfoundry-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: adocfoundry-cli --version");
            ExitCode::from(2)
        }
    }
}
