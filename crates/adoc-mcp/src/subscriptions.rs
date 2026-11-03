//! Resource subscriptions (MCP PRD §7.7; #335): a session subscribes to a page's resource and
//! is told, with `notifications/resources/updated`, when the page changes on disk.
//!
//! Every session shares one engine and so its unsaved buffers; a change only in a buffer is
//! never announced, to anyone: a subscriber learns of a change once it is on disk, saved or
//! written there by anything. Each check reads only what may have changed: every subscribed
//! file is read again when the workspace revision has advanced, otherwise only one whose
//! modification time or length moved (a save changes neither the index nor its revision). Only
//! a file whose text on disk differs from what its subscriber last knew is announced, at most
//! once a [`THROTTLE`] per resource.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use adoc_engine::text_hash;
use rmcp::{model::ResourceUpdatedNotificationParam, service::Peer, RoleServer};

/// The least time between two announcements of one resource.
pub const THROTTLE: Duration = Duration::from_secs(1);

/// One subscribed resource: its file and the text on disk its subscriber last knew of.
struct Entry {
    path: PathBuf,
    known: Option<String>,
    stamp: Option<(std::time::SystemTime, u64)>,
    announced: Option<Instant>,
    /// A change held back by the throttle, to announce at a later check.
    pending: bool,
}

/// The file's modification time and length: what tells a check to read it again.
fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// One session's subscriptions, and the peer to tell.
#[derive(Default)]
pub(crate) struct Subscriptions {
    peer: Option<Peer<RoleServer>>,
    entries: BTreeMap<String, Entry>,
}

/// Every session's subscriptions, for the idle pump to check; a session gone is dropped.
#[derive(Default)]
pub(crate) struct Registry {
    sessions: Vec<Weak<Mutex<Subscriptions>>>,
    /// The workspace revision the last check ran at.
    checked_at: Option<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text of `path` on disk, as a hash; `None` when it cannot be read.
fn on_disk(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text_hash(&text))
}

/// Subscribes `session` to `uri`, the file at `path`, telling `peer`; registers the session
/// with `registry` the first time.
pub(crate) fn subscribe(
    registry: &Mutex<Registry>,
    session: &Arc<Mutex<Subscriptions>>,
    peer: Peer<RoleServer>,
    uri: String,
    path: PathBuf,
) {
    let mut subscriptions = lock(session);
    if subscriptions.entries.is_empty() && subscriptions.peer.is_none() {
        lock(registry).sessions.push(Arc::downgrade(session));
    }
    subscriptions.peer = Some(peer);
    let (known, stamp) = (on_disk(&path), stamp(&path));
    subscriptions.entries.insert(
        uri,
        Entry {
            path,
            known,
            stamp,
            announced: None,
            pending: false,
        },
    );
}

pub(crate) fn unsubscribe(session: &Mutex<Subscriptions>, uri: &str) {
    lock(session).entries.remove(uri);
}

/// What to announce now the workspace is at `revision`: each subscriber's resources whose
/// text on disk changed since it last knew it, throttled.
pub(crate) fn due(
    registry: &Mutex<Registry>,
    revision: u64,
    now: Instant,
) -> Vec<(Peer<RoleServer>, String)> {
    let (sessions, moved) = {
        let mut registry = lock(registry);
        let moved = registry.checked_at != Some(revision);
        registry.checked_at = Some(revision);
        registry
            .sessions
            .retain(|session| session.strong_count() > 0);
        let sessions: Vec<Arc<Mutex<Subscriptions>>> =
            registry.sessions.iter().filter_map(Weak::upgrade).collect();
        (sessions, moved)
    };
    let mut due = Vec::new();
    for session in sessions {
        let mut subscriptions = lock(&session);
        let Some(peer) = subscriptions.peer.clone() else {
            continue;
        };
        for (uri, entry) in &mut subscriptions.entries {
            if check(entry, now, moved) {
                due.push((peer.clone(), uri.clone()));
            }
        }
    }
    due
}

/// Whether `entry` is to be announced now: its text on disk differs from what its subscriber
/// knew, and the throttle allows it. Its file is read only when the workspace `moved`, its
/// stamp moved, or a change is held back; a change within the throttle is held, not lost.
fn check(entry: &mut Entry, now: Instant, moved: bool) -> bool {
    let stamp = stamp(&entry.path);
    if !moved && !entry.pending && stamp == entry.stamp {
        return false;
    }
    let now_on_disk = on_disk(&entry.path);
    if now_on_disk == entry.known {
        entry.stamp = stamp;
        entry.pending = false;
        return false;
    }
    if entry
        .announced
        .is_some_and(|announced| now.duration_since(announced) < THROTTLE)
    {
        entry.pending = true;
        return false;
    }
    entry.stamp = stamp;
    entry.known = now_on_disk;
    entry.announced = Some(now);
    entry.pending = false;
    true
}

/// Sends each announcement; a session that has gone just misses it.
pub(crate) async fn announce(due: Vec<(Peer<RoleServer>, String)>) {
    for (peer, uri) in due {
        let _ = peer
            .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &Path) -> Entry {
        Entry {
            path: path.to_path_buf(),
            known: on_disk(path),
            stamp: stamp(path),
            announced: None,
            pending: false,
        }
    }

    #[test]
    fn a_change_is_announced_once_and_a_held_one_later() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.adoc");
        std::fs::write(&path, "= One\n").unwrap();
        let mut entry = entry(&path);
        let start = Instant::now();
        assert!(!check(&mut entry, start, true), "unchanged");

        std::fs::write(&path, "= Two, a longer title\n").unwrap();
        assert!(check(&mut entry, start, false), "changed on disk");
        assert!(!check(&mut entry, start, true), "once");

        // Changed again within the throttle: held, and announced once it has passed.
        std::fs::write(&path, "= Three, longer again\n").unwrap();
        assert!(!check(
            &mut entry,
            start + Duration::from_millis(100),
            false
        ));
        assert!(entry.pending);
        assert!(check(&mut entry, start + THROTTLE * 2, false));
        assert!(!entry.pending);

        // Written back to what the subscriber knows: nothing to say.
        let known = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, "= Four\n").unwrap();
        std::fs::write(&path, known).unwrap();
        assert!(!check(&mut entry, start + THROTTLE * 4, true));
    }
}
