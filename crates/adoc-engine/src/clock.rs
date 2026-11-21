use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

/// The engine's source of time, injectable so that time-dependent behaviour such as undo
/// coalescing can be tested deterministically.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;

    /// The time of day, for times people read, such as when a comment was written.
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock that only moves when told to.
#[derive(Clone)]
pub struct ManualClock {
    now: Arc<Mutex<(Instant, SystemTime)>>,
}

impl ManualClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new((Instant::now(), SystemTime::now()))),
        }
    }

    fn times(&self) -> std::sync::MutexGuard<'_, (Instant, SystemTime)> {
        self.now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Moves the clock on, its time of day with it.
    pub fn advance(&self, by: Duration) {
        let mut times = self.times();
        times.0 += by;
        times.1 += by;
    }

    /// Sets the time of day, leaving the instant where it is.
    pub fn set_wall(&self, wall: SystemTime) {
        self.times().1 = wall;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ManualClock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManualClock")
            .finish_non_exhaustive()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.times().0
    }

    fn wall(&self) -> SystemTime {
        self.times().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_wall_clock_advances_with_the_instant() {
        let clock = ManualClock::new();
        let noon = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        clock.set_wall(noon);
        let instant = clock.now();
        clock.advance(Duration::from_secs(5));
        assert_eq!(clock.now(), instant + Duration::from_secs(5));
        assert_eq!(clock.wall(), noon + Duration::from_secs(5));
    }
}
