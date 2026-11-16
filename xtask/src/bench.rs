//! `cargo xtask bench <tier>`: the engine's benchmarks, judged for one of three tiers (#612).
//!
//! `crates/adoc-engine/benches/budgets.rs` measures and writes what it measured to the file
//! `BENCH_RESULTS` names; this module reads that and judges it, so every tier judges the same
//! numbers the same way.
//!
//! - **CI** ([`Tier::Ci`]): a bench fails only past [`CI_CEILING`] times its budget, and the
//!   benches timed at several sizes must grow no faster than [`GROWTH`] allows. A shared runner's
//!   speed varies by about 1.5× from run to run; a slower runner slows every size alike, so the
//!   growth barely moves, while a path that has turned quadratic shows on any runner.
//! - **Release** ([`Tier::Release`]): every budget as written, and the growth, on a quiet machine
//!   before a release is tagged.
//! - **Development** ([`compare`]): each bench against a baseline measured on the same machine
//!   just before, failing past a margin.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// What a bench must stay within, as the bench wrote it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Budget {
    /// Recorded only.
    None,
    /// The median, in nanoseconds.
    Median(f64),
    /// The median per megabyte of input, in nanoseconds.
    PerMegabyte(f64),
    /// Memory added, as a multiple of the input's bytes.
    Ratio(f64),
}

/// What a bench measured.
#[derive(Clone, Debug, PartialEq)]
pub enum Measure {
    /// Timings in nanoseconds, of an input of `bytes` (0 when the size is not in bytes).
    Time { median: f64, worst: f64, bytes: u64 },
    /// Resident memory `added` while building something from `bytes` of input.
    Memory { added: u64, bytes: u64 },
}

/// One row the bench wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub bench: String,
    pub size: String,
    pub measure: Measure,
    pub budget: Budget,
}

/// Which budgets a run is held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Ci,
    Release,
}

/// How far past its budget a timed bench may go in CI before it fails.
pub const CI_CEILING: f64 = 3.0;

/// The benches timed at several sizes, and the most their median may grow with size, as the
/// power of the size: 1 is linear. Between the smallest size and the largest, `parse_whole` is
/// linear (n^1.00 on every runner so far) and a keystroke nearly constant (n^0.13).
pub const GROWTH: &[(&str, f64)] = &[
    ("parse_whole", 1.25),
    ("keystroke_insert", 0.6),
    ("keystroke_backspace", 0.6),
];

/// A change smaller than this is not a slowdown, whatever its percentage: a keystroke's median
/// is under a microsecond, where a tenth is noise on any machine.
pub const COMPARE_FLOOR_NS: f64 = 1_000.0;

/// One line of a judged table.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub bench: String,
    pub size: String,
    pub measured: String,
    pub worst: String,
    pub limit: String,
    pub over: bool,
}

/// Reads the rows `budgets.rs` writes, one per line, fields separated by tabs:
///
/// - `time  <bench>  <size>  <median ns>  <worst ns>  <bytes>  <budget>`
/// - `memory  <bench>  <size>  <added bytes>  <input bytes>  <budget>`
///
/// where `<budget>` is `none`, `median:<ns>`, `per_mb:<ns>` or `ratio:<multiple>`.
pub fn parse(text: &str) -> Result<Vec<Measured>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| measured(line).ok_or_else(|| format!("line {}: `{line}`", index + 1)))
        .collect()
}

fn measured(line: &str) -> Option<Measured> {
    let fields: Vec<&str> = line.split('\t').collect();
    let (measure, budget) = match fields.as_slice() {
        ["time", _, _, median, worst, bytes, budget] => (
            Measure::Time {
                median: number(median)?,
                worst: number(worst)?,
                bytes: bytes.parse().ok()?,
            },
            budget,
        ),
        ["memory", _, _, added, bytes, budget] => (
            Measure::Memory {
                added: added.parse().ok()?,
                bytes: bytes.parse().ok()?,
            },
            budget,
        ),
        _ => return None,
    };
    let budget = match budget.split_once(':') {
        None if *budget == "none" => Budget::None,
        Some(("median", value)) => Budget::Median(number(value)?),
        Some(("per_mb", value)) => Budget::PerMegabyte(number(value)?),
        Some(("ratio", value)) => Budget::Ratio(number(value)?),
        _ => return None,
    };
    Some(Measured {
        bench: fields[1].to_owned(),
        size: fields[2].to_owned(),
        measure,
        budget,
    })
}

fn number(text: &str) -> Option<f64> {
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// Judges every row for `tier`, then the growth of the benches in [`GROWTH`].
pub fn judge(results: &[Measured], tier: Tier) -> Vec<Verdict> {
    let mut verdicts: Vec<Verdict> = results.iter().map(|result| held(result, tier)).collect();
    verdicts.extend(growth(results));
    verdicts
}

/// One row against its budget, loosened to [`CI_CEILING`] for a time in CI.
fn held(result: &Measured, tier: Tier) -> Verdict {
    let factor = match tier {
        Tier::Ci => CI_CEILING,
        Tier::Release => 1.0,
    };
    let limit = |shown: String, limit: String| {
        if factor == 1.0 {
            format!("≤ {limit}")
        } else {
            format!("≤ {shown} ({factor:.0} × {limit})")
        }
    };
    let (measured, worst, limit, over) = match (&result.measure, result.budget) {
        (Measure::Time { median, worst, .. }, Budget::Median(budget)) => (
            duration(*median),
            duration(*worst),
            limit(duration(budget * factor), duration(budget)),
            *median > budget * factor,
        ),
        (
            Measure::Time {
                median,
                worst,
                bytes,
            },
            Budget::PerMegabyte(budget),
        ) => {
            let megabytes = *bytes as f64 / 1e6;
            (
                duration(*median),
                duration(*worst),
                limit(
                    format!("{}/MB", duration(budget * factor)),
                    format!("{}/MB", duration(budget)),
                ),
                megabytes > 0.0 && *median / megabytes > budget * factor,
            )
        }
        (Measure::Time { median, worst, .. }, _) => (
            duration(*median),
            duration(*worst),
            "record".to_owned(),
            false,
        ),
        (Measure::Memory { added, bytes }, budget) => {
            let ratio = *added as f64 / (*bytes).max(1) as f64;
            let (limit, over) = match budget {
                Budget::Ratio(limit) => (format!("≤ {limit:.1} × input"), ratio > limit),
                _ => ("record".to_owned(), false),
            };
            (
                format!("{} ({ratio:.2} × input)", megabytes(*added)),
                String::new(),
                limit,
                over,
            )
        }
    };
    Verdict {
        bench: result.bench.clone(),
        size: result.size.clone(),
        measured,
        worst,
        limit,
        over,
    }
}

/// How each bench in [`GROWTH`] grows between its smallest and its largest size.
fn growth(results: &[Measured]) -> Vec<Verdict> {
    GROWTH
        .iter()
        .filter_map(|(bench, most)| {
            let mut sizes: Vec<(f64, &Measured, f64)> = results
                .iter()
                .filter(|result| result.bench == *bench)
                .filter_map(|result| match result.measure {
                    Measure::Time { median, .. } => {
                        Some((leading_number(&result.size)?, result, median))
                    }
                    Measure::Memory { .. } => None,
                })
                .collect();
            sizes.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (small, large) = (sizes.first()?, sizes.last()?);
            if large.0 <= small.0 || small.2 <= 0.0 {
                return None;
            }
            let times = large.2 / small.2;
            let power = times.ln() / (large.0 / small.0).ln();
            Some(Verdict {
                bench: format!("{bench} growth"),
                size: format!("{} → {}", small.1.size, large.1.size),
                measured: format!("× {times:.1} (n^{power:.2})"),
                worst: String::new(),
                limit: format!("≤ n^{most:.2}"),
                over: power > *most,
            })
        })
        .collect()
}

/// The number a size such as `20000 lines` starts with.
fn leading_number(size: &str) -> Option<f64> {
    let digits: String = size.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Each bench of `current` against the same bench and size in `baseline`: over when it is more
/// than `margin` (0.1 for 10%) slower, and by more than [`COMPARE_FLOOR_NS`].
pub fn compare(baseline: &[Measured], current: &[Measured], margin: f64) -> Vec<Verdict> {
    current
        .iter()
        .map(|now| {
            let before = baseline
                .iter()
                .find(|before| before.bench == now.bench && before.size == now.size);
            let limit = |change: f64| {
                // Rounded first, so a change too small to show is `+0%`, never `-0%`.
                let percent = (change * 100.0).round() + 0.0;
                let sign = if percent >= 0.0 { "+" } else { "" };
                format!("{sign}{percent:.0}% (≤ +{:.0}%)", margin * 100.0)
            };
            let (measured, worst, limit, over) =
                match (before.map(|before| &before.measure), &now.measure) {
                    (
                        Some(Measure::Time { median: was, .. }),
                        Measure::Time { median, worst, .. },
                    ) => {
                        let change = (median - was) / was;
                        (
                            format!("{} → {}", duration(*was), duration(*median)),
                            duration(*worst),
                            limit(change),
                            change > margin && median - was > COMPARE_FLOOR_NS,
                        )
                    }
                    (Some(Measure::Memory { added: was, .. }), Measure::Memory { added, .. }) => {
                        let change = (*added as f64 - *was as f64) / (*was).max(1) as f64;
                        (
                            format!("{} → {}", megabytes(*was), megabytes(*added)),
                            String::new(),
                            limit(change),
                            change > margin,
                        )
                    }
                    (_, Measure::Time { median, worst, .. }) => {
                        (duration(*median), duration(*worst), "new".to_owned(), false)
                    }
                    (_, Measure::Memory { added, .. }) => {
                        (megabytes(*added), String::new(), "new".to_owned(), false)
                    }
                };
            Verdict {
                bench: now.bench.clone(),
                size: now.size.clone(),
                measured,
                worst,
                limit,
                over,
            }
        })
        .collect()
}

/// The verdicts as a Markdown table, `**over**` marking each one over its limit.
pub fn table(verdicts: &[Verdict]) -> String {
    let mut table =
        String::from("| Bench | Size | Measured | Worst | Limit |\n|---|---|---|---|---|\n");
    for verdict in verdicts {
        let _ = writeln!(
            table,
            "| `{}` | {} | {} | {} | {}{} |",
            verdict.bench,
            verdict.size,
            verdict.measured,
            verdict.worst,
            verdict.limit,
            if verdict.over { " **over**" } else { "" }
        );
    }
    table
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// What `cargo xtask bench` was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// `ci`: the engine's benches for [`Tier::Ci`].
    Ci,
    /// `dev [--against <ref>] [--only <text>] [--margin <percent>]`: the benches whose names hold
    /// `only`, here and at `against`, compared.
    Dev {
        against: String,
        only: Option<String>,
        margin: f64,
    },
    /// `release [--skip-typing] [--corpus <folder>]`: the engine's benches for
    /// [`Tier::Release`], then the graph's, the timing probes, the typing run and, given a
    /// folder of documentation, a check of it, timed.
    Release {
        typing: bool,
        corpus: Option<PathBuf>,
    },
}

/// The usage `cargo xtask bench` prints when it cannot read its arguments.
pub const USAGE: &str = "usage: cargo xtask bench ci\n       cargo xtask bench dev [--against <ref>] [--only <text>] [--margin <percent>]\n       cargo xtask bench release [--skip-typing] [--corpus <folder>]";

/// Reads the arguments after `bench`.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Request, String> {
    let mut args = args.into_iter();
    let tier = args.next().ok_or_else(|| USAGE.to_owned())?;
    let mut request = match tier.as_str() {
        "ci" => Request::Ci,
        "dev" => Request::Dev {
            against: "main".to_owned(),
            only: None,
            margin: 0.10,
        },
        "release" => Request::Release {
            typing: true,
            corpus: None,
        },
        other => return Err(format!("unknown tier `{other}`\n{USAGE}")),
    };
    while let Some(option) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{option} needs a value\n{USAGE}"))
        };
        match (&mut request, option.as_str()) {
            (Request::Dev { against, .. }, "--against") => *against = value()?,
            (Request::Dev { only, .. }, "--only") => *only = Some(value()?),
            (Request::Dev { margin, .. }, "--margin") => {
                let text = value()?;
                *margin = text
                    .parse::<f64>()
                    .ok()
                    .filter(|percent| percent.is_finite() && *percent >= 0.0)
                    .ok_or_else(|| format!("--margin takes a percentage, not `{text}`"))?
                    / 100.0;
            }
            (Request::Release { typing, .. }, "--skip-typing") => *typing = false,
            (Request::Release { corpus, .. }, "--corpus") => {
                *corpus = Some(PathBuf::from(value()?));
            }
            _ => return Err(format!("`{option}` is not an option of `{tier}`\n{USAGE}")),
        }
    }
    Ok(request)
}

/// Runs `request` from the workspace at `root`; `Ok(false)` when something is over its limit
/// or a step failed.
pub fn run(root: &Path, cargo: &OsStr, request: &Request) -> Result<bool, String> {
    let work = root.join("target/bench");
    fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    match request {
        Request::Ci => {
            let results = engine(root, cargo, None, &work.join("ci.tsv"))?;
            Ok(report(
                "Engine benchmarks, CI tier",
                &judge(&results, Tier::Ci),
            ))
        }
        Request::Dev {
            against,
            only,
            margin,
        } => {
            let checkout = baseline_checkout(root, &work.join("baseline"), against)?;
            println!("bench: the baseline, at {against}");
            let baseline = engine(
                &checkout,
                cargo,
                only.as_deref(),
                &work.join("baseline.tsv"),
            )
            .map_err(|error| {
                format!("{error} (a baseline from before `cargo xtask bench` writes no results)")
            })?;
            println!("bench: this checkout");
            let current = engine(root, cargo, only.as_deref(), &work.join("current.tsv"))?;
            Ok(report(
                &format!("Engine benchmarks against {against}"),
                &compare(&baseline, &current, *margin),
            ))
        }
        Request::Release { typing, corpus } => {
            let results = engine(root, cargo, None, &work.join("release.tsv"))?;
            let mut passed = report(
                "Engine benchmarks, release tier",
                &judge(&results, Tier::Release),
            );
            let mut steps = Vec::new();
            let mut step = |name: &str, ok: bool, detail: String| {
                passed &= ok;
                steps.push(format!(
                    "- {name}: {} {detail}",
                    if ok { "ok" } else { "**failed**" }
                ));
            };
            let graph = cargo_status(root, cargo, &["bench", "-p", "adoc-graph"], &[])?;
            step("`cargo bench -p adoc-graph`", graph, String::new());
            let probes = cargo_status(
                root,
                cargo,
                &[
                    "test",
                    "--release",
                    "-p",
                    "adoc-engine",
                    "--test",
                    "perf",
                    "--",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ],
                &[],
            )?;
            step(
                "the timing and memory probes (`tests/perf.rs`)",
                probes,
                "(figures above)".to_owned(),
            );
            if *typing {
                let (ok, detail) = typing_run(root, cargo, &work)?;
                step("typing into a 1 MB page", ok, detail);
            }
            if let Some(corpus) = corpus {
                let (ok, detail) = corpus_check(root, cargo, corpus)?;
                step("checking the corpus", ok, detail);
            }
            let steps = steps.join("\n");
            println!("{steps}");
            summary(&format!("### Release checks\n\n{steps}\n"));
            Ok(passed)
        }
    }
}

/// Runs the engine's benches in the workspace at `dir`, those whose names hold `only`, and reads
/// what they measured from `results`.
fn engine(
    dir: &Path,
    cargo: &OsStr,
    only: Option<&str>,
    results: &Path,
) -> Result<Vec<Measured>, String> {
    let _ = fs::remove_file(results);
    let mut env = vec![("BENCH_RESULTS", results.as_os_str())];
    if let Some(only) = only {
        env.push(("BENCH_ONLY", OsStr::new(only)));
    }
    if !cargo_status(
        dir,
        cargo,
        &["bench", "-p", "adoc-engine", "--bench", "budgets"],
        &env,
    )? {
        return Err(format!("the engine's benches failed in {}", dir.display()));
    }
    let text = fs::read_to_string(results)
        .map_err(|_| format!("the engine's benches in {} wrote no results", dir.display()))?;
    parse(&text).map_err(|error| format!("{}: {error}", results.display()))
}

fn cargo_status(
    dir: &Path,
    cargo: &OsStr,
    args: &[&str],
    env: &[(&str, &OsStr)],
) -> Result<bool, String> {
    let mut command = Command::new(cargo);
    command.args(args).current_dir(dir);
    for (name, value) in env {
        command.env(name, value);
    }
    command
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("cargo {}: {error}", args.join(" ")))
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {} failed", args.join(" ")))
    }
}

/// A worktree at `against`, kept at `checkout` between runs so its build is reused.
fn baseline_checkout(root: &Path, checkout: &Path, against: &str) -> Result<PathBuf, String> {
    git(root, &["worktree", "prune"])?;
    let path = checkout.to_string_lossy();
    if checkout.join(".git").exists() {
        git(checkout, &["checkout", "--quiet", "--detach", against])?;
    } else {
        git(
            root,
            &["worktree", "add", "--quiet", "--detach", &path, against],
        )?;
    }
    // The app's grammar is a workspace member's dependency, so Cargo needs it to resolve the
    // workspace at all.
    git(checkout, &["submodule", "update", "--init", "--quiet"])?;
    Ok(checkout.to_path_buf())
}

/// The release app typing into a generated 1 MB page for twenty seconds, held to the frame
/// budget itself (no `ADOCFOUNDRY_TYPING_TIER`). It needs a display, and its window must stay
/// in view.
fn typing_run(root: &Path, cargo: &OsStr, work: &Path) -> Result<(bool, String), String> {
    if !cargo_status(
        root,
        cargo,
        &["build", "--release", "-p", "adocfoundry"],
        &[],
    )? {
        return Ok((false, "(the app did not build)".to_owned()));
    }
    let folder = work.join("typing");
    fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let page = folder.join("page.adoc");
    fs::write(&page, long_page()).map_err(|error| format!("{}: {error}", page.display()))?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let app = target.join(format!(
        "release/adocfoundry{}",
        std::env::consts::EXE_SUFFIX
    ));
    let mut child = Command::new(&app)
        .arg(&folder)
        .arg(&page)
        .env("ADOCFOUNDRY_TYPING_BENCH", "20")
        .env_remove("ADOCFOUNDRY_TYPING_TIER")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{}: {error}", app.display()))?;
    let stderr = child.stderr.take().expect("piped");
    let lines = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .filter(|line| line.starts_with("typing benchmark"))
            .collect::<Vec<_>>()
    });
    // The app quits itself once the benchmark ends.
    let deadline = Instant::now() + Duration::from_secs(300);
    while child
        .try_wait()
        .map_err(|error| error.to_string())?
        .is_none()
    {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok((
                false,
                "(the app did not finish within 5 minutes)".to_owned(),
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let lines = lines.join().unwrap_or_default();
    for line in &lines {
        println!("{line}");
    }
    let verdict = lines
        .iter()
        .find_map(|line| line.strip_prefix("typing benchmark budget: "))
        .unwrap_or("no result");
    Ok((verdict.starts_with("within:"), format!("({verdict})")))
}

/// About 1 MB of sections and paragraphs, as CI's `typing` job types into.
fn long_page() -> String {
    let mut text = String::from("= A long page\n\n");
    let mut paragraph = 0;
    while text.len() < 1_000_000 {
        paragraph += 1;
        if paragraph % 40 == 1 {
            let _ = write!(text, "== Section {}\n\n", paragraph / 40 + 1);
        }
        let _ = write!(
            text,
            "Paragraph {paragraph} with *bold*, _emphasis_, `code` and xref:other.adoc[a link].\n\n"
        );
    }
    text
}

/// `adocfoundry-cli check` over a folder of real documentation, timed and recorded.
fn corpus_check(root: &Path, cargo: &OsStr, corpus: &Path) -> Result<(bool, String), String> {
    if !corpus.is_dir() {
        return Err(format!("{}: not a folder", corpus.display()));
    }
    if !cargo_status(
        root,
        cargo,
        &["build", "--release", "-p", "adocfoundry-cli"],
        &[],
    )? {
        return Ok((false, "(the command line did not build)".to_owned()));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let cli = target.join(format!(
        "release/adocfoundry-cli{}",
        std::env::consts::EXE_SUFFIX
    ));
    let started = Instant::now();
    let status = Command::new(&cli)
        .arg("check")
        .arg(corpus)
        .stdout(Stdio::null())
        .status()
        .map_err(|error| format!("{}: {error}", cli.display()))?;
    let took = started.elapsed();
    // Its warnings are the corpus's own; only a run that could not finish fails here.
    let finished = matches!(status.code(), Some(0 | 1));
    Ok((
        finished,
        format!(
            "({} in {:.2} s, exit {})",
            corpus.display(),
            took.as_secs_f64(),
            status.code().unwrap_or(-1)
        ),
    ))
}

/// Prints the verdicts and adds them to the run's summary; whether none is over.
fn report(title: &str, verdicts: &[Verdict]) -> bool {
    let table = table(verdicts);
    println!("{title}\n\n{table}");
    summary(&format!("### {title}\n\n{table}\n"));
    let over: Vec<String> = verdicts
        .iter()
        .filter(|verdict| verdict.over)
        .map(|verdict| format!("{} ({})", verdict.bench, verdict.size))
        .collect();
    if !over.is_empty() {
        eprintln!("over: {}", over.join(", "));
    }
    over.is_empty()
}

/// Appends `text` to `$GITHUB_STEP_SUMMARY` when CI sets it.
fn summary(text: &str) {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let _ = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()));
    }
}

/// A duration in nanoseconds as the bench shows one.
fn duration(nanos: f64) -> String {
    let micros = nanos / 1_000.0;
    if micros < 10.0 {
        format!("{micros:.2} µs")
    } else if micros < 1_000.0 {
        format!("{micros:.0} µs")
    } else {
        format!("{:.2} ms", micros / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(bench: &str, size: &str, median_ns: f64, budget: Budget) -> Measured {
        Measured {
            bench: bench.to_owned(),
            size: size.to_owned(),
            measure: Measure::Time {
                median: median_ns,
                worst: median_ns * 1.1,
                bytes: 0,
            },
            budget,
        }
    }

    fn sized(bench: &str, lines: usize, median_ns: f64) -> Measured {
        time(bench, &format!("{lines} lines"), median_ns, Budget::None)
    }

    fn over(verdicts: &[Verdict]) -> Vec<String> {
        verdicts
            .iter()
            .filter(|verdict| verdict.over)
            .map(|verdict| format!("{} {}", verdict.bench, verdict.size))
            .collect()
    }

    const MS: f64 = 1_000_000.0;
    const US: f64 = 1_000.0;

    #[test]
    fn reads_what_the_bench_writes() {
        let text = "time\tparse_whole\t1000 lines\t267000\t284000\t65000\tper_mb:30000000\n\
                    time\tsettle_git\t20000 lines\t124000.5\t142000\t0\tmedian:1000000\n\
                    time\tbuild_visual\t5000 lines\t10010000\t10630000\t0\tnone\n\
                    memory\tindex_memory\t10000 × 8 KB pages\t189700000\t94000000\tratio:2.5\n";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                Measured {
                    bench: "parse_whole".to_owned(),
                    size: "1000 lines".to_owned(),
                    measure: Measure::Time {
                        median: 267_000.0,
                        worst: 284_000.0,
                        bytes: 65_000
                    },
                    budget: Budget::PerMegabyte(30.0 * MS),
                },
                Measured {
                    bench: "settle_git".to_owned(),
                    size: "20000 lines".to_owned(),
                    measure: Measure::Time {
                        median: 124_000.5,
                        worst: 142_000.0,
                        bytes: 0
                    },
                    budget: Budget::Median(MS),
                },
                Measured {
                    bench: "build_visual".to_owned(),
                    size: "5000 lines".to_owned(),
                    measure: Measure::Time {
                        median: 10_010_000.0,
                        worst: 10_630_000.0,
                        bytes: 0
                    },
                    budget: Budget::None,
                },
                Measured {
                    bench: "index_memory".to_owned(),
                    size: "10000 × 8 KB pages".to_owned(),
                    measure: Measure::Memory {
                        added: 189_700_000,
                        bytes: 94_000_000
                    },
                    budget: Budget::Ratio(2.5),
                },
            ]
        );
    }

    #[test]
    fn a_line_it_cannot_read_is_named() {
        assert_eq!(
            parse("time\tparse_whole\t1000 lines\tsoon\t1\t0\tnone\n"),
            Err("line 1: `time\tparse_whole\t1000 lines\tsoon\t1\t0\tnone`".to_owned())
        );
        assert!(parse("time\tx\t1\t2\t3\t4\tmedian:\n").is_err());
        assert!(parse("weight\tx\t1\t2\t3\tnone\n").is_err());
        assert_eq!(parse("\n").unwrap(), vec![]);
    }

    #[test]
    fn ci_fails_a_bench_only_past_three_times_its_budget() {
        let results = [
            time("settle_git", "20000 lines", 2.0 * MS, Budget::Median(MS)),
            time("settle_index", "20000 lines", 3.5 * MS, Budget::Median(MS)),
            time("build_visual", "5000 lines", 900.0 * MS, Budget::None),
        ];
        assert_eq!(
            over(&judge(&results, Tier::Release)),
            ["settle_git 20000 lines", "settle_index 20000 lines"]
        );
        assert_eq!(
            over(&judge(&results, Tier::Ci)),
            ["settle_index 20000 lines"]
        );
        let ci = judge(&results, Tier::Ci);
        assert_eq!(ci[0].limit, "≤ 3.00 ms (3 × 1.00 ms)");
        assert_eq!(ci[0].measured, "2.00 ms");
        assert_eq!(ci[2].limit, "record");
        assert_eq!(judge(&results, Tier::Release)[0].limit, "≤ 1.00 ms");
    }

    #[test]
    fn a_budget_per_megabyte_is_held_per_megabyte() {
        let parse_whole = |median: f64| Measured {
            bench: "parse_whole".to_owned(),
            size: "20000 lines".to_owned(),
            measure: Measure::Time {
                median,
                worst: median,
                bytes: 2_000_000,
            },
            budget: Budget::PerMegabyte(30.0 * MS),
        };
        assert!(over(&judge(&[parse_whole(59.0 * MS)], Tier::Release)).is_empty());
        assert_eq!(
            over(&judge(&[parse_whole(61.0 * MS)], Tier::Release)).len(),
            1
        );
        assert!(over(&judge(&[parse_whole(150.0 * MS)], Tier::Ci)).is_empty());
        assert_eq!(over(&judge(&[parse_whole(190.0 * MS)], Tier::Ci)).len(), 1);
        assert_eq!(
            judge(&[parse_whole(59.0 * MS)], Tier::Ci)[0].limit,
            "≤ 90.00 ms/MB (3 × 30.00 ms/MB)"
        );
    }

    #[test]
    fn memory_is_held_to_its_budget_in_every_tier() {
        // A runner's speed does not change what indexing keeps.
        let memory = Measured {
            bench: "index_memory".to_owned(),
            size: "10000 × 8 KB pages".to_owned(),
            measure: Measure::Memory {
                added: 260_000_000,
                bytes: 100_000_000,
            },
            budget: Budget::Ratio(2.5),
        };
        for tier in [Tier::Ci, Tier::Release] {
            let verdicts = judge(std::slice::from_ref(&memory), tier);
            assert!(verdicts[0].over);
            assert_eq!(verdicts[0].measured, "260.0 MB (2.60 × input)");
            assert_eq!(verdicts[0].limit, "≤ 2.5 × input");
        }
    }

    #[test]
    fn growth_with_size_is_judged_whatever_the_runner_s_speed() {
        let parse = |scale: f64| {
            vec![
                sized("parse_whole", 1_000, 300.0 * US * scale),
                sized("parse_whole", 5_000, 1.5 * MS * scale),
                sized("parse_whole", 100_000, 30.0 * MS * scale),
            ]
        };
        // Linear, on a fast runner and on one half as fast again.
        for scale in [1.0, 1.5] {
            let verdicts = judge(&parse(scale), Tier::Ci);
            let growth = verdicts.last().unwrap();
            assert_eq!(growth.bench, "parse_whole growth");
            assert_eq!(growth.size, "1000 lines → 100000 lines");
            assert_eq!(growth.measured, "× 100.0 (n^1.00)");
            assert_eq!(growth.limit, "≤ n^1.25");
            assert!(!growth.over);
        }
        // Quadratic at the largest size: 100 times the size, 10,000 times the time.
        let mut quadratic = parse(1.0);
        quadratic[2] = sized("parse_whole", 100_000, 3_000.0 * MS);
        for tier in [Tier::Ci, Tier::Release] {
            assert_eq!(
                over(&judge(&quadratic, tier)),
                ["parse_whole growth 1000 lines → 100000 lines"]
            );
        }
    }

    #[test]
    fn a_keystroke_must_not_grow_with_the_page() {
        let keystrokes = |large: f64| {
            vec![
                sized("keystroke_insert", 1_000, 0.34 * US),
                sized("keystroke_insert", 100_000, large),
            ]
        };
        assert!(over(&judge(&keystrokes(0.62 * US), Tier::Ci)).is_empty());
        // Linear in the page: 100 times the size, 100 times the time.
        assert_eq!(
            over(&judge(&keystrokes(34.0 * US), Tier::Ci)),
            ["keystroke_insert growth 1000 lines → 100000 lines"]
        );
    }

    #[test]
    fn growth_needs_two_sizes() {
        let verdicts = judge(&[sized("parse_whole", 1_000, 300.0 * US)], Tier::Ci);
        assert_eq!(verdicts.len(), 1, "no growth line from one size");
    }

    #[test]
    fn a_change_is_over_past_the_margin_and_the_floor() {
        let baseline = [
            time("settle_git", "20000 lines", 100.0 * US, Budget::Median(MS)),
            time("parse_whole", "1000 lines", 300.0 * US, Budget::None),
            time("keystroke_insert", "1000 lines", 0.30 * US, Budget::None),
            time("quick_open", "20000 files, `a`", 4.0 * MS, Budget::None),
            time("settle_index", "20000 lines", 1.0 * MS, Budget::None),
        ];
        let current = [
            // 8% slower: within a 10% margin.
            time("settle_git", "20000 lines", 108.0 * US, Budget::Median(MS)),
            // 20% slower.
            time("parse_whole", "1000 lines", 360.0 * US, Budget::None),
            // Twice as slow, but by a fraction of a microsecond.
            time("keystroke_insert", "1000 lines", 0.60 * US, Budget::None),
            // Faster.
            time("quick_open", "20000 files, `a`", 3.0 * MS, Budget::None),
            // Unchanged, a hair faster: no `-0%`.
            time("settle_index", "20000 lines", 0.999 * MS, Budget::None),
            // Not in the baseline.
            time("graph_snapshot", "5000 pages", 2.0 * MS, Budget::None),
        ];
        let verdicts = compare(&baseline, &current, 0.10);
        assert_eq!(over(&verdicts), ["parse_whole 1000 lines"]);
        assert_eq!(verdicts[0].measured, "100 µs → 108 µs");
        assert_eq!(verdicts[0].limit, "+8% (≤ +10%)");
        assert_eq!(verdicts[1].limit, "+20% (≤ +10%)");
        assert_eq!(verdicts[3].limit, "-25% (≤ +10%)");
        assert_eq!(verdicts[4].limit, "+0% (≤ +10%)");
        assert_eq!(verdicts[5].measured, "2.00 ms");
        assert_eq!(verdicts[5].limit, "new");
    }

    #[test]
    fn memory_is_compared_by_the_bytes_added() {
        let memory = |added| Measured {
            bench: "index_memory".to_owned(),
            size: "10000 × 8 KB pages".to_owned(),
            measure: Measure::Memory {
                added,
                bytes: 100_000_000,
            },
            budget: Budget::Ratio(2.5),
        };
        let verdicts = compare(&[memory(200_000_000)], &[memory(230_000_000)], 0.10);
        assert_eq!(over(&verdicts), ["index_memory 10000 × 8 KB pages"]);
        assert_eq!(verdicts[0].measured, "200.0 MB → 230.0 MB");
    }

    fn args(text: &str) -> Result<Request, String> {
        parse_args(text.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn reads_each_tier_and_its_options() {
        assert_eq!(args("ci"), Ok(Request::Ci));
        assert_eq!(
            args("dev"),
            Ok(Request::Dev {
                against: "main".to_owned(),
                only: None,
                margin: 0.10,
            })
        );
        assert_eq!(
            args("dev --only parse --against v0.11.0 --margin 5"),
            Ok(Request::Dev {
                against: "v0.11.0".to_owned(),
                only: Some("parse".to_owned()),
                margin: 0.05,
            })
        );
        assert_eq!(
            args("release"),
            Ok(Request::Release {
                typing: true,
                corpus: None,
            })
        );
        assert_eq!(
            args("release --skip-typing --corpus ../docs"),
            Ok(Request::Release {
                typing: false,
                corpus: Some(PathBuf::from("../docs")),
            })
        );
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        for wrong in [
            "",
            "nightly",
            "ci --only parse",
            "dev --margin",
            "dev --margin ten",
            "dev --margin -5",
            "dev --against",
            "release --corpus",
            "release --only parse",
        ] {
            assert!(args(wrong).is_err(), "`{wrong}` is refused");
        }
    }

    #[test]
    fn the_table_marks_what_is_over() {
        let verdicts = judge(
            &[
                time("settle_git", "20000 lines", 2.0 * MS, Budget::Median(MS)),
                time("build_visual", "5000 lines", 10.0 * MS, Budget::None),
            ],
            Tier::Release,
        );
        assert_eq!(
            table(&verdicts),
            "| Bench | Size | Measured | Worst | Limit |\n|---|---|---|---|---|\n\
             | `settle_git` | 20000 lines | 2.00 ms | 2.20 ms | ≤ 1.00 ms **over** |\n\
             | `build_visual` | 5000 lines | 10.00 ms | 11.00 ms | record |\n"
        );
    }
}
