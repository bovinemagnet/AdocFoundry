//! An opt-in typing benchmark for the frame-time acceptance check.
//!
//! `ADOCFOUNDRY_TYPING_BENCH=10 adocfoundry <folder> <file>` types into the active editor for ten
//! seconds through the real widget → engine path, records the interval between rendered frames,
//! prints a summary to standard error and quits. Each frame over [`LONG_FRAME`] is listed too,
//! with when it came and how long the front end's thread spent in each part [`measure`] times
//! since the frame before (#241); what is left over is GPUI's own layout and painting, the
//! editor widget's included, and whatever the platform did.

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
/// A frame listed with its parts: one and a half of a 60 Hz display's.
const LONG_FRAME: Duration = Duration::from_millis(25);

#[derive(Default)]
struct Frames {
    start: Option<Instant>,
    last: Option<Instant>,
    intervals: Vec<Duration>,
    long: Vec<LongFrame>,
}

/// A frame over [`LONG_FRAME`]: when it ended, after the first frame, how long it took, and the
/// parts timed while it was prepared.
struct LongFrame {
    at: Duration,
    interval: Duration,
    parts: Vec<(&'static str, Duration)>,
}

thread_local! {
    /// The time spent in each part since the last frame, while the benchmark runs.
    static PARTS: RefCell<Option<Vec<(&'static str, Duration)>>> = const { RefCell::new(None) };
}

/// Runs `f`, adding the time it took to the part `name` of the frame being prepared while the
/// benchmark runs. Parts may nest: a part's time includes any part it runs.
pub fn measure<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let _timed = timed(name);
    f()
}

/// Adds the time until it is dropped to the part `name`, as [`measure`] does for a closure: for
/// timing a whole function from its first line.
#[must_use]
pub fn timed(name: &'static str) -> Timed {
    let running = PARTS.with(|parts| parts.borrow().is_some());
    Timed {
        name,
        start: running.then(Instant::now),
    }
}

pub struct Timed {
    name: &'static str,
    start: Option<Instant>,
}

impl Drop for Timed {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let took = start.elapsed();
        PARTS.with(|parts| {
            if let Some(parts) = parts.borrow_mut().as_mut() {
                match parts.iter_mut().find(|(part, _)| *part == self.name) {
                    Some((_, total)) => *total += took,
                    None => parts.push((self.name, took)),
                }
            }
        });
    }
}

/// The parts timed since the last call.
fn take_parts() -> Vec<(&'static str, Duration)> {
    PARTS.with(|parts| {
        parts
            .borrow_mut()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    })
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
    PARTS.with(|parts| *parts.borrow_mut() = Some(Vec::new()));
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
            PARTS.with(|parts| *parts.borrow_mut() = None);
            report(&frames.borrow());
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
        let parts = take_parts();
        {
            let mut frames = frames.borrow_mut();
            let start = *frames.start.get_or_insert(now);
            if let Some(last) = frames.last {
                let interval = now - last;
                frames.intervals.push(interval);
                if interval > LONG_FRAME {
                    frames.long.push(LongFrame {
                        at: now - start,
                        interval,
                        parts,
                    });
                }
            }
            frames.last = Some(now);
        }
        if now < end {
            track(frames, end, window);
        }
    });
}

fn report(frames: &Frames) {
    let intervals = &frames.intervals;
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
    for frame in &frames.long {
        eprintln!("typing benchmark long frame: {}", describe(frame));
    }
}

/// A long frame as one line: when, how long, and its parts, the longest first.
fn describe(frame: &LongFrame) -> String {
    let ms = |duration: Duration| format!("{:.1} ms", duration.as_secs_f64() * 1000.0);
    let mut parts = frame.parts.clone();
    parts.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    // Parts nest, so at least the frame less its longest part went untimed.
    let longest = parts.first().map(|(_, took)| *took).unwrap_or_default();
    let listed: Vec<String> = parts
        .iter()
        .filter(|(_, took)| *took >= Duration::from_micros(100))
        .map(|(part, took)| format!("{part} {}", ms(*took)))
        .collect();
    format!(
        "at {:.2} s, {}: {}; not timed at least {}",
        frame.at.as_secs_f64(),
        ms(frame.interval),
        if listed.is_empty() {
            "nothing timed".to_owned()
        } else {
            listed.join(", ")
        },
        ms(frame.interval.saturating_sub(longest)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_frame_lists_its_parts_longest_first() {
        let frame = LongFrame {
            at: Duration::from_millis(1_250),
            interval: Duration::from_millis(40),
            parts: vec![
                ("render.editor", Duration::from_millis(3)),
                ("pump", Duration::from_millis(12)),
                ("events", Duration::from_micros(50)),
            ],
        };
        assert_eq!(
            describe(&frame),
            "at 1.25 s, 40.0 ms: pump 12.0 ms, render.editor 3.0 ms; not timed at least 28.0 ms"
        );
    }

    #[test]
    fn parts_are_timed_only_while_the_benchmark_runs() {
        assert_eq!(measure("pump", || 1), 1);
        assert!(take_parts().is_empty());
        PARTS.with(|parts| *parts.borrow_mut() = Some(Vec::new()));
        measure("pump", || ());
        measure("pump", || ());
        measure("events", || ());
        let names: Vec<_> = take_parts().into_iter().map(|(part, _)| part).collect();
        assert_eq!(names, ["pump", "events"]);
        assert!(take_parts().is_empty(), "taken once");
        PARTS.with(|parts| *parts.borrow_mut() = None);
    }
}
