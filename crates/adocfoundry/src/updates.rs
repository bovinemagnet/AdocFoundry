//! Checking for a newer release: the app reads the releases feed through the system's `curl`
//! and hands it to the engine, which decides what to announce and remembers it. No HTTP client
//! or TLS stack is compiled into the app.

use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use adoc_engine::updates::Release;
use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    notification::Notification,
    WindowExt as _,
};

use crate::app::AppState;

/// Reads a web address, giving its body or why it could not.
pub type Fetch = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// How long a feed request may take, in seconds.
const FEED_TIMEOUT: &str = "20";

/// Marks the update notice, so a second is not stacked on the first.
struct UpdateNotice;

/// The system's `curl`: the one macOS and Windows ship, else the first on `PATH`.
#[must_use]
pub fn curl() -> Option<PathBuf> {
    let shipped = if cfg!(target_os = "macos") {
        Some(PathBuf::from("/usr/bin/curl"))
    } else if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("curl.exe"))
    } else {
        None
    };
    shipped.filter(|path| path.is_file()).or_else(|| {
        let name = if cfg!(windows) { "curl.exe" } else { "curl" };
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|directory| directory.join(name))
            .find(|path| path.is_file())
    })
}

/// Fetches through the system's `curl`.
#[must_use]
pub fn system_fetch() -> Fetch {
    Arc::new(|url| {
        let curl = curl().ok_or_else(|| "curl was not found".to_owned())?;
        let output = Command::new(curl)
            .args(["--fail", "--silent", "--show-error", "--location"])
            .args(["--max-time", FEED_TIMEOUT])
            .args(["--header", "Accept: application/vnd.github+json"])
            .args([
                "--user-agent",
                concat!("AdocFoundry/", env!("CARGO_PKG_VERSION")),
            ])
            .arg(url)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|error| error.to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    })
}

/// The feed to read: the releases feed, or in a debug build `ADOCFOUNDRY_UPDATE_FEED`, so an
/// update can be tried against a feed of one's own.
#[must_use]
pub fn feed(engine_feed: &str) -> String {
    if cfg!(debug_assertions) {
        if let Some(feed) = std::env::var("ADOCFOUNDRY_UPDATE_FEED")
            .ok()
            .filter(|feed| !feed.is_empty())
        {
            return feed;
        }
    }
    engine_feed.to_owned()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// At start-up: checks for a newer release when the engine says a check is due.
pub fn check_at_start_up(app: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    if !app.read(cx).engine.update_check_due(now()) {
        return;
    }
    check(app, adoc_engine::updates::FEED, false, window, cx);
}

/// Reads `feed` on a background thread, then tells the user about a newer release. `asked`
/// says the user asked: they hear the outcome either way, and see a release they were told
/// about before. A check at start-up is silent unless there is news.
pub fn check(app: &Entity<AppState>, feed: &str, asked: bool, window: &mut Window, cx: &mut App) {
    let fetch = app.read(cx).fetch.clone();
    let url = self::feed(feed);
    let task = cx.background_spawn(async move { fetch(&url) });
    let app = app.clone();
    window
        .spawn(cx, async move |cx| {
            let body = task.await;
            cx.update(|window, cx| {
                let outcome = body
                    .map_err(|error| format!("could not read the releases: {error}"))
                    .and_then(|body| {
                        app.update(cx, |app, _| {
                            app.engine.record_update_check(&body, now(), asked)
                        })
                        .map_err(|error| error.to_string())
                    });
                match outcome {
                    Ok(Some(release)) => announce(&release, window, cx),
                    Ok(None) if asked => {
                        window.push_notification("AdocFoundry is up to date.", cx);
                    }
                    Err(error) if asked => {
                        window.push_notification(Notification::error(error), cx);
                    }
                    // At start-up, no news is not news, and a failed check waits for the next.
                    Ok(None) | Err(_) => {}
                }
            })
        })
        .detach();
}

/// The notice for `release`, with its release notes a click away.
fn announce(release: &Release, window: &mut Window, cx: &mut App) {
    let page = release.page.clone();
    window.push_notification(
        Notification::info(format!("AdocFoundry {} is available.", release.version))
            .id::<UpdateNotice>()
            .action(move |_, _, _| {
                let page = page.clone();
                Button::new("update-notes")
                    .primary()
                    .label("Release notes")
                    .on_click(move |_, _, cx| cx.open_url(&page))
            }),
        cx,
    );
}
