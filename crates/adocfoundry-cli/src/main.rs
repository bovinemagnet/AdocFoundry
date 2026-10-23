//! `adocfoundry-cli`: the headless front end over `adoc-engine`.
//!
//! ```text
//! adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
//! adocfoundry-cli symbols <root> <query>
//! adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
//! adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
//! adocfoundry-cli --version
//! ```

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use adoc_core::{Diagnostic, DiagnosticSeverity, LineIndex};
use adoc_engine::{commands::ids, CommandArgs, CommandOutcome, Engine, EngineError, Settings};
use serde::Serialize;

const USAGE: &str = "usage:
  adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
  adocfoundry-cli symbols <root> <query>
  adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
  adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
  adocfoundry-cli --version";

const INDEX_TIMEOUT: Duration = Duration::from_secs(300);

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("adocfoundry-cli: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: &[String]) -> Result<ExitCode, String> {
    let Some((command, rest)) = arguments.split_first() else {
        return Err(USAGE.to_owned());
    };
    match command.as_str() {
        "--version" | "-V" => {
            println!("adocfoundry-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "check" => check(rest),
        "symbols" => symbols(rest),
        "follow" => follow(rest),
        "render" => render(rest),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

/// Positional arguments and `(name, value)` options.
type Parsed<'a> = (Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// Splits `--name value` options from positional arguments.
fn parse_options<'a>(arguments: &'a [String], names: &[&str]) -> Result<Parsed<'a>, String> {
    let mut positional = Vec::new();
    let mut options = Vec::new();
    let mut iter = arguments.iter();
    while let Some(argument) = iter.next() {
        if let Some(name) = argument.strip_prefix("--") {
            if !names.contains(&name) {
                return Err(format!("unknown option `{argument}`\n{USAGE}"));
            }
            let value = iter
                .next()
                .ok_or_else(|| format!("`{argument}` needs a value"))?;
            options.push((name, value.as_str()));
        } else {
            positional.push(argument.as_str());
        }
    }
    Ok((positional, options))
}

fn option<'a>(options: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .rev()
        .find(|(option, _)| *option == name)
        .map(|(_, value)| *value)
}

fn open_engine(root: &Path) -> Result<Engine, String> {
    let settings = Settings::load(None, None).map_err(|error| error.to_string())?;
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(vec![root.to_path_buf()])
        .map_err(|error| error.to_string())?;
    if !engine.wait_for_workers(INDEX_TIMEOUT) {
        return Err("indexing did not finish".to_owned());
    }
    Ok(engine)
}

/// `path` relative to `root` when it lies beneath it, with `/` separators.
fn display_path(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Information => "info",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Error => "error",
    }
}

#[derive(Serialize)]
struct JsonRange {
    start: usize,
    end: usize,
}

#[derive(Serialize)]
struct JsonDiagnostic {
    path: String,
    range: JsonRange,
    /// One-based.
    line: usize,
    /// One-based, in bytes.
    column: usize,
    severity: &'static str,
    code: &'static str,
    message: String,
}

fn check(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["format", "fail-on"])?;
    let [root] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let json = match option(&options, "format").unwrap_or("text") {
        "text" => false,
        "json" => true,
        other => return Err(format!("unknown format `{other}`")),
    };
    let threshold = match option(&options, "fail-on").unwrap_or("error") {
        "error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        other => return Err(format!("unknown --fail-on level `{other}`")),
    };

    let root = adoc_index::normalize_path(Path::new(root));
    let engine = open_engine(&root)?;
    let mut failed = false;
    let mut entries = Vec::new();
    for (path, diagnostics) in engine.workspace_diagnostics() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines = LineIndex::new(&text);
        for diagnostic in diagnostics {
            failed |= fails(&diagnostic, threshold);
            let (line, column) = lines.line_column(diagnostic.range.start);
            entries.push(JsonDiagnostic {
                path: display_path(&path, &root),
                range: JsonRange {
                    start: diagnostic.range.start,
                    end: diagnostic.range.end,
                },
                line: line + 1,
                column: column + 1,
                severity: severity_name(diagnostic.severity),
                code: diagnostic.code.as_str(),
                message: diagnostic.message,
            });
        }
    }
    if json {
        let output = serde_json::to_string_pretty(&entries).map_err(|error| error.to_string())?;
        println!("{output}");
    } else {
        for entry in &entries {
            println!(
                "{}:{}:{}: {} {} {}",
                entry.path, entry.line, entry.column, entry.severity, entry.code, entry.message
            );
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn fails(diagnostic: &Diagnostic, threshold: DiagnosticSeverity) -> bool {
    let rank = |severity| match severity {
        DiagnosticSeverity::Information => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Error => 2,
    };
    rank(diagnostic.severity) >= rank(threshold)
}

fn symbols(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, _) = parse_options(arguments, &[])?;
    let [root, query] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let root = adoc_index::normalize_path(Path::new(root));
    let engine = open_engine(&root)?;
    for symbol in engine.workspace_symbols(query) {
        let text = std::fs::read_to_string(&symbol.path).unwrap_or_default();
        let (line, column) = LineIndex::new(&text).line_column(symbol.range.start);
        println!(
            "{:?} {} {}:{}:{}",
            symbol.kind,
            symbol.name,
            display_path(&symbol.path, &root),
            line + 1,
            column + 1
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The root to index for `file`: the parent of the nearest component (so sibling components
/// resolve), or the file's own directory outside Antora.
fn default_root(file: &Path) -> PathBuf {
    let directory = file.parent().unwrap_or_else(|| Path::new("."));
    let root = directory
        .ancestors()
        .find(|ancestor| ancestor.join("antora.yml").is_file())
        .map_or_else(
            || directory.to_path_buf(),
            |component| component.parent().unwrap_or(component).to_path_buf(),
        );
    // A relative path's last ancestor is the empty path, which stands for the current directory.
    if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root
    }
}

fn follow(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["root"])?;
    let [file, offset] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let file = adoc_index::normalize_path(Path::new(file));
    let offset: usize = offset
        .parse()
        .map_err(|_| format!("`{offset}` is not a byte offset"))?;
    let root = option(&options, "root").map_or_else(
        || default_root(&file),
        |root| adoc_index::normalize_path(Path::new(root)),
    );

    let mut engine = open_engine(&root)?;
    let id = engine.open(&file).map_err(|error| error.to_string())?;
    match engine.invoke(
        ids::NAVIGATE_FOLLOW_LINK,
        CommandArgs::Offset { id, offset },
    ) {
        Ok(CommandOutcome::Location(location)) => {
            let text = std::fs::read_to_string(&location.path).unwrap_or_default();
            let (line, column) = LineIndex::new(&text).line_column(location.range.start);
            println!(
                "{}:{}:{}",
                display_path(&location.path, &root),
                line + 1,
                column + 1
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(CommandOutcome::External(path)) => {
            println!("external {}", display_path(&path, &root));
            Ok(ExitCode::SUCCESS)
        }
        Ok(other) => Err(format!("unexpected outcome {other:?}")),
        Err(EngineError::NotFound(message)) => {
            eprintln!("{message}");
            Ok(ExitCode::FAILURE)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Renders a page to a standalone HTML file, or to standard output, as the preview shows it.
fn render(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["root", "output", "parser"])?;
    let [file] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let parser: std::sync::Arc<dyn adoc_dom::DocumentParser> =
        match option(&options, "parser").unwrap_or("acdc") {
            "acdc" => std::sync::Arc::new(adoc_dom::AcdcParser),
            "asciidoc-parser" => std::sync::Arc::new(adoc_dom::AsciidocParser),
            other => return Err(format!("unknown parser `{other}`")),
        };
    let file = adoc_index::normalize_path(Path::new(file));
    let root = option(&options, "root").map_or_else(
        || default_root(&file),
        |root| adoc_index::normalize_path(Path::new(root)),
    );

    let mut engine = open_engine(&root)?;
    engine.set_document_parser(parser);
    let id = engine.open(&file).map_err(|error| error.to_string())?;
    let preview = engine
        .render_blocking(id, INDEX_TIMEOUT)
        .ok_or_else(|| format!("{} did not render", file.display()))?;
    let html = adoc_render::to_html(
        &preview.view,
        &adoc_render::HtmlOptions { standalone: true },
    );
    match option(&options, "output") {
        Some(output) => {
            std::fs::write(output, html).map_err(|error| format!("{output}: {error}"))?;
        }
        None => print!("{html}"),
    }
    Ok(ExitCode::SUCCESS)
}
