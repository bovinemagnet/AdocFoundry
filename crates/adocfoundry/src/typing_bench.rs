//! An opt-in typing benchmark for the frame-time acceptance check.
//!
//! `ADOCFOUNDRY_TYPING_BENCH=10 adocfoundry <folder> <file>` types into the active editor for ten
//! seconds through the real widget → engine path, records the interval between rendered frames,
//! prints a summary to standard error and quits.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{App, Entity, Window};

use crate::editor::SourceEditorView;

/// The environment variable that enables the benchmark; its value is the duration in seconds.
pub const ENABLE: &str = "ADOCFOUNDRY_TYPING_BENCH";

/// Faster than anyone types, so every frame has an edit to show.
const KEYSTROKE_INTERVAL: Duration = Duration::from_millis(35);
const TEXT: &str = "The quick brown fox jumps over the lazy dog. ";

#[derive(Default)]
struct Frames {
    last: Option<Instant>,
    intervals: Vec<Duration>,
}

/// The benchmark's duration, if it is enabled.
pub fn requested() -> Option<Duration> {
    std::env::var(ENABLE)
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

pub fn start(
    editor: Entity<SourceEditorView>,
    duration: Duration,
    window: &mut Window,
    cx: &mut App,
) {
    let frames = Rc::new(RefCell::new(Frames::default()));
    let end = Instant::now() + duration;
    track(frames.clone(), end, window);

    window
        .spawn(cx, async move |cx| {
            let mut characters = TEXT.chars().cycle();
            while Instant::now() < end {
                cx.background_executor().timer(KEYSTROKE_INTERVAL).await;
                let character = characters.next().unwrap_or(' ').to_string();
                let typed = editor.update_in(cx, |editor, window, cx| {
                    editor.type_for_benchmark(&character, window, cx);
                });
                if typed.is_err() {
                    return;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            report(&frames.borrow().intervals);
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
}

/// Records the time between consecutive frames until `end`, asking for a frame each time so the
/// measurement covers every frame the window draws.
fn track(frames: Rc<RefCell<Frames>>, end: Instant, window: &mut Window) {
    window.refresh();
    window.on_next_frame(move |window, _| {
        let now = Instant::now();
        {
            let mut frames = frames.borrow_mut();
            if let Some(last) = frames.last {
                frames.intervals.push(now - last);
            }
            frames.last = Some(now);
        }
        if now < end {
            track(frames, end, window);
        }
    });
}

fn report(intervals: &[Duration]) {
    if intervals.is_empty() {
        // macOS stops drawing a window that is covered, minimised or on a locked screen.
        eprintln!("typing benchmark: no frames were drawn (was the window hidden?)");
        return;
    }
    let mut sorted = intervals.to_vec();
    sorted.sort();
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
    let over = |limit: Duration| {
        intervals
            .iter()
            .filter(|interval| **interval > limit)
            .count()
    };
    eprintln!(
        "typing benchmark: {} frames; interval median {:?}, p99 {:?}, worst {:?}; over 16.7 ms: {}; over 25 ms: {}; over 33 ms: {}",
        intervals.len(),
        percentile(0.5),
        percentile(0.99),
        sorted[sorted.len() - 1],
        over(Duration::from_micros(16_700)),
        over(Duration::from_millis(25)),
        over(Duration::from_millis(33)),
    );
}
