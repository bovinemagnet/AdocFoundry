//! The layout's budget: `cargo bench -p adoc-graph`. One step for 2,000 nodes and 6,000 edges
//! must take under 8 ms (median), or the run fails. `BENCH_RECORD=1` records without failing.
//! The table is appended to `$GITHUB_STEP_SUMMARY` when CI sets it.

use std::{
    fs,
    io::Write as _,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use adoc_graph::{EdgeKind, GraphEdge, GraphKey, GraphNode, GraphSnapshot, Layout, NodeKind};

const BUDGET: Duration = Duration::from_millis(8);

/// `nodes` pages with `edges` links among them, spread as a documentation site's are.
fn graph(nodes: usize, edges: usize) -> GraphSnapshot {
    let mut keys: Vec<GraphKey> = (0..nodes)
        .map(|n| {
            GraphKey::File(Arc::from(Path::new(&format!(
                "modules/m{}/pages/p{n:05}.adoc",
                n % 20
            ))))
        })
        .collect();
    keys.sort();
    let mut links: Vec<GraphEdge> = (0..edges)
        .map(|n| {
            let from = (n * 7919) % nodes;
            // Mostly near neighbours, as pages of one module link to each other.
            let to = (from + 1 + (n * 31 + n / nodes * 17) % 40) % nodes;
            GraphEdge {
                from: keys[from].clone(),
                to: keys[to].clone(),
                kind: EdgeKind::Xref,
                count: 1,
            }
        })
        .collect();
    links.sort();
    links.dedup();
    GraphSnapshot {
        revision: 1,
        nodes: keys
            .into_iter()
            .map(|key| GraphNode {
                key,
                title: String::new(),
                kind: NodeKind::Page,
                coordinate: None,
                links_out: 0,
                links_in: 0,
            })
            .collect(),
        edges: links,
    }
}

fn main() {
    // `cargo test` runs benches once to check they build; there is nothing to time then.
    if std::env::args().any(|argument| argument == "--test" || argument == "--list") {
        return;
    }
    let snapshot = graph(2_000, 6_000);
    let mut layout = Layout::new(1);
    layout.sync(&snapshot);
    // Warm up, and leave the start's widest moves behind.
    for _ in 0..10 {
        layout.step(1.0);
    }
    let mut samples: Vec<Duration> = (0..50)
        .map(|_| {
            layout.wake();
            let started = Instant::now();
            layout.step(1.0);
            started.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = samples[samples.len() - 1];
    let over = median > BUDGET;
    let table = format!(
        "| bench | size | median | worst | budget |\n|---|---|---|---|---|\n\
         | layout step | {} nodes, {} edges | {:.2} ms | {:.2} ms | ≤ {} ms{} |\n",
        snapshot.nodes.len(),
        snapshot.edges.len(),
        median.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        BUDGET.as_millis(),
        if over { " **over**" } else { "" },
    );
    print!("{table}");
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        if let Ok(mut file) = fs::OpenOptions::new().append(true).open(summary) {
            let _ = file.write_all(table.as_bytes());
        }
    }
    if over && std::env::var_os("BENCH_RECORD").is_none() {
        eprintln!("the layout step is over its budget");
        std::process::exit(1);
    }
}
