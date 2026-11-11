pub mod capabilities;
pub mod handlers;
mod position;
pub mod preview;
pub mod protocol;
pub mod server;
pub mod state;

/// Tests that write and run an executable take this, so no other test forks while the script
/// is open for writing (`ETXTBSY`), as `adoc-render`'s `spawn_guard` does there.
#[cfg(test)]
pub(crate) fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
