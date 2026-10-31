//! `adoc-mcp`: a Model Context Protocol server over stdio, or Streamable HTTP on a loopback
//! address, so assistants and tools can read, check, search and render an Antora workspace
//! through the engine, and edit it when allowed.
//!
//! ```text
//! adoc-mcp <root>... [--allow-write] [--http <addr>]
//! adoc-mcp --version
//! ```

mod http;
mod paths;
mod server;

use std::{path::PathBuf, process::ExitCode, time::Duration};

use adoc_engine::{Engine, FileAccess, Settings};
use rmcp::ServiceExt as _;

const USAGE: &str =
    "usage: adoc-mcp <root>... [--allow-write] [--http <addr>]\n       adoc-mcp --version";

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
    let mut listen = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
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
            "--http" => {
                let address = arguments
                    .next()
                    .ok_or_else(|| format!("--http needs an address\n{USAGE}"))?;
                listen = Some(http::loopback(address)?);
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option `{option}`\n{USAGE}"));
            }
            root => roots.push(adoc_engine::normalize_path(&absolute(root)?)),
        }
    }
    if roots.is_empty() {
        return Err(USAGE.to_owned());
    }
    let token = match listen {
        Some(_) => {
            let given = std::env::var(http::TOKEN_VARIABLE).ok();
            let generated = given.as_deref().is_none_or(str::is_empty);
            let token = http::token(given)?;
            // Shown once, so the user can hand it to a client; a token they set is theirs.
            if generated {
                eprintln!("adoc-mcp: token {token}");
            }
            Some(token)
        }
        None => None,
    };

    let settings = Settings::load(Settings::default_user_file().as_deref(), None)
        .map_err(|error| error.to_string())?;
    let mut engine = Engine::new(settings);
    // Every read the engine makes for a tool stays inside the roots, as the tools' own paths
    // do: includes, images, themes and fonts included.
    engine.set_file_access(FileAccess::Within(roots.clone()));
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
            tokio::spawn(server.clone().pump_while_idle());
            if let (Some(address), Some(token)) = (listen, token) {
                return http::serve(server, address, token).await;
            }
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
