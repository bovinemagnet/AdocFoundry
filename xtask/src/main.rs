//! Workspace automation: `cargo xtask <task>`.

use std::{path::PathBuf, process::ExitCode};

mod layers;

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("layers") => run_layers(),
        _ => {
            eprintln!("usage: cargo xtask <task>\n\ntasks:\n  layers    check the crate dependency ladder");
            ExitCode::FAILURE
        }
    }
}

fn run_layers() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    match layers::run(&root) {
        Ok(violations) if violations.is_empty() => {
            println!("layers: ok");
            ExitCode::SUCCESS
        }
        Ok(violations) => {
            for violation in &violations {
                eprintln!("layers: {violation}");
            }
            eprintln!("layers: {} violation(s)", violations.len());
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("layers: {error}");
            ExitCode::FAILURE
        }
    }
}
