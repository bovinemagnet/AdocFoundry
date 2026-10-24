//! `adocfoundry-cli`: the headless front end over `adoc-engine`.
//!
//! ```text
//! adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
//! adocfoundry-cli symbols <root> <query>
//! adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
//! adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
//! adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>]
//!     [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>]
//!     [--standard pdf-a-2a|pdf-ua-1] [--typst <file.typ>]
//! adocfoundry-cli git-status <root> [--format text|json]
//! adocfoundry-cli --version
//! ```

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use adoc_core::{DiagnosticSeverity, LineIndex};
use adoc_engine::{
    commands::ids,
    pdf_standard,
    report::{display_path, severity_name, ChangedFile},
    CommandArgs, CommandOutcome, Engine, EngineError, ExportArgs, ExportTarget, Settings,
};

const USAGE: &str = "usage:
  adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
  adocfoundry-cli symbols <root> <query>
  adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
  adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--parser acdc|asciidoc-parser]
  adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>]
      [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>]
      [--standard pdf-a-2a|pdf-ua-1] [--typst <file.typ>]
  adocfoundry-cli git-status <root> [--format text|json]
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
        "pdf" => pdf(rest),
        "git-status" => git_status(rest),
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

/// An engine over the workspace at `root`, indexed. Git is looked at only when `git` is true.
fn open_engine(root: &Path, git: bool) -> Result<Engine, String> {
    let mut settings = Settings::load(None, None).map_err(|error| error.to_string())?;
    settings.git.enabled = git;
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(vec![root.to_path_buf()])
        .map_err(|error| error.to_string())?;
    if !engine.wait_for_workers(INDEX_TIMEOUT) {
        return Err("indexing did not finish".to_owned());
    }
    Ok(engine)
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
    let engine = open_engine(&root, false)?;
    let entries = engine.check_report(&root, None);
    let failed = entries.iter().any(|entry| entry.meets(threshold));
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

fn symbols(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, _) = parse_options(arguments, &[])?;
    let [root, query] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let root = adoc_index::normalize_path(Path::new(root));
    let engine = open_engine(&root, false)?;
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

    let mut engine = open_engine(&root, false)?;
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

    let mut engine = open_engine(&root, false)?;
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

/// Exports a page, or a component version as a book, to PDF.
fn pdf(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(
        arguments,
        &[
            "root",
            "component",
            "theme",
            "fonts-dir",
            "output",
            "standard",
            "typst",
        ],
    )?;
    let [target] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let target = adoc_index::normalize_path(Path::new(target));
    let is_dir = target.is_dir();
    let root = option(&options, "root").map_or_else(
        || {
            if is_dir {
                // A component directory: its parent, so sibling components resolve; the parent
                // of a relative name such as `guide` is the current directory.
                if target.join("antora.yml").is_file() {
                    match target.parent() {
                        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                        _ => PathBuf::from("."),
                    }
                } else {
                    target.clone()
                }
            } else {
                default_root(&target)
            }
        },
        |root| adoc_index::normalize_path(Path::new(root)),
    );
    let standards = options
        .iter()
        .filter(|(name, _)| *name == "standard")
        .map(|(_, value)| {
            pdf_standard(value)
                .ok_or_else(|| format!("unknown standard `{value}`: expected pdf-a-2a or pdf-ua-1"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut engine = open_engine(&root, false)?;
    let component = match option(&options, "component") {
        Some(spec) => Some(match spec.split_once('@') {
            Some((version, name)) => (name.to_owned(), Some(version.to_owned())),
            None => (spec.to_owned(), None),
        }),
        None if is_dir => {
            let component = engine
                .workspace()
                .antora
                .components()
                .find(|component| component.root == target)
                .ok_or_else(|| format!("{} is not an Antora component", target.display()))?;
            Some((component.name.clone(), component.version.clone()))
        }
        None => None,
    };
    let (export_target, default_output) = match component {
        Some((name, version)) => {
            let version = version.or_else(|| {
                engine
                    .workspace()
                    .antora
                    .latest_version(&name)
                    .and_then(|latest| latest.version.clone())
            });
            let output = PathBuf::from(format!("{name}.pdf"));
            (ExportTarget::Component { name, version }, output)
        }
        None => {
            let id = engine.open(&target).map_err(|error| error.to_string())?;
            (ExportTarget::Page(id), target.with_extension("pdf"))
        }
    };
    let output = option(&options, "output").map_or(default_output, PathBuf::from);
    let summary = engine.export_blocking(
        ExportArgs {
            target: export_target,
            output: output.clone(),
            theme: option(&options, "theme").map(str::to_owned),
            standards,
            fonts_dir: option(&options, "fonts-dir").map(PathBuf::from),
            typst: option(&options, "typst").map(PathBuf::from),
        },
        INDEX_TIMEOUT,
    );
    match summary {
        Ok(summary) => {
            for diagnostic in &summary.diagnostics {
                match &diagnostic.location {
                    Some((path, line)) => eprintln!(
                        "{}:{line}: {}: {}",
                        display_path(path, &root),
                        severity_name(diagnostic.severity),
                        diagnostic.message
                    ),
                    None => eprintln!(
                        "{}: {}",
                        severity_name(diagnostic.severity),
                        diagnostic.message
                    ),
                }
            }
            println!("{} ({} pages)", output.display(), summary.pages);
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("adocfoundry-cli: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The branch of the repository holding `root`, and its changed AsciiDoc files with the number
/// of changed line runs in each.
fn git_status(arguments: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["format"])?;
    let [root] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let json = match option(&options, "format").unwrap_or("text") {
        "text" => false,
        "json" => true,
        other => return Err(format!("unknown format `{other}`")),
    };
    let root = adoc_index::normalize_path(Path::new(root));
    let mut engine = open_engine(&root, true)?;
    let report = engine
        .git_status_report(&root, INDEX_TIMEOUT)
        .map_err(|error| error.to_string())?;
    if json {
        let output = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        println!("{output}");
    } else {
        println!("branch {}", report.head);
        for file in &report.files {
            let hunks = file
                .hunks
                .map_or_else(String::new, |count| format!(" ({count} changed)"));
            println!("{} {}{hunks}", short_status(file), file.path);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The two letters `git status --short` shows for a file.
fn short_status(file: &ChangedFile) -> String {
    match file.worktree {
        "untracked" => "??".to_owned(),
        "ignored" => "!!".to_owned(),
        worktree => format!("{}{}", change_letter(file.index), change_letter(worktree)),
    }
}

/// The letter `git status --short` shows for a change.
fn change_letter(name: &str) -> char {
    match name {
        "added" => 'A',
        "modified" => 'M',
        "deleted" => 'D',
        "renamed" => 'R',
        "untracked" => '?',
        "ignored" => '!',
        "conflicted" => 'U',
        _ => ' ',
    }
}
