//! Workspace automation: `cargo xtask <task>`.

use std::{path::PathBuf, process::ExitCode};

mod bump;
mod layers;
// Shared with the app's build script, which uses what this binary does not.
#[allow(dead_code)]
mod notices;
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
        Some("notices") => run_notices(std::env::args().nth(2)),
        Some("bump") => run_bump(std::env::args().nth(2)),
        _ => {
            eprintln!(
                "usage: cargo xtask <task>\n\ntasks:\n  bump       write a release's version into Cargo.toml, the docs and the changelog\n  layers     check the crate dependency ladder\n  notices    write the third-party notices to a file, or print them\n  package    release build and archive for this platform in dist/"
            );
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask notices [<file>]`: the notices file the packages ship, written to `file` or
/// printed.
fn run_notices(output: Option<String>) -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let text = match notices::collect(&root, &cargo) {
        Ok(notices) => notices.to_text(),
        Err(error) => {
            eprintln!("notices: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(output) = output else {
        print!("{text}");
        return ExitCode::SUCCESS;
    };
    match std::fs::write(&output, text) {
        Ok(()) => {
            println!("notices: {output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("notices: {output}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask bump <version>`: the version every file the release checks gives.
fn run_bump(version: Option<String>) -> ExitCode {
    let Some(version) = version else {
        eprintln!("usage: cargo xtask bump <version>");
        return ExitCode::FAILURE;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    match bump::run(&root, &version) {
        Ok(()) => {
            println!("bump: Cargo.toml, src/docs/antora.yml and CHANGELOG.md give {version}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("bump: {error}");
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
