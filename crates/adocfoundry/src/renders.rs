//! How often each view renders, so tests can assert that an event redraws only what it changes.
//! Counted per thread, and only in test builds.

/// A view whose renders are counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Workspace,
    Editor,
    Visual,
    Preview,
}

#[cfg(test)]
thread_local! {
    static RENDERS: [std::cell::Cell<u64>; 4] = Default::default();
}

/// Counts one render of `view`.
#[inline]
pub fn rendered(view: View) {
    #[cfg(test)]
    RENDERS.with(|renders| {
        let count = &renders[view as usize];
        count.set(count.get() + 1);
    });
    #[cfg(not(test))]
    let _ = view;
}

/// How many times `view` rendered on this thread.
#[cfg(test)]
pub fn count(view: View) -> u64 {
    RENDERS.with(|renders| renders[view as usize].get())
}

/// Sets every count back to 0.
#[cfg(test)]
pub fn reset() {
    RENDERS.with(|renders| renders.iter().for_each(|count| count.set(0)));
}
