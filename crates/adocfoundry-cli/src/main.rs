//! `adocfoundry-cli`: the headless front end over `adoc-engine`.
//!
//! ```text
//! adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
//! adocfoundry-cli audit <root> [--component [<version>@]<name>] [--format text|json|sarif]
//!     [--output <file>] [--fail-on none|warning|error] [--require-complete]
//! adocfoundry-cli symbols <root> <query>
//! adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
//! adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--link-images] [--parser acdc|asciidoc-parser]
//! adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>]
//!     [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>]
//!     [--standard pdf-a-1b|pdf-a-1a|pdf-a-2a|pdf-ua-1] [--typst <file.typ>]
//!     [--fail-on none|warning|error]
//! adocfoundry-cli pdf --profile <name> [<root>] [--output <file.pdf>] [--typst <file.typ>]
//!     [--fail-on none|warning|error]
//! adocfoundry-cli git-status <root> [--format text|json]
//! adocfoundry-cli graph <root> [--page <file> [--depth <n>] | --path <from> <to> [--undirected]] [--ages]
//!     [--format json|dot|svg] [--no-partials] [--no-nav] [--unresolved] [--dark]
//! adocfoundry-cli doctor [<root>] [--format text|json]
//! adocfoundry-cli --version
//! ```

use std::{
    io::{self, BufWriter, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use adoc_core::{DiagnosticSeverity, LineIndex};
use adoc_engine::{
    audit,
    commands::ids,
    pdf_standard,
    report::{display_path, severity_name, ChangedFile},
    CommandArgs, CommandOutcome, Engine, EngineError, ExportArgs, ExportTarget, GraphFilter,
    GraphScope, Settings, SvgStyle,
};

const USAGE: &str = "usage:
  adocfoundry-cli check <root> [--format text|json] [--fail-on warning|error]
  adocfoundry-cli audit <root> [--component [<version>@]<name>] [--format text|json|sarif]
      [--output <file>] [--fail-on none|warning|error] [--require-complete]
  adocfoundry-cli symbols <root> <query>
  adocfoundry-cli follow <file> <byte-offset> [--root <dir>]
  adocfoundry-cli render <file> [--root <dir>] [--output <file.html>] [--link-images] [--parser acdc|asciidoc-parser]
  adocfoundry-cli pdf <file|component-dir> [--root <dir>] [--component [<version>@]<name>]
      [--theme <name|file.yml>] [--fonts-dir <dir>] [--output <file.pdf>]
      [--standard pdf-a-1b|pdf-a-1a|pdf-a-2a|pdf-ua-1] [--typst <file.typ>]
      [--fail-on none|warning|error]
  adocfoundry-cli pdf --profile <name> [<root>] [--output <file.pdf>] [--typst <file.typ>]
      [--fail-on none|warning|error]
  adocfoundry-cli git-status <root> [--format text|json]
  adocfoundry-cli graph <root> [--page <file> [--depth <n>] | --path <from> <to> [--undirected]] [--ages]
      [--format json|dot|svg] [--no-partials] [--no-nav] [--unresolved] [--dark]
  adocfoundry-cli doctor [<root>] [--format text|json]
  adocfoundry-cli impact <root> (--base <ref> --head <ref> | --against HEAD | --changed-file <path>…)
      [--format text|json] [--output <file>] [--max-impacts <n>] [--require-complete]
  adocfoundry-cli new-module <component-root> <name> [--families pages,partials,images]
      [--no-nav] [--index-title <title>]
  adocfoundry-cli new-project <folder> --name <name> [--template <id|folder>] [--title <title>]
      [--version <version>] [--subfolder <dir>] [--no-git] [--no-readme] [--no-playbook]
  adocfoundry-cli --version
Relative paths, `--changed-file` among them, are read against the current directory.";

const INDEX_TIMEOUT: Duration = Duration::from_secs(300);

/// Writes to standard output through an [`Out`], as `print!` does.
macro_rules! out {
    ($out:expr, $($argument:tt)*) => {
        $out.write(format_args!($($argument)*))
    };
}

/// Writes a line to standard output through an [`Out`], as `println!` does.
macro_rules! outln {
    ($out:expr, $($argument:tt)*) => {
        $out.write(format_args!("{}\n", format_args!($($argument)*)))
    };
}

/// Standard output, locked and buffered once. A failed write ends the output rather than the
/// program, as `println!` would by panicking: a reader that has gone (`check docs | head`) is
/// the end of what anyone reads, and the command exits with the code it earned (#513).
struct Out {
    writer: BufWriter<io::StdoutLock<'static>>,
    /// The first failed write, after which nothing more is written.
    error: Option<io::Error>,
}

impl Out {
    fn new() -> Self {
        Self {
            writer: BufWriter::new(io::stdout().lock()),
            error: None,
        }
    }

    fn write(&mut self, arguments: std::fmt::Arguments<'_>) {
        if self.error.is_none() {
            if let Err(error) = self.writer.write_fmt(arguments) {
                self.error = Some(error);
            }
        }
    }

    /// Writes what is buffered, and returns the error that stopped the output, unless it was
    /// only that the reader went.
    fn finish(mut self) -> Option<io::Error> {
        if self.error.is_none() {
            if let Err(error) = self.writer.flush() {
                self.error = Some(error);
            }
        }
        self.error
            .take()
            .filter(|error| error.kind() != io::ErrorKind::BrokenPipe)
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut out = Out::new();
    let result = run(&arguments, &mut out);
    // What was written goes out before any message about it.
    let unwritten = out.finish();
    match (result, unwritten) {
        (Err(message), _) => {
            eprintln!("adocfoundry-cli: {message}");
            ExitCode::from(2)
        }
        (Ok(_), Some(error)) => {
            eprintln!("adocfoundry-cli: could not write the output: {error}");
            ExitCode::from(2)
        }
        (Ok(code), None) => code,
    }
}

fn run(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let Some((command, rest)) = arguments.split_first() else {
        return Err(USAGE.to_owned());
    };
    match command.as_str() {
        "--version" | "-V" => {
            outln!(out, "adocfoundry-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "--help" | "-h" => {
            outln!(out, "{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "check" => check(rest, out),
        "audit" => audit(rest, out),
        "symbols" => symbols(rest, out),
        "follow" => follow(rest, out),
        "render" => render(rest, out),
        "pdf" => pdf(rest, out),
        "git-status" => git_status(rest, out),
        "graph" => graph(rest, out),
        "doctor" => doctor(rest, out),
        "new-module" => new_module(rest, out),
        "new-project" => new_project(rest, out),
        "impact" => impact(rest, out),
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

/// Makes a module in the component version at `<component-root>` (the folder holding
/// `antora.yml`) through the engine's `module.new`, as the app's New Module does, and lists the
/// files it made, relative to the component root.
fn new_module(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let nav = !arguments.iter().any(|argument| argument == "--no-nav");
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|argument| *argument != "--no-nav")
        .cloned()
        .collect();
    let (positional, options) = parse_options(&arguments, &["families", "index-title"])?;
    let [component, name] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let families = match option(&options, "families") {
        None => adoc_engine::NewModule::DEFAULT_FAMILIES.to_vec(),
        Some(list) => list
            .split(',')
            .map(|name| {
                let name = name.trim();
                adoc_engine::ResourceFamily::ALL
                    .into_iter()
                    .find(|family| family.directory() == name || family.to_string() == name)
                    .ok_or_else(|| format!("unknown family `{name}`"))
            })
            .collect::<Result<_, _>>()?,
    };
    let root = adoc_index::normalize_path(Path::new(component));
    let mut engine = open_engine(&root, false)?;
    let request = adoc_engine::NewModule {
        component: root.clone(),
        name: (*name).to_owned(),
        families,
        nav,
        index_title: Some(
            option(&options, "index-title")
                .unwrap_or_default()
                .to_owned(),
        ),
    };
    engine
        .invoke(
            adoc_engine::commands::ids::MODULE_NEW,
            adoc_engine::CommandArgs::NewModule(request),
        )
        .map_err(|error| error.to_string())?;
    // Every write is done: the files made, listed in order.
    let folder = root.join("modules").join(name);
    let mut made = Vec::new();
    let mut folders = vec![folder];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().is_dir() {
                folders.push(entry.path());
            } else {
                made.push(adoc_engine::report::display_path(&entry.path(), &root));
            }
        }
    }
    made.sort();
    for file in made {
        outln!(out, "made {file}");
    }
    if nav {
        outln!(out, "listed modules/{name}/nav.adoc in antora.yml");
    }
    Ok(ExitCode::SUCCESS)
}

/// Makes an Antora project in `<folder>` (empty, or with `--subfolder` for a code repository)
/// through the engine's `project.new`, as the app's New Antora Project does, and lists the files
/// it made, relative to the folder.
fn new_project(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let flags = ["--no-git", "--no-readme", "--no-playbook"];
    let has = |flag: &str| arguments.iter().any(|argument| argument == flag);
    let (git, readme, playbook) = (!has("--no-git"), !has("--no-readme"), !has("--no-playbook"));
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|argument| !flags.contains(&argument.as_str()))
        .cloned()
        .collect();
    let (positional, options) = parse_options(
        &arguments,
        &["name", "title", "version", "subfolder", "template"],
    )?;
    let [folder] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let root = adoc_index::normalize_path(Path::new(folder));
    let mut engine = open_engine(&root, false)?;
    // A built-in or listed template, or a template's folder.
    let template = match option(&options, "template") {
        Some(given) if Path::new(given).is_dir() => std::path::absolute(given)
            .map_err(|error| format!("{given}: {error}"))?
            .display()
            .to_string(),
        Some(given) => given.to_owned(),
        None => adoc_engine::DEFAULT_TEMPLATE.to_owned(),
    };
    let chosen = engine.template(&template)?;
    let name = match option(&options, "name") {
        Some(name) => name,
        None if chosen.uses(adoc_engine::TemplateField::Name) => {
            return Err("`--name` is needed".to_owned())
        }
        None => "",
    };
    let request = adoc_engine::NewProject {
        template,
        subfolder: option(&options, "subfolder").map(str::to_owned),
        title: option(&options, "title").unwrap_or_default().to_owned(),
        version: option(&options, "version").map(str::to_owned),
        readme,
        git_init: git,
        playbook,
        ..adoc_engine::NewProject::new(root.clone(), name)
    };
    let had_repository = root.join(".git").exists();
    // Listed before, as what is planned depends on what is there already.
    let mut made: Vec<String> = engine
        .planned_tree(&request)
        .iter()
        .map(|path| adoc_engine::report::display_path(path, &root))
        .collect();
    engine
        .invoke(
            adoc_engine::commands::ids::PROJECT_NEW,
            adoc_engine::CommandArgs::NewProject(request.clone()),
        )
        .map_err(|error| error.to_string())?;
    made.sort();
    for file in made {
        outln!(out, "made {file}");
    }
    if !had_repository && root.join(".git").exists() {
        outln!(out, "initialised a Git repository");
    }
    Ok(ExitCode::SUCCESS)
}

/// Reports what a change to the documentation may affect (Impact PRD §§6, 7 and 9): between two
/// Git refs, `HEAD` against the working tree, or for paths named, as text or JSON. The exit
/// code is 0 for a finished analysis, however many impacts; 2 when the source is invalid or
/// unavailable, or the analysis is partial under `--require-complete`.
fn impact(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let require_complete = arguments
        .iter()
        .any(|argument| argument == "--require-complete");
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|argument| *argument != "--require-complete")
        .cloned()
        .collect();
    let (positional, options) = parse_options(
        &arguments,
        &[
            "base",
            "head",
            "against",
            "changed-file",
            "format",
            "output",
            "max-impacts",
        ],
    )?;
    let [root] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let format = option(&options, "format").unwrap_or("text");
    if !matches!(format, "text" | "json") {
        return Err(format!("unknown format `{format}`"));
    }
    let root = adoc_index::normalize_path(Path::new(root));
    // Read against the current directory, as `git diff --name-only` gives them from the top of
    // the repository, and handed to the engine relative to the root.
    let changed: Vec<PathBuf> = options
        .iter()
        .filter(|(name, _)| *name == "changed-file")
        .map(|(_, path)| within(&root, path))
        .collect::<Result<_, _>>()?;
    let source = match (
        option(&options, "base"),
        option(&options, "head"),
        option(&options, "against"),
        changed.is_empty(),
    ) {
        (Some(base), Some(head), None, true) => adoc_engine::ChangeSource::GitRefs {
            base: base.to_owned(),
            head: head.to_owned(),
        },
        (None, None, Some("HEAD"), true) => adoc_engine::ChangeSource::WorkingTree,
        (None, None, Some(other), true) => {
            return Err(format!(
                "--against compares with `HEAD` only, not `{other}`; use --base and --head"
            ))
        }
        (None, None, None, false) => adoc_engine::ChangeSource::Paths(changed),
        _ => {
            return Err(format!(
                "give one of --base <ref> --head <ref>, --against HEAD or --changed-file <path>\n{USAGE}"
            ))
        }
    };
    let mut limits = adoc_engine::impact::Limits::default();
    if let Some(max) = option(&options, "max-impacts") {
        limits.impacts = max
            .parse()
            .map_err(|_| format!("--max-impacts takes a number, not `{max}`"))?;
    }
    let mut engine = open_engine(&root, false)?;
    let report = match engine.analyse_impact(&source, limits) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("adocfoundry-cli: {error}");
            return Ok(ExitCode::from(2));
        }
    };
    let written = match format {
        "json" => serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        _ => impact_text(&report),
    };
    match option(&options, "output") {
        Some(file) => std::fs::write(file, format!("{written}\n"))
            .map_err(|error| format!("{file}: {error}"))?,
        None => outln!(out, "{written}"),
    }
    let incomplete = !report.is_complete();
    Ok(ExitCode::from(if require_complete && incomplete {
        2
    } else {
        0
    }))
}

/// A report for reading: what was compared and how far, the changes, then the impacts by
/// kind, each with why, rendered content apart from references to review.
fn impact_text(report: &adoc_engine::impact::ImpactReport) -> String {
    use adoc_engine::impact::{Completeness, ImpactKind, ReportSource};
    use std::fmt::Write as _;

    let mut out = String::new();
    let source = match &report.source {
        Some(ReportSource::GitRefs { base, head }) => format!(
            "{} ({}) against {} ({})",
            head.spec,
            &head.commit[..head.commit.len().min(12)],
            base.spec,
            &base.commit[..base.commit.len().min(12)]
        ),
        Some(ReportSource::WorkingTree { base, .. }) => format!(
            "the working tree against {} ({})",
            base.spec,
            &base.commit[..base.commit.len().min(12)]
        ),
        Some(ReportSource::Paths) | None => "the paths named, with no base revision".to_owned(),
        Some(ReportSource::Removal) => "the paths named, as if removed".to_owned(),
    };
    let status = match &report.completeness {
        Completeness::Complete => "complete",
        Completeness::Partial { .. } => "partial",
        Completeness::Unavailable { .. } => "unavailable",
    };
    let _ = writeln!(out, "Impact of {source}: {status}");
    if let Completeness::Partial { gaps } | Completeness::Unavailable { gaps } =
        &report.completeness
    {
        for gap in gaps {
            let _ = writeln!(out, "  not covered ({}): {}", gap.reason, gap.detail);
        }
    }
    let _ = writeln!(out, "\nChanged ({}):", report.changes.len());
    for change in &report.changes {
        match &change.from {
            Some(from) => {
                let _ = writeln!(out, "  {} {} (from {from})", change.kind, change.path);
            }
            None => {
                let _ = writeln!(out, "  {} {}", change.kind, change.path);
            }
        }
    }
    for (kind, title) in [
        (ImpactKind::Render, "Rendered content that may change"),
        (ImpactKind::ReferenceIntegrity, "References to review"),
        (ImpactKind::Navigation, "Navigation"),
        (ImpactKind::Publication, "Publications"),
    ] {
        let impacts: Vec<_> = report
            .impacts
            .iter()
            .filter(|impact| impact.kind == kind)
            .collect();
        if impacts.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{title} ({}):", impacts.len());
        for impact in impacts {
            let what = impact.resource_id.as_deref().unwrap_or(&impact.path);
            let mut line = format!("  {what}");
            if let Some(publication) = &impact.publication {
                line = format!("  {publication}: {what}");
            }
            if let Some(status) = impact.status {
                let status = serde_json::to_value(status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let _ = write!(line, " [{}]", status.replace('_', " "));
            }
            if impact.confidence == adoc_engine::impact::Confidence::Potential {
                line.push_str(" (potential)");
            }
            if let Some(anchor) = &impact.anchor {
                let _ = write!(line, " #{anchor}");
            }
            let _ = writeln!(out, "{line}");
            let _ = writeln!(out, "      via {}", impact.via.join(" → "));
            if let Some(source) = &impact.source {
                let _ = writeln!(
                    out,
                    "      at {} bytes {}–{}",
                    source.path, source.range.start, source.range.end
                );
            }
        }
    }
    if let Some(truncated) = report.truncated {
        let _ = writeln!(
            out,
            "\nShown {} of {} impacts; --max-impacts raises the limit.",
            truncated.shown, truncated.total
        );
    }
    out.trim_end().to_owned()
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
    if git {
        settings.git.enabled = true;
    } else {
        // Opt-in rather than off, so opening the workspace, which reads the settings again,
        // does not turn it back on.
        settings = settings.with_git_opt_in();
    }
    open_engine_with(root, settings)
}

/// An engine with `settings` over the workspace at `root`, indexed.
fn open_engine_with(root: &Path, settings: Settings) -> Result<Engine, String> {
    let mut engine = Engine::new(settings);
    engine
        .open_workspace(vec![root.to_path_buf()])
        .map_err(|error| error.to_string())?;
    if !engine.wait_for_workers(INDEX_TIMEOUT) {
        return Err("indexing did not finish".to_owned());
    }
    // A workspace that could not be indexed has nothing to report, which is not a pass.
    if let Some(error) = engine.index_failure() {
        return Err(error.to_owned());
    }
    Ok(engine)
}

fn check(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
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
        outln!(out, "{output}");
    } else {
        for entry in &entries {
            outln!(
                out,
                "{}:{}:{}: {} {} {}",
                entry.path,
                entry.line,
                entry.column,
                entry.severity,
                entry.code,
                entry.message
            );
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Audits the documentation's quality and writes the report as text, JSON or SARIF. The policy
/// is the workspace's own `.adocfoundry.toml` `[quality]` table, never a user file, so CI gets
/// the same result on every machine; `--fail-on` and `--require-complete` override it. The exit
/// code is the report's verdict (Quality PRD §5.4): 0 below the threshold, 1 at it, and 2 when
/// the audit is unavailable, partial while completeness is required, or the `[quality]`
/// settings are invalid. The report is written in full before exiting, whatever the code.
fn audit(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let require_complete = arguments
        .iter()
        .any(|argument| argument == "--require-complete");
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|argument| *argument != "--require-complete")
        .cloned()
        .collect();
    let (positional, options) =
        parse_options(&arguments, &["format", "fail-on", "component", "output"])?;
    let [root] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let format = option(&options, "format").unwrap_or("text");
    if !matches!(format, "text" | "json" | "sarif") {
        return Err(format!("unknown format `{format}`"));
    }
    let fail_on = option(&options, "fail-on")
        .map(|name| {
            audit::FailOn::from_name(name)
                .ok_or_else(|| format!("unknown --fail-on level `{name}`"))
        })
        .transpose()?;
    let scope = match option(&options, "component") {
        None => audit::AuditScope::Workspace,
        Some(spec) => match spec.split_once('@') {
            Some((version, name)) => audit::AuditScope::Component {
                name: name.to_owned(),
                version: Some(version.to_owned()),
            },
            None => audit::AuditScope::Component {
                name: spec.to_owned(),
                version: None,
            },
        },
    };

    let root = adoc_index::normalize_path(Path::new(root));
    let engine = open_engine_with(&root, audit_settings(&root)?)?;
    // Opening the workspace reads its settings file again, so the options apply afterwards.
    let quality = &engine.settings().quality;
    let problems = quality.problems.clone();
    let mut policy = quality.policy.clone();
    if let Some(fail_on) = fail_on {
        policy.fail_on = fail_on;
    }
    if require_complete {
        policy.require_complete = true;
    }
    let report = engine.audit_now_with(scope, &policy);

    let roots = [root];
    let texts = std::cell::RefCell::new(std::collections::HashMap::new());
    let regions = |location: &audit::Location| {
        let range = location.range?;
        let mut texts = texts.borrow_mut();
        let text = texts
            .entry((location.root, location.path.clone()))
            .or_insert_with(|| {
                roots
                    .get(location.root)
                    .and_then(|root| std::fs::read_to_string(root.join(&location.path)).ok())
            });
        text.as_deref().map(|text| audit::Region::of(text, range))
    };
    let written = match format {
        "json" => serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        "sarif" => serde_json::to_string_pretty(&audit::to_sarif(
            &report,
            env!("CARGO_PKG_VERSION"),
            &regions,
        ))
        .map_err(|error| error.to_string())?,
        _ => audit_text(&report, &regions),
    };
    match option(&options, "output") {
        Some(file) => std::fs::write(file, format!("{written}\n"))
            .map_err(|error| format!("{file}: {error}"))?,
        None => outln!(out, "{written}"),
    }
    for problem in &problems {
        eprintln!("adocfoundry-cli: invalid [quality] settings: {problem}");
    }
    let code = if problems.is_empty() {
        report.verdict().exit_code()
    } else {
        2
    };
    Ok(ExitCode::from(code))
}

/// The settings `audit` opens the workspace at `root` with: its own settings file, never a
/// user's, and Git off, as the audit reads nothing of it. Opt-in rather than off, so opening
/// the workspace, which reads the settings again, does not turn it back on (#508).
fn audit_settings(root: &Path) -> Result<Settings, String> {
    Settings::load(None, Some(root))
        .map(Settings::with_git_opt_in)
        .map_err(|error| error.to_string())
}

/// A report for reading: its scope and completeness first, the counts, each finding at its
/// place, the gaps and navigation coverage, and the result last.
fn audit_text(
    report: &audit::AuditReport,
    regions: &dyn Fn(&audit::Location) -> Option<audit::Region>,
) -> String {
    let scope = match report.scope() {
        audit::AuditScope::Workspace => "the workspace".to_owned(),
        audit::AuditScope::Component {
            name,
            version: Some(version),
        } => format!("{name} {version}"),
        audit::AuditScope::Component {
            name,
            version: None,
        } => format!("component {name}"),
    };
    let (status, gaps) = match report.completeness() {
        audit::Completeness::Complete => ("complete".to_owned(), &[][..]),
        audit::Completeness::Partial(gaps) => (
            format!(
                "partial coverage ({} gap{})",
                gaps.len(),
                if gaps.len() == 1 { "" } else { "s" }
            ),
            gaps.as_slice(),
        ),
        audit::Completeness::Unavailable(gaps) => ("unavailable".to_owned(), gaps.as_slice()),
    };
    let counts = report.counts();
    let mut lines = vec![
        format!("Audit of {scope}: {status}"),
        format!(
            "{} errors, {} warnings, {} info; {} excluded, {} skipped",
            counts.error, counts.warning, counts.info, counts.excluded, counts.skipped
        ),
    ];
    for finding in report.findings() {
        let place = match regions(&finding.location) {
            Some(region) => format!(
                "{}:{}:{}",
                finding.location.path, region.start_line, region.start_column
            ),
            None => finding.location.path.clone(),
        };
        let message: Vec<&str> = finding
            .message
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        lines.push(format!(
            "{place}: {} {} {}",
            finding.severity.as_str(),
            finding.code,
            message.join(" ")
        ));
    }
    for gap in gaps {
        let reason = serde_json::to_value(gap.reason)
            .ok()
            .and_then(|reason| reason.as_str().map(str::to_owned))
            .unwrap_or_default();
        lines.push(format!("Gap ({reason}): {}", gap.detail));
    }
    for coverage in report.coverage() {
        let version = coverage
            .version
            .as_ref()
            .map_or_else(String::new, |version| format!(" {version}"));
        let undetermined = if coverage.undetermined > 0 {
            format!(", {} undetermined", coverage.undetermined)
        } else {
            String::new()
        };
        lines.push(format!(
            "Navigation of {}{version}: {} of {} pages listed{undetermined}",
            coverage.component, coverage.listed, coverage.pages
        ));
    }
    for status in report.providers() {
        match &status.state {
            audit::ProviderState::Disabled => {}
            audit::ProviderState::Ran => lines.push(format!("Provider {}: ran", status.provider)),
            audit::ProviderState::Unavailable { .. } => {
                lines.push(format!("Provider {}: unavailable", status.provider));
            }
        }
    }
    let partial = matches!(report.completeness(), audit::Completeness::Partial(_));
    lines.push(
        match report.verdict() {
            audit::Verdict::Pass if partial => "Result: pass, with partial coverage",
            audit::Verdict::Pass => "Result: pass",
            audit::Verdict::Fail => "Result: fail",
            audit::Verdict::Untrustworthy if partial => {
                "Result: not trustworthy, as coverage is partial and completeness is required"
            }
            audit::Verdict::Untrustworthy => "Result: not trustworthy, as nothing could be audited",
        }
        .to_owned(),
    );
    lines.join("\n")
}

fn symbols(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let (positional, _) = parse_options(arguments, &[])?;
    let [root, query] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let root = adoc_index::normalize_path(Path::new(root));
    let engine = open_engine(&root, false)?;
    for symbol in engine.workspace_symbols(query) {
        let text = std::fs::read_to_string(&symbol.path).unwrap_or_default();
        let (line, column) = LineIndex::new(&text).line_character(&text, symbol.range.start);
        outln!(
            out,
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

/// `path` made absolute against the current directory, lexically: the file system is not read,
/// so a path that does not exist is made absolute too. The file `follow`, `render` and `pdf`
/// take, and their root, go through it, so the component above the file is found wherever the
/// command runs from and however the two are written (#510); what is printed stays relative to
/// the root, through `display_path`.
fn absolute(path: impl AsRef<Path>) -> Result<PathBuf, String> {
    let path = path.as_ref();
    std::path::absolute(path)
        .map(|absolute| adoc_index::normalize_path(&absolute))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// `path`, read against the current directory, relative to `root` when it lies beneath it, and
/// otherwise absolute: so a relative root and an absolute path, or the reverse, name the same
/// file.
fn within(root: &Path, path: &str) -> Result<PathBuf, String> {
    let path = absolute(path)?;
    Ok(match path.strip_prefix(absolute(root)?) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => path,
    })
}

/// `path`, read against the current directory, written as the index writes the files beneath
/// `root`: joined to the root as it was given.
fn beneath(root: &Path, path: &str) -> Result<PathBuf, String> {
    Ok(adoc_index::normalize_path(&root.join(within(root, path)?)))
}

/// The root to index for `file`, an absolute path: the parent of the nearest component (so
/// sibling components resolve), or the file's own directory outside Antora.
fn default_root(file: &Path) -> PathBuf {
    let directory = file.parent().unwrap_or(file);
    directory
        .ancestors()
        .find(|ancestor| ancestor.join("antora.yml").is_file())
        .map_or_else(
            || directory.to_path_buf(),
            |component| component.parent().unwrap_or(component).to_path_buf(),
        )
}

fn follow(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["root"])?;
    let [file, offset] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let file = absolute(file)?;
    let offset: usize = offset
        .parse()
        .map_err(|_| format!("`{offset}` is not a byte offset"))?;
    let root = match option(&options, "root") {
        Some(root) => absolute(root)?,
        None => default_root(&file),
    };

    let mut engine = open_engine(&root, false)?;
    let id = engine.open(&file).map_err(|error| error.to_string())?;
    match engine.invoke(
        ids::NAVIGATE_FOLLOW_LINK,
        CommandArgs::Offset { id, offset },
    ) {
        Ok(CommandOutcome::Location(location)) => {
            let text = std::fs::read_to_string(&location.path).unwrap_or_default();
            let (line, column) =
                LineIndex::new(&text).line_character(&text, location.range.start);
            outln!(
                out,
                "{}:{}:{}",
                display_path(&location.path, &root),
                line + 1,
                column + 1
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(CommandOutcome::External(path)) => {
            outln!(out, "external {}", display_path(&path, &root));
            Ok(ExitCode::SUCCESS)
        }
        // An image opens in an image tab, where the app shows it; here it is a path like any
        // attachment's.
        Ok(CommandOutcome::OpenedImage(image)) => {
            let path = engine
                .image(image)
                .map(|image| image.path.clone())
                .ok_or_else(|| format!("unexpected outcome {image:?}"))?;
            outln!(out, "external {}", display_path(&path, &root));
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
fn render(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let link_images = arguments.iter().any(|argument| argument == "--link-images");
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|argument| *argument != "--link-images")
        .cloned()
        .collect();
    let (positional, options) = parse_options(&arguments, &["root", "output", "parser"])?;
    let [file] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let parser: std::sync::Arc<dyn adoc_dom::DocumentParser> =
        match option(&options, "parser").unwrap_or("acdc") {
            "acdc" => std::sync::Arc::new(adoc_dom::AcdcParser),
            "asciidoc-parser" => std::sync::Arc::new(adoc_dom::AsciidocParser),
            other => return Err(format!("unknown parser `{other}`")),
        };
    let file = absolute(file)?;
    let root = match option(&options, "root") {
        Some(root) => absolute(root)?,
        None => default_root(&file),
    };

    let mut engine = open_engine(&root, false)?;
    engine.set_document_parser(parser);
    let id = engine.open(&file).map_err(|error| error.to_string())?;
    let preview = engine
        .render_blocking(id, INDEX_TIMEOUT)
        .ok_or_else(|| format!("{} did not render", file.display()))?;
    let html = adoc_render::to_html(&preview.view, &engine.html_options(true));
    match option(&options, "output") {
        Some(output) => {
            // A file is meant to be moved or published, so it carries its images, as the
            // app's Export Page to HTML does; `--link-images` keeps their `file://` addresses.
            let html = if link_images {
                html
            } else {
                adoc_render::embed_local_files(&html)
            };
            std::fs::write(output, html).map_err(|error| format!("{output}: {error}"))?;
        }
        None => out!(out, "{html}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Exports a page, or a component version as a book, to PDF.
fn pdf(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
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
            "profile",
            "fail-on",
        ],
    )?;
    let fail_on = match option(&options, "fail-on").unwrap_or("none") {
        "none" => None,
        "warning" => Some(DiagnosticSeverity::Warning),
        "error" => Some(DiagnosticSeverity::Error),
        other => return Err(format!("unknown --fail-on level `{other}`")),
    };
    let standards = options
        .iter()
        .filter(|(name, _)| *name == "standard")
        .map(|(_, value)| {
            pdf_standard(value).ok_or_else(|| {
                format!(
                    "unknown standard `{value}`: expected pdf-a-1b, pdf-a-1a, pdf-a-2a or pdf-ua-1"
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(profile) = option(&options, "profile") {
        // A profile is exported as the app exports it; options given here win over it.
        let root = match (positional.as_slice(), option(&options, "root")) {
            ([], None) => PathBuf::from("."),
            ([root], None) => PathBuf::from(root),
            ([], Some(root)) => PathBuf::from(root),
            _ => return Err(USAGE.to_owned()),
        };
        let root = adoc_index::normalize_path(&root);
        let mut engine = open_engine(&root, false)?;
        let mut args = engine
            .profile_export_args(profile)
            .map_err(|error| error.to_string())?;
        if let Some(output) = option(&options, "output") {
            args.output = PathBuf::from(output);
        }
        if let Some(theme) = option(&options, "theme") {
            args.theme = Some(theme.to_owned());
        }
        if let Some(fonts_dir) = option(&options, "fonts-dir") {
            args.fonts_dir = Some(PathBuf::from(fonts_dir));
        }
        for standard in standards {
            if !args.standards.contains(&standard) {
                args.standards.push(standard);
            }
        }
        args.typst = option(&options, "typst").map(PathBuf::from);
        return report_export(&mut engine, args, &root, fail_on, out);
    }
    let [target] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    // A page's PDF goes beside it, named as the page was.
    let beside = Path::new(target).with_extension("pdf");
    let target = absolute(target)?;
    let is_dir = target.is_dir();
    let root = match option(&options, "root") {
        Some(root) => absolute(root)?,
        // A component directory: its parent, so sibling components resolve.
        None if is_dir && target.join("antora.yml").is_file() => {
            target.parent().unwrap_or(&target).to_path_buf()
        }
        None if is_dir => target.clone(),
        None => default_root(&target),
    };

    let mut engine = open_engine(&root, false)?;
    // The component version to export, `None` its version for one unversioned (`~`).
    let component = match option(&options, "component") {
        Some(spec) => Some(match spec.split_once('@') {
            Some((version, name)) => (name.to_owned(), Some(version.to_owned())),
            // No version: the latest, as Antora reads a coordinate without one.
            None => (
                spec.to_owned(),
                engine
                    .workspace()
                    .antora
                    .latest_version(spec)
                    .and_then(|latest| latest.version.clone()),
            ),
        }),
        // A folder: the version it holds, never the latest in its place (#511).
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
            let output = PathBuf::from(pdf_file_name(&name));
            (ExportTarget::Component { name, version }, output)
        }
        None => {
            let id = engine.open(&target).map_err(|error| error.to_string())?;
            (ExportTarget::Page(id), beside)
        }
    };
    let output = option(&options, "output").map_or(default_output, PathBuf::from);
    let args = ExportArgs {
        target: export_target,
        output,
        theme: option(&options, "theme").map(str::to_owned),
        standards,
        fonts_dir: option(&options, "fonts-dir").map(PathBuf::from),
        typst: option(&options, "typst").map(PathBuf::from),
        attributes: Vec::new(),
    };
    report_export(&mut engine, args, &root, fail_on, out)
}

/// The file a component named `name` is exported to by default, in the current directory: the
/// name's last part, with anything but letters, digits, `-`, `_` and `.` made `-`, so the
/// workspace's content never decides where the file goes (#511).
fn pdf_file_name(name: &str) -> String {
    let last = name
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or_default();
    let safe: String = last
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if safe.chars().all(|character| character == '.') {
        "component.pdf".to_owned()
    } else {
        format!("{safe}.pdf")
    }
}

/// Exports and waits, printing each diagnostic and then the output and its page count. Fails
/// when a diagnostic is at least as severe as `fail_on`, though the PDF is written.
fn report_export(
    engine: &mut Engine,
    args: ExportArgs,
    root: &Path,
    fail_on: Option<DiagnosticSeverity>,
    out: &mut Out,
) -> Result<ExitCode, String> {
    let output = args.output.clone();
    match engine.export_blocking(args, INDEX_TIMEOUT) {
        Ok(summary) => {
            for diagnostic in &summary.diagnostics {
                match &diagnostic.location {
                    Some((path, line)) => eprintln!(
                        "{}:{line}: {}: {}",
                        display_path(path, root),
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
            outln!(out, "{} ({} pages)", output.display(), summary.pages);
            let failed = fail_on.is_some_and(|threshold| {
                summary.diagnostics.iter().any(|diagnostic| {
                    threshold == DiagnosticSeverity::Warning
                        || diagnostic.severity == DiagnosticSeverity::Error
                })
            });
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Err(error) => {
            eprintln!("adocfoundry-cli: {error}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The branch of the repository holding `root`, and its changed AsciiDoc files with the number
/// of changed line runs in each.
fn git_status(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
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
        outln!(out, "{output}");
    } else {
        outln!(out, "branch {}", report.head);
        for file in &report.files {
            let hunks = file
                .hunks
                .map_or_else(String::new, |count| format!(" ({count} changed)"));
            outln!(out, "{} {}{hunks}", short_status(file), file.path);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The link graph of the workspace at `root`, of a page and its neighbours, or along the
/// shortest path between two pages, as JSON, DOT or an SVG image.
fn graph(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    // Flags take no value: take them out before the options.
    let flags = [
        "--no-partials",
        "--no-nav",
        "--unresolved",
        "--dark",
        "--undirected",
        "--ages",
    ];
    let flag = |name: &str| arguments.iter().any(|argument| argument == name);
    let mut rest: Vec<String> = arguments
        .iter()
        .filter(|argument| !flags.contains(&argument.as_str()))
        .cloned()
        .collect();
    // `--path` takes two values.
    let ends = match rest.iter().position(|argument| argument == "--path") {
        Some(at) => {
            let values: Vec<String> = rest.drain(at..(at + 3).min(rest.len())).skip(1).collect();
            let [from, to] = values.as_slice() else {
                return Err("`--path` needs the two pages to join".to_owned());
            };
            Some((from.clone(), to.clone()))
        }
        None => None,
    };
    let (positional, options) = parse_options(&rest, &["page", "depth", "format"])?;
    let [root] = positional.as_slice() else {
        return Err(USAGE.to_owned());
    };
    let format = option(&options, "format").unwrap_or("json");
    if !["json", "dot", "svg"].contains(&format) {
        return Err(format!("unknown format `{format}`"));
    }
    let depth = match option(&options, "depth") {
        None => 1,
        Some(_) if option(&options, "page").is_none() => {
            return Err("`--depth` needs `--page`".to_owned())
        }
        Some(depth) => depth
            .parse::<u8>()
            .ok()
            .filter(|depth| (1..=3).contains(depth))
            .ok_or_else(|| format!("`--depth` is 1, 2 or 3, not `{depth}`"))?,
    };
    let root = adoc_index::normalize_path(Path::new(root));
    let scope = match option(&options, "page") {
        // Written as the root is: the index holds paths as the root gives them.
        Some(page) => GraphScope::Local {
            path: beneath(&root, page)?,
            depth,
        },
        None => GraphScope::Workspace,
    };
    let filter = GraphFilter {
        partials: !flag("--no-partials"),
        nav: !flag("--no-nav"),
        unresolved: flag("--unresolved"),
        ..GraphFilter::default()
    };
    // Ages come from Git history, read once the repository is found.
    let ages = flag("--ages");
    let mut engine = open_engine(&root, ages)?;
    if ages {
        for _ in 0..3 {
            engine.pump();
            if !engine.wait_for_workers(INDEX_TIMEOUT) {
                return Err("reading the Git history did not finish".to_owned());
            }
        }
        engine.pump();
    }
    if let Some((from, to)) = ends {
        if option(&options, "page").is_some() {
            return Err("`--path` and `--page` cannot be used together".to_owned());
        }
        // Written as the root is, like `--page`.
        let (from, to) = (beneath(&root, &from)?, beneath(&root, &to)?);
        for page in [&from, &to] {
            if engine.workspace().index.file(page).is_none() {
                return Err(format!(
                    "{} is not a page in {}",
                    page.display(),
                    root.display()
                ));
            }
        }
        let directed = !flag("--undirected");
        let Some(report) = engine.graph_path_report(&root, &filter, &from, &to, directed) else {
            eprintln!(
                "adocfoundry-cli: no path from {} to {}",
                display_path(&from, &root),
                display_path(&to, &root)
            );
            return Ok(ExitCode::from(1));
        };
        match format {
            "svg" => {
                let style = SvgStyle {
                    colour_by: engine.settings().graph.colour_by,
                    dark: flag("--dark"),
                    groups: engine.graph_colour_groups(engine.settings().graph.colour_by),
                };
                let (snapshot, _) = engine
                    .graph_path_view(&filter, &from, &to, directed)
                    .unwrap_or_default();
                out!(out, "{}", adoc_engine::to_svg_laid_out(&snapshot, &style));
            }
            "dot" => out!(out, "{}", report.to_dot()),
            _ => outln!(
                out,
                "{}",
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
            ),
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let GraphScope::Local { path, .. } = &scope {
        if engine.workspace().index.file(path).is_none() {
            return Err(format!(
                "{} is not a page in {}",
                path.display(),
                root.display()
            ));
        }
    }
    if format == "svg" {
        let style = SvgStyle {
            colour_by: engine.settings().graph.colour_by,
            dark: flag("--dark"),
            groups: engine.graph_colour_groups(engine.settings().graph.colour_by),
        };
        out!(out, "{}", engine.graph_svg(&scope, &filter, &style));
        return Ok(ExitCode::SUCCESS);
    }
    let report = engine.graph_report(&root, &scope, &filter);
    if format == "dot" {
        out!(out, "{}", report.to_dot());
    } else {
        let output = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        outln!(out, "{output}");
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

/// The doctor report, for support requests: with the user settings file the app reads, and
/// `root`'s workspace settings when given.
fn doctor(arguments: &[String], out: &mut Out) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(arguments, &["format"])?;
    let json = match option(&options, "format").unwrap_or("text") {
        "text" => false,
        "json" => true,
        other => return Err(format!("unknown format `{other}`")),
    };
    let user_file = Settings::default_user_file();
    let settings = Settings::load(user_file.as_deref(), None).map_err(|error| error.to_string())?;
    let mut engine = Engine::new(settings);
    match positional.as_slice() {
        [] => {}
        [root] => engine
            .open_workspace(vec![adoc_index::normalize_path(Path::new(root))])
            .map_err(|error| error.to_string())?,
        _ => return Err(USAGE.to_owned()),
    }
    let report = engine
        .doctor()
        .program("adocfoundry-cli", env!("CARGO_PKG_VERSION"))
        .run();
    if json {
        let output = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        outln!(out, "{output}");
    } else {
        out!(out, "{}", report.to_text());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening the workspace reads its settings again, which must leave Git off: the audit
    /// reads nothing of it, so it neither finds the repository nor waits for its workers (#508).
    #[test]
    fn the_audit_looks_at_no_repository() {
        let directory = tempfile::tempdir().unwrap();
        let repository = adoc_git::testing::TestRepo::init(directory.path());
        repository.write("page.adoc", "= Page\n");
        repository.commit_all("page");

        let root = directory.path();
        let engine = open_engine_with(root, audit_settings(root).unwrap()).unwrap();
        assert!(engine.git().repositories().is_empty());
    }

    #[test]
    fn a_component_name_gives_a_file_name_in_the_current_directory() {
        for (name, file) in [
            ("guide", "guide.pdf"),
            ("api/v2", "v2.pdf"),
            ("../../escaped", "escaped.pdf"),
            ("/etc/passwd", "passwd.pdf"),
            ("C:\\Windows\\x", "x.pdf"),
            ("a b:c", "a-b-c.pdf"),
            ("v1.0_beta-2", "v1.0_beta-2.pdf"),
            ("..", "component.pdf"),
        ] {
            assert_eq!(pdf_file_name(name), file, "{name}");
        }
    }
}
