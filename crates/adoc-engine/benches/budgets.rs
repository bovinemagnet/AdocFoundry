//! Benchmarks with budgets: `cargo bench -p adoc-engine`. Each bench times an engine path on
//! the engine thread and fails the run when its median is over budget; a bench without a
//! budget is recorded only. `BENCH_RECORD=1` records every bench without failing. The table is
//! printed, and appended to `$GITHUB_STEP_SUMMARY` when CI sets it.
//!
//! The budgets are for a CI runner, a few times slower than a developer's machine, so a bench
//! well under its budget locally can still be near it there.

use std::{
    fmt::Write as _,
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use adoc_core::SourceRange;
use adoc_engine::{
    commands::ids, CommandArgs, DocumentId, Engine, ManualClock, Settings, GIT_SETTLE,
    INDEX_SETTLE, QUICK_OPEN_LIMIT, RECOVERY_SETTLE,
};

/// What a bench must stay within.
#[derive(Clone, Copy)]
enum Budget {
    /// Recorded only.
    None,
    /// The median of one run.
    Median(Duration),
    /// The median per megabyte of input.
    PerMegabyte(Duration),
    /// Resident memory added, as a multiple of the input's bytes.
    Bytes(f64),
}

struct Row {
    bench: &'static str,
    size: String,
    median: String,
    worst: String,
    budget: String,
    over: bool,
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    /// Times `run` `runs` times after one warm-up, and checks the median against `budget`.
    /// `bytes` is the size of the input, for a per-megabyte budget.
    fn time(
        &mut self,
        bench: &'static str,
        size: impl Into<String>,
        runs: usize,
        bytes: usize,
        budget: Budget,
        mut run: impl FnMut() -> Duration,
    ) {
        run();
        let mut samples: Vec<Duration> = (0..runs).map(|_| run()).collect();
        samples.sort();
        let median = samples[samples.len() / 2];
        let worst = *samples.last().unwrap();
        let megabytes = bytes as f64 / 1e6;
        let (shown, over) = match budget {
            Budget::None | Budget::Bytes(_) => ("record".to_owned(), false),
            Budget::Median(limit) => (format!("≤ {}", duration(limit)), median > limit),
            Budget::PerMegabyte(limit) => (
                format!("≤ {}/MB", duration(limit)),
                median.as_secs_f64() / megabytes > limit.as_secs_f64(),
            ),
        };
        self.rows.push(Row {
            bench,
            size: size.into(),
            median: duration(median),
            worst: duration(worst),
            budget: shown,
            over,
        });
    }

    /// Records the resident memory `added` while building something from `bytes` of input.
    fn memory(
        &mut self,
        bench: &'static str,
        size: String,
        added: u64,
        bytes: usize,
        budget: Budget,
    ) {
        let ratio = added as f64 / bytes as f64;
        let (shown, over) = match budget {
            Budget::Bytes(limit) => (format!("≤ {limit:.1} × input"), ratio > limit),
            _ => ("record".to_owned(), false),
        };
        self.rows.push(Row {
            bench,
            size,
            median: format!("{:.1} MB ({ratio:.2} × input)", added as f64 / 1e6),
            worst: String::new(),
            budget: shown,
            over,
        });
    }

    fn table(&self) -> String {
        let mut table =
            String::from("| Bench | Size | Median | Worst | Budget |\n|---|---|---|---|---|\n");
        for row in &self.rows {
            let _ = writeln!(
                table,
                "| `{}` | {} | {} | {} | {}{} |",
                row.bench,
                row.size,
                row.median,
                row.worst,
                row.budget,
                if row.over { " **over**" } else { "" }
            );
        }
        table
    }
}

fn duration(duration: Duration) -> String {
    let micros = duration.as_secs_f64() * 1e6;
    if micros < 10.0 {
        format!("{micros:.2} µs")
    } else if micros < 1_000.0 {
        format!("{micros:.0} µs")
    } else {
        format!("{:.2} ms", micros / 1_000.0)
    }
}

/// A document of about `lines` lines with sections, xrefs, attributes and blocks, as the
/// timing probes in `tests/perf.rs` use.
fn synthetic_document(lines: usize) -> String {
    let mut text = String::from("= Synthetic\n:product: Foundry\n\n");
    let mut section = 0;
    while text.lines().count() < lines {
        section += 1;
        let _ = write!(
            text,
            "== Section {section}\n\nSee xref:other.adoc#part-{section}[part {section}] and <<section-{}>>.\n\
             The {{product}} value is used here.\n\n[source,rust]\n----\nfn main() {{}}\n----\n\n\
             * item one\n* item two\n\n",
            section.max(2) - 1
        );
    }
    text
}

fn settings() -> Settings {
    Settings::load(None, None).unwrap().with_git_opt_in()
}

/// An engine on a manual clock with `text` open, in a workspace of its folder.
fn page_engine(dir: &Path, text: &str, settings: Settings) -> (Engine, ManualClock, DocumentId) {
    let path = dir.join("page.adoc");
    fs::write(&path, text).unwrap();
    let clock = ManualClock::new();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.open_workspace(vec![dir.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(60)));
    let id = engine.open(&path).unwrap();
    (engine, clock, id)
}

fn insert(engine: &mut Engine, id: DocumentId, at: usize, text: &str) -> Duration {
    let started = Instant::now();
    engine
        .invoke(
            ids::EDIT_INSERT_TEXT,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at),
                text: text.to_owned(),
            },
        )
        .unwrap();
    started.elapsed()
}

/// Indexes what typing left pending, as any command but typing does, so a settle times only
/// its own work.
fn flush(engine: &mut Engine) {
    let _ = engine.invoke(
        ids::WORKSPACE_QUICK_OPEN,
        CommandArgs::Query {
            query: String::new(),
            document: None,
        },
    );
}

fn backspace(engine: &mut Engine, id: DocumentId, at: usize) -> Duration {
    let started = Instant::now();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at - 1, at),
                text: String::new(),
            },
        )
        .unwrap();
    started.elapsed()
}

const SIZES: [usize; 4] = [1_000, 5_000, 20_000, 100_000];

fn keystrokes(report: &mut Report) {
    for lines in SIZES {
        let text = synthetic_document(lines);
        let runs = if lines >= 100_000 { 50 } else { 200 };
        let size = format!("{lines} lines");
        report.time(
            "parse_whole",
            size.clone(),
            runs.min(50),
            text.len(),
            Budget::PerMegabyte(Duration::from_millis(30)),
            || {
                let started = Instant::now();
                let _ = adoc_parser::parse("file:///page.adoc", &text);
                started.elapsed()
            },
        );

        let dir = tempfile::tempdir().unwrap();
        let (mut engine, _clock, id) = page_engine(dir.path(), &text, settings());
        // In a paragraph near the middle, as typing usually is.
        let at = text[text.len() / 2..].find("The ").unwrap() + text.len() / 2;
        let budget = if lines == 20_000 {
            Budget::Median(Duration::from_micros(100))
        } else {
            Budget::None
        };
        report.time(
            "keystroke_insert",
            size.clone(),
            runs,
            text.len(),
            budget,
            || insert(&mut engine, id, at, "x"),
        );
        // Each backspace takes out a character typed just before it, untimed.
        report.time(
            "keystroke_backspace",
            size,
            runs,
            text.len(),
            budget,
            || {
                insert(&mut engine, id, at, "x");
                backspace(&mut engine, id, at + 1)
            },
        );
    }
}

fn settles(report: &mut Report) {
    let text = synthetic_document(20_000);
    let size = "20000 lines".to_owned();
    let budget = Budget::Median(Duration::from_millis(1));

    let dir = tempfile::tempdir().unwrap();
    let (mut engine, clock, id) = page_engine(dir.path(), &text, settings());
    let at = text.len() / 2;
    let at = at + text[at..].find("The ").unwrap();
    // Parsing and diagnosing the page run on the engine thread, about 7 ms here: recorded until
    // they move to a worker.
    report.time(
        "settle_index",
        size.clone(),
        30,
        text.len(),
        Budget::None,
        || {
            insert(&mut engine, id, at, "x");
            clock.advance(INDEX_SETTLE);
            let started = Instant::now();
            engine.pump();
            started.elapsed()
        },
    );

    let dir = tempfile::tempdir().unwrap();
    let repo = adoc_git::testing::TestRepo::init(dir.path());
    repo.write("page.adoc", &text);
    repo.commit_all("first");
    let (mut engine, clock, id) =
        page_engine(dir.path(), &text, Settings::load(None, None).unwrap());
    report.time("settle_git", size, 30, text.len(), budget, || {
        insert(&mut engine, id, at, "x");
        flush(&mut engine);
        clock.advance(GIT_SETTLE);
        let started = Instant::now();
        engine.pump();
        let elapsed = started.elapsed();
        engine.wait_for_workers(Duration::from_secs(10));
        elapsed
    });

    let page = "A line of a long page, typed into and not yet saved.\n".repeat(20_000);
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut recovering = settings().with_state_dir(Some(state.path().to_path_buf()));
    recovering.git.enabled = false;
    let (mut engine, clock, id) = page_engine(dir.path(), &page, recovering);
    engine.enable_recovery();
    report.time(
        "settle_recovery",
        format!("{:.1} MB", page.len() as f64 / 1e6),
        30,
        page.len(),
        budget,
        || {
            insert(&mut engine, id, 0, "x");
            flush(&mut engine);
            clock.advance(RECOVERY_SETTLE);
            let started = Instant::now();
            engine.pump();
            let elapsed = started.elapsed();
            engine.wait_for_workers(Duration::from_secs(10));
            elapsed
        },
    );

    let text = synthetic_document(5_000);
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, _clock, id) = page_engine(dir.path(), &text, settings());
    report.time(
        "build_visual",
        "5000 lines",
        10,
        text.len(),
        Budget::None,
        || {
            let started = Instant::now();
            engine
                .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
                .unwrap();
            let elapsed = started.elapsed();
            engine
                .invoke(ids::VIEW_SHOW_SOURCE, CommandArgs::Document(id))
                .unwrap();
            elapsed
        },
    );
}

fn workspace(report: &mut Report) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("antora.yml"),
        "name: docs\nversion: '1.0'\n",
    )
    .unwrap();
    for module in 0..200 {
        let pages = dir.path().join(format!("modules/module-{module}/pages"));
        fs::create_dir_all(&pages).unwrap();
        for page in 0..100 {
            fs::write(
                pages.join(format!("topic-{page}.adoc")),
                format!("= Topic {page} of Module {module}\n"),
            )
            .unwrap();
        }
    }
    let mut engine = Engine::new(settings());
    engine
        .open_workspace(vec![dir.path().to_path_buf()])
        .unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(120)));
    // The issue's query is budgeted; a query of several words, slower, is recorded.
    for (query, budget) in [
        ("a", Budget::Median(Duration::from_millis(5))),
        ("topic 42", Budget::None),
    ] {
        report.time(
            "quick_open",
            format!("20000 files, `{query}`"),
            30,
            0,
            budget,
            || {
                let started = Instant::now();
                let _ = engine.quick_open(query, QUICK_OPEN_LIMIT);
                started.elapsed()
            },
        );
    }

    let dir = tempfile::tempdir().unwrap();
    for page in 0..100 {
        fs::write(dir.path().join(format!("page-{page}.adoc")), "= Page\n").unwrap();
    }
    for folder in 0..20_000 {
        fs::create_dir_all(dir.path().join(format!(
            "node_modules/package-{}/lib-{folder}",
            folder / 100
        )))
        .unwrap();
    }
    report.time(
        "workspace_open",
        "100 pages, 20000 ignored folders",
        10,
        0,
        Budget::Median(Duration::from_millis(50)),
        || {
            let mut engine = Engine::new(settings());
            let started = Instant::now();
            engine
                .open_workspace(vec![dir.path().to_path_buf()])
                .unwrap();
            let elapsed = started.elapsed();
            assert!(engine.wait_for_workers(Duration::from_secs(60)));
            elapsed
        },
    );
}

/// The process's resident memory, where the system says (Linux).
fn resident() -> Option<u64> {
    let statm = fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

fn index_memory(report: &mut Report) {
    let Some(before) = resident() else {
        return;
    };
    let mut index = adoc_index::WorkspaceIndex::new();
    let mut bytes = 0;
    for number in 0..10_000 {
        let mut text = format!("= Page {number}\n\n");
        for section in 0..10 {
            let _ = write!(
                text,
                "[[page-{number}-section-{section}]]\n== Section {section} of page {number}\n\n"
            );
            for _ in 0..6 {
                text.push_str("A paragraph of prose long enough to look like documentation, with a few words of every length in it, so the page is of an ordinary size.\n\n");
            }
            let _ = write!(
                text,
                "See xref:page-{}.adoc#page-{}-section-{section}[the next page].\n\n",
                number + 1,
                number + 1
            );
        }
        bytes += text.len();
        index.index_file(format!("/bench/page-{number}.adoc"), &text);
    }
    let added = resident().unwrap_or(before).saturating_sub(before);
    report.memory(
        "index_memory",
        "10000 × 8 KB pages".to_owned(),
        added,
        bytes,
        Budget::Bytes(2.5),
    );
    drop(index);
}

fn main() {
    // `cargo test` runs benches once to check they build; there is nothing to time then.
    if std::env::args().any(|argument| argument == "--test" || argument == "--list") {
        return;
    }
    let mut report = Report::default();
    index_memory(&mut report);
    keystrokes(&mut report);
    settles(&mut report);
    workspace(&mut report);

    let table = report.table();
    println!("{table}");
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        let _ = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(summary)
            .and_then(|mut file| {
                std::io::Write::write_all(
                    &mut file,
                    format!("### Engine benchmarks\n\n{table}\n").as_bytes(),
                )
            });
    }
    let over: Vec<&str> = report
        .rows
        .iter()
        .filter(|row| row.over)
        .map(|row| row.bench)
        .collect();
    if !over.is_empty() && std::env::var_os("BENCH_RECORD").is_none() {
        eprintln!("over budget: {}", over.join(", "));
        std::process::exit(1);
    }
}
