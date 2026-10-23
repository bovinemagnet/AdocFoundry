use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The engine's source of time, injectable so that time-dependent behaviour such as undo
/// coalescing can be tested deterministically.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
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
    now: Arc<Mutex<Instant>>,
}

impl ManualClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *now += by;
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
        *self
            .now
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
