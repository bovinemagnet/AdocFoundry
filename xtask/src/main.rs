//! Workspace automation: `cargo xtask <task>`.

use std::{path::PathBuf, process::ExitCode};

mod layers;
mod package;

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("layers") => run_layers(),
        Some("package") => {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
            match package::run_package(&root) {
                Ok(archive) => {
                    println!("package: {}", archive.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("package: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <task>\n\ntasks:\n  layers     check the crate dependency ladder\n  package    release build and archive for this platform in dist/"
            );
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
