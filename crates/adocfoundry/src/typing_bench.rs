//! An opt-in typing benchmark for the frame-time acceptance check.
//!
//! `ADOCFOUNDRY_TYPING_BENCH=10 adocfoundry <folder> <file>` types into the active editor for ten
//! seconds through the real widget → engine path, records the interval between rendered frames,
//! prints a summary to standard error and quits.
//!
//! It also records the app's own work in each interval (#241): the layout and paint of the frame
//! drawn at its start, from the frame callback GPUI runs just before drawing to [`paint_marker`],
//! painted last in the workspace, and the parts [`measure`] times on the front end's thread after
//! that paint. What is left of the interval is the platform's: presenting the frame, waiting for
//! the display and whatever else the machine did. A hosted runner's tail is mostly that, so the
//! budget ([`budget`]) holds the app's work, not the intervals. Work the editor widget does in its
//! own tasks between frames is not timed.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{App, Entity, IntoElement, Window};

use crate::editor::SourceEditorView;

/// The environment variable that enables the benchmark; its value is the duration in seconds.
pub const ENABLE: &str = "ADOCFOUNDRY_TYPING_BENCH";

/// Faster than anyone types, so every frame has an edit to show.
const KEYSTROKE_INTERVAL: Duration = Duration::from_millis(35);
const TEXT: &str = "The quick brown fox jumps over the lazy dog. ";
/// A frame listed with its parts: one and a half of a 60 Hz display's.
const LONG_FRAME: Duration = Duration::from_millis(25);
/// The start-up the budget leaves out: the page's first layout and highlight.
const SETTLING: Duration = Duration::from_secs(1);
/// The budget's p99 of the app's work in a frame interval: a 60 Hz frame.
const WORK_P99: Duration = Duration::from_micros(16_700);
/// The budget's longest single piece of work, one draw or one timed part: two 60 Hz frames.
const LONGEST_PIECE: Duration = Duration::from_millis(33);
/// The name a frame's layout and paint goes by among the parts.
const DRAW: &str = "draw";

#[derive(Default)]
struct Frames {
    start: Option<Instant>,
    last: Option<Instant>,
    frames: Vec<Frame>,
}

/// One interval between frames: when it ended, after the first frame, how long it was, the app's
/// work in it and that work's longest single piece, and the parts timed in it, the frame's
/// layout and paint as [`DRAW`] among them (absent when the workspace was not painted).
struct Frame {
    at: Duration,
    interval: Duration,
    work: Duration,
    longest: Option<(&'static str, Duration)>,
    parts: Vec<(&'static str, Duration)>,
    /// The thread's CPU time in the frame's layout and paint, where the platform gives it: far
    /// less than the draw's own time when the machine held the thread rather than the app.
    draw_cpu: Option<Duration>,
}

/// A moment by the clock and by the front end's thread's CPU time.
#[derive(Clone, Copy)]
struct Clocks {
    wall: Instant,
    cpu: Option<Duration>,
}

impl Clocks {
    fn now() -> Self {
        Clocks {
            wall: Instant::now(),
            cpu: thread_cpu(),
        }
    }
}

/// A frame's layout and paint, by the clock and on the CPU.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    wall: Duration,
    cpu: Option<Duration>,
}

/// The CPU time the calling thread has used.
#[cfg(unix)]
fn thread_cpu() -> Option<Duration> {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    Some(Duration::new(
        u64::try_from(time.tv_sec).ok()?,
        u32::try_from(time.tv_nsec).ok()?,
    ))
}

#[cfg(not(unix))]
fn thread_cpu() -> Option<Duration> {
    None
}

/// What the front end's thread was timed doing while the benchmark runs.
#[derive(Default)]
struct Recording {
    /// The time in each part since the last frame began, nested parts included.
    parts: Vec<(&'static str, Duration)>,
    /// How many timed parts are running, so a nested part is not counted twice.
    depth: usize,
    /// The outermost parts' time since the last frame began, outside its layout and paint.
    busy: Duration,
    /// The longest of those parts.
    longest: Option<(&'static str, Duration)>,
    /// When the frame being drawn began, until the workspace is painted.
    drawing: Option<Clocks>,
    /// That frame's layout and paint, once painted.
    drawn: Option<Drawn>,
}

/// What was recorded between one frame beginning and the next.
#[derive(Default)]
struct Taken {
    parts: Vec<(&'static str, Duration)>,
    busy: Duration,
    longest: Option<(&'static str, Duration)>,
    drawn: Option<Drawn>,
}

thread_local! {
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

fn longer(
    longest: Option<(&'static str, Duration)>,
    name: &'static str,
    took: Duration,
) -> Option<(&'static str, Duration)> {
    match longest {
        Some((_, before)) if before >= took => longest,
        _ => Some((name, took)),
    }
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
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        let Some(recording) = recording.as_mut() else {
            return Timed {
                name,
                start: None,
                counted: false,
            };
        };
        recording.depth += 1;
        Timed {
            name,
            start: Some(Instant::now()),
            // Inside a frame's layout and paint, the draw's own time covers it.
            counted: recording.depth == 1 && recording.drawing.is_none(),
        }
    })
}

pub struct Timed {
    name: &'static str,
    start: Option<Instant>,
    counted: bool,
}

impl Drop for Timed {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let took = start.elapsed();
        RECORDING.with(|recording| {
            if let Some(recording) = recording.borrow_mut().as_mut() {
                recording.depth = recording.depth.saturating_sub(1);
                match recording
                    .parts
                    .iter_mut()
                    .find(|(part, _)| *part == self.name)
                {
                    Some((_, total)) => *total += took,
                    None => recording.parts.push((self.name, took)),
                }
                if self.counted {
                    recording.busy += took;
                    recording.longest = longer(recording.longest, self.name, took);
                }
            }
        });
    }
}

fn running() -> bool {
    RECORDING.with(|recording| recording.borrow().is_some())
}

/// An empty element that marks the end of the workspace's paint while the benchmark runs, for
/// the last child of the workspace's root.
pub fn paint_marker() -> Option<gpui::AnyElement> {
    running()
        .then(|| gpui::canvas(|_, _, _| (), |_, _, _, _| painted(Clocks::now())).into_any_element())
}

/// The frame begun at the last [`begin_frame`] was painted at `now`.
fn painted(now: Clocks) {
    RECORDING.with(|recording| {
        if let Some(recording) = recording.borrow_mut().as_mut() {
            if let Some(began) = recording.drawing.take() {
                recording.drawn = Some(Drawn {
                    wall: now.wall - began.wall,
                    cpu: now
                        .cpu
                        .zip(began.cpu)
                        .map(|(now, began)| now.saturating_sub(began)),
                });
            }
        }
    });
}

/// A frame begins drawing at `now`: what was recorded since the last one began.
fn begin_frame(now: Clocks) -> Taken {
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        let Some(recording) = recording.as_mut() else {
            return Taken::default();
        };
        let taken = Taken {
            parts: std::mem::take(&mut recording.parts),
            busy: std::mem::take(&mut recording.busy),
            longest: recording.longest.take(),
            drawn: recording.drawn.take(),
        };
        recording.drawing = Some(now);
        taken
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
    RECORDING.with(|recording| *recording.borrow_mut() = Some(Recording::default()));
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
            RECORDING.with(|recording| *recording.borrow_mut() = None);
            report(&frames.borrow().frames);
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
}

/// Records each interval between consecutive frames until `end`, asking for a frame each time so
/// the measurement covers every frame the window draws. GPUI runs the callback just before it
/// draws the frame.
fn track(frames: Rc<RefCell<Frames>>, end: Instant, window: &mut Window) {
    window.refresh();
    window.on_next_frame(move |window, _| {
        let clocks = Clocks::now();
        let now = clocks.wall;
        let taken = begin_frame(clocks);
        {
            let mut frames = frames.borrow_mut();
            let start = *frames.start.get_or_insert(now);
            if let Some(last) = frames.last {
                frames.frames.push(frame(now - start, now - last, taken));
            }
            frames.last = Some(now);
        }
        if now < end {
            track(frames, end, window);
        }
    });
}

/// An interval of `interval` ending `at` after the first frame, from what was recorded in it.
fn frame(at: Duration, interval: Duration, taken: Taken) -> Frame {
    let Taken {
        mut parts,
        busy,
        mut longest,
        drawn,
    } = taken;
    if let Some(drawn) = drawn {
        parts.push((DRAW, drawn.wall));
        longest = longer(longest, DRAW, drawn.wall);
    }
    Frame {
        at,
        interval,
        work: busy + drawn.map(|drawn| drawn.wall).unwrap_or_default(),
        longest,
        parts,
        draw_cpu: drawn.and_then(|drawn| drawn.cpu),
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// The value below which `p` of the sorted `values` fall.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(frames: &[Frame]) {
    if frames.is_empty() {
        // macOS stops drawing a window that is covered, minimised or on a locked screen.
        eprintln!("typing benchmark: no frames were drawn (was the window hidden?)");
        eprintln!("typing benchmark budget: not judged, as no frames were drawn");
        return;
    }
    let mut sorted: Vec<Duration> = frames.iter().map(|frame| frame.interval).collect();
    sorted.sort();
    let over = |limit: Duration| frames.iter().filter(|frame| frame.interval > limit).count();
    eprintln!(
        "typing benchmark: {} frames; interval median {:?}, p99 {:?}, worst {:?}; over 16.7 ms: {}; over 25 ms: {}; over 33 ms: {}",
        frames.len(),
        percentile(&sorted, 0.5),
        percentile(&sorted, 0.99),
        sorted[sorted.len() - 1],
        over(Duration::from_micros(16_700)),
        over(Duration::from_millis(25)),
        over(Duration::from_millis(33)),
    );
    eprintln!("typing benchmark work: {}", work_summary(frames));
    match budget(frames) {
        Ok(within) => eprintln!("typing benchmark budget: within: {within}"),
        Err(over) => eprintln!("typing benchmark budget: over: {over}"),
    }
    for frame in frames.iter().filter(|frame| frame.interval > LONG_FRAME) {
        eprintln!("typing benchmark long frame: {}", describe(frame));
    }
}

/// The app's work per interval over the whole run, as one line.
fn work_summary(frames: &[Frame]) -> String {
    let mut work: Vec<Duration> = frames.iter().map(|frame| frame.work).collect();
    work.sort();
    let unpainted = frames
        .iter()
        .filter(|frame| !frame.parts.iter().any(|(part, _)| *part == DRAW))
        .count();
    let most_cpu = frames
        .iter()
        .filter_map(|frame| frame.draw_cpu.map(|cpu| (frame.at, cpu)))
        .max_by_key(|(_, cpu)| *cpu);
    format!(
        "median {}, p99 {}, worst {}; longest piece {}; most CPU in one draw {}; intervals without a paint: {unpainted}",
        ms(percentile(&work, 0.5)),
        ms(percentile(&work, 0.99)),
        ms(work[work.len() - 1]),
        longest_piece(frames.iter())
            .map(|(at, name, took)| format!("{name} {} at {:.2} s", ms(took), at.as_secs_f64()))
            .unwrap_or_else(|| "none".to_owned()),
        most_cpu
            .map(|(at, cpu)| format!("{} at {:.2} s", ms(cpu), at.as_secs_f64()))
            .unwrap_or_else(|| "not known".to_owned()),
    )
}

/// The longest single piece of work among `frames`: when, what and how long.
fn longest_piece<'a>(
    frames: impl Iterator<Item = &'a Frame>,
) -> Option<(Duration, &'static str, Duration)> {
    frames
        .filter_map(|frame| frame.longest.map(|(name, took)| (frame.at, name, took)))
        .max_by_key(|(_, _, took)| *took)
}

/// The budget after [`SETTLING`]: the app's work in an interval at p99 within [`WORK_P99`], and
/// no single piece of it over [`LONGEST_PIECE`]. `Ok` says what it held, `Err` what it missed.
fn budget(frames: &[Frame]) -> Result<String, String> {
    let judged: Vec<&Frame> = frames.iter().filter(|frame| frame.at >= SETTLING).collect();
    if judged.is_empty() {
        return Err(format!(
            "no frames after the first {} s to judge",
            SETTLING.as_secs()
        ));
    }
    let mut work: Vec<Duration> = judged.iter().map(|frame| frame.work).collect();
    work.sort();
    let p99 = percentile(&work, 0.99);
    let longest = longest_piece(judged.iter().copied());
    let mut missed = Vec::new();
    if p99 > WORK_P99 {
        missed.push(format!("p99 work {} > {}", ms(p99), ms(WORK_P99)));
    }
    if let Some((at, name, took)) = longest.filter(|(_, _, took)| *took > LONGEST_PIECE) {
        missed.push(format!(
            "{name} took {} at {:.2} s > {}",
            ms(took),
            at.as_secs_f64(),
            ms(LONGEST_PIECE)
        ));
    }
    if missed.is_empty() {
        Ok(format!(
            "p99 work {} ≤ {}, longest piece {} ≤ {}, after the first {} s",
            ms(p99),
            ms(WORK_P99),
            ms(longest.map(|(_, _, took)| took).unwrap_or_default()),
            ms(LONGEST_PIECE),
            SETTLING.as_secs()
        ))
    } else {
        Err(missed.join("; "))
    }
}

/// A long frame as one line: when, how long, its parts, the longest first, and what was left.
fn describe(frame: &Frame) -> String {
    let mut parts = frame.parts.clone();
    parts.sort_by_key(|(_, took)| std::cmp::Reverse(*took));
    let listed: Vec<String> = parts
        .iter()
        .filter(|(_, took)| *took >= Duration::from_micros(100))
        .map(|(part, took)| format!("{part} {}", ms(*took)))
        .collect();
    let cpu = frame
        .draw_cpu
        .map(|cpu| format!("; draw on the CPU {}", ms(cpu)))
        .unwrap_or_default();
    format!(
        "at {:.2} s, {}: {}; not timed {}{cpu}",
        frame.at.as_secs_f64(),
        ms(frame.interval),
        if listed.is_empty() {
            "nothing timed".to_owned()
        } else {
            listed.join(", ")
        },
        ms(frame.interval.saturating_sub(frame.work)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() {
        RECORDING.with(|recording| *recording.borrow_mut() = Some(Recording::default()));
    }

    fn stop() {
        RECORDING.with(|recording| *recording.borrow_mut() = None);
    }

    fn work_frame(at_ms: u64, work_ms: u64, longest: Option<(&'static str, u64)>) -> Frame {
        Frame {
            at: Duration::from_millis(at_ms),
            interval: Duration::from_millis(17),
            work: Duration::from_millis(work_ms),
            longest: longest.map(|(name, ms)| (name, Duration::from_millis(ms))),
            parts: Vec::new(),
            draw_cpu: None,
        }
    }

    #[test]
    fn a_long_frame_lists_its_parts_longest_first() {
        let frame = frame(
            Duration::from_millis(1_250),
            Duration::from_millis(40),
            Taken {
                parts: vec![
                    ("render.editor", Duration::from_millis(3)),
                    ("pump", Duration::from_millis(12)),
                    ("events", Duration::from_micros(50)),
                ],
                busy: Duration::from_millis(12),
                longest: Some(("pump", Duration::from_millis(12))),
                drawn: Some(Drawn {
                    wall: Duration::from_millis(5),
                    cpu: Some(Duration::from_millis(4)),
                }),
            },
        );
        assert_eq!(
            describe(&frame),
            "at 1.25 s, 40.0 ms: pump 12.0 ms, draw 5.0 ms, render.editor 3.0 ms; not timed 23.0 ms; draw on the CPU 4.0 ms"
        );
    }

    #[test]
    fn parts_are_timed_only_while_the_benchmark_runs() {
        assert_eq!(measure("pump", || 1), 1);
        assert!(begin_frame(Clocks::now()).parts.is_empty());
        record();
        measure("pump", || ());
        measure("pump", || ());
        measure("events", || ());
        let names: Vec<_> = begin_frame(Clocks::now())
            .parts
            .into_iter()
            .map(|(part, _)| part)
            .collect();
        assert_eq!(names, ["pump", "events"]);
        assert!(begin_frame(Clocks::now()).parts.is_empty(), "taken once");
        stop();
    }

    #[test]
    fn a_nested_part_counts_once_towards_the_work() {
        record();
        measure("pump", || measure("events.workspace", || ()));
        let taken = begin_frame(Clocks::now());
        let pump = taken
            .parts
            .iter()
            .find(|(part, _)| *part == "pump")
            .unwrap()
            .1;
        assert_eq!(taken.busy, pump, "the outer part's time only");
        assert_eq!(taken.longest, Some(("pump", pump)));
        stop();
    }

    #[test]
    fn a_frame_s_layout_and_paint_runs_from_its_beginning_to_the_paint_marker() {
        record();
        let began = Instant::now();
        let at = |ms: u64, cpu_ms: u64| Clocks {
            wall: began + Duration::from_millis(ms),
            cpu: Some(Duration::from_millis(cpu_ms)),
        };
        begin_frame(at(0, 100));
        // A view rendered while the frame is drawn is inside the draw's own time.
        measure("render.editor", || ());
        painted(at(6, 102));
        // A second paint in the same frame does not move its end.
        painted(at(9, 105));
        measure("type", || ());
        let taken = begin_frame(at(17, 110));
        let typed = taken
            .parts
            .iter()
            .find(|(part, _)| *part == "type")
            .unwrap()
            .1;
        assert_eq!(
            taken.drawn,
            Some(Drawn {
                wall: Duration::from_millis(6),
                cpu: Some(Duration::from_millis(2)),
            })
        );
        assert_eq!(taken.busy, typed, "render.editor is not counted twice");
        let frame = frame(Duration::from_secs(2), Duration::from_millis(17), taken);
        assert_eq!(frame.work, Duration::from_millis(6) + typed);
        assert_eq!(frame.longest, Some((DRAW, Duration::from_millis(6))));
        assert_eq!(frame.draw_cpu, Some(Duration::from_millis(2)));
        stop();
    }

    #[test]
    fn a_draw_s_cpu_time_is_not_known_without_both_ends() {
        record();
        let began = Instant::now();
        begin_frame(Clocks {
            wall: began,
            cpu: None,
        });
        painted(Clocks {
            wall: began + Duration::from_millis(6),
            cpu: Some(Duration::from_millis(3)),
        });
        let taken = begin_frame(Clocks::now());
        assert_eq!(taken.drawn.map(|drawn| drawn.cpu), Some(None));
        stop();
    }

    #[cfg(unix)]
    #[test]
    fn the_thread_s_cpu_time_moves_forward_as_it_works() {
        let before = thread_cpu().expect("unix gives a thread's CPU time");
        let mut spin = 0u64;
        while thread_cpu().unwrap() < before + Duration::from_millis(1) {
            spin = spin.wrapping_add(1);
        }
        assert!(spin > 0);
    }

    #[test]
    fn an_interval_whose_frame_was_not_painted_has_no_draw() {
        record();
        begin_frame(Clocks::now());
        let frame = frame(
            Duration::from_secs(2),
            Duration::from_millis(17),
            begin_frame(Clocks::now()),
        );
        assert!(frame.parts.is_empty());
        assert_eq!(frame.work, Duration::ZERO);
        stop();
    }

    #[test]
    fn the_budget_holds_the_app_s_work_after_the_first_second() {
        let mut frames: Vec<Frame> = (0..200)
            .map(|n| work_frame(1_000 + n * 17, 4, Some((DRAW, 4))))
            .collect();
        // Start-up is left out, however long.
        frames.insert(0, work_frame(300, 400, Some(("type", 400))));
        assert_eq!(
            budget(&frames),
            Ok(
                "p99 work 4.0 ms ≤ 16.7 ms, longest piece 4.0 ms ≤ 33.0 ms, after the first 1 s"
                    .to_owned()
            )
        );
    }

    #[test]
    fn the_budget_fails_on_a_slow_tail_or_one_long_piece() {
        let mut frames: Vec<Frame> = (0..200)
            .map(|n| work_frame(1_000 + n * 17, 4, Some((DRAW, 4))))
            .collect();
        for frame in frames.iter_mut().take(5) {
            frame.work = Duration::from_millis(20);
        }
        frames[100].longest = Some(("type", Duration::from_millis(40)));
        assert_eq!(
            budget(&frames),
            Err("p99 work 20.0 ms > 16.7 ms; type took 40.0 ms at 2.70 s > 33.0 ms".to_owned())
        );
    }

    #[test]
    fn a_long_interval_with_little_work_is_within_the_budget() {
        // The runner held the display for a second: the app did nothing wrong.
        let mut frames: Vec<Frame> = (0..200)
            .map(|n| work_frame(1_000 + n * 17, 4, Some((DRAW, 4))))
            .collect();
        frames[50].interval = Duration::from_secs(1);
        assert!(budget(&frames).is_ok());
    }

    #[test]
    fn a_run_with_nothing_after_the_first_second_is_not_within_the_budget() {
        assert_eq!(
            budget(&[work_frame(500, 1, None)]),
            Err("no frames after the first 1 s to judge".to_owned())
        );
    }
}
