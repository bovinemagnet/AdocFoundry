//! `adoc-mcp`: a Model Context Protocol server over stdio, so assistants and tools can read,
//! check, search and render an Antora workspace through the engine, and edit it when allowed.
//!
//! ```text
//! adoc-mcp <root>... [--allow-write]
//! adoc-mcp --version
//! ```

mod paths;
mod server;

use std::{path::PathBuf, process::ExitCode, time::Duration};

use adoc_engine::{Engine, Settings};
use rmcp::ServiceExt as _;

const USAGE: &str = "usage: adoc-mcp <root>... [--allow-write]\n       adoc-mcp --version";

/// How long indexing the workspace may take before the server starts anyway.
const INDEX_TIMEOUT: Duration = Duration::from_secs(120);

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("adoc-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    let mut roots = Vec::new();
    let mut allow_write = false;
    for argument in arguments {
        match argument.as_str() {
            "--version" | "-V" => {
                println!("adoc-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--allow-write" => allow_write = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown option `{option}`\n{USAGE}"));
            }
            root => roots.push(adoc_engine::normalize_path(&absolute(root)?)),
        }
    }
    if roots.is_empty() {
        return Err(USAGE.to_owned());
    }

    let settings = Settings::load(Settings::default_user_file().as_deref(), None)
        .map_err(|error| error.to_string())?;
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(roots.clone())
        .map_err(|error| error.to_string())?;
    engine.wait_for_workers(INDEX_TIMEOUT);

    let server = server::Server::new(engine, roots, allow_write);
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())?
        .block_on(async move {
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|error| error.to_string())?;
            service.waiting().await.map_err(|error| error.to_string())?;
            Ok(())
        })
}

/// `root` made absolute against the current directory, without resolving links.
fn absolute(root: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(root);
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| error.to_string())
}
