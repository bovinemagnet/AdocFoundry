//! Checking for a newer release: the app reads the releases feed through the system's `curl`
//! and hands it to the engine, which decides what to announce and remembers it. No HTTP client
//! or TLS stack is compiled into the app.

use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use adoc_engine::updates::{self as engine_updates, Asset, Install, Release, UpdatePlan};
use gpui::{App, AppContext as _, Entity, ParentElement as _, Styled as _, Window};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex, WindowExt as _,
};

use crate::app::AppState;

/// Reads a web address, giving its body or why it could not.
pub type Fetch = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// How long a feed request may take, in seconds.
const FEED_TIMEOUT: &str = "20";

/// How long downloading a package may take, in seconds.
const DOWNLOAD_TIMEOUT: &str = "600";

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
                    Ok(Some(release)) => announce(&app, &release, window, cx),
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

/// How the running app was installed.
fn running_install() -> Install {
    std::env::current_exe().map_or(Install::Other, |executable| {
        engine_updates::install_kind(&executable, &|name| std::env::var(name).ok())
    })
}

/// The notice for `release`: **Update…** where the app can update itself from a release that
/// publishes checksums, else its release notes a click away.
fn announce(app: &Entity<AppState>, release: &Release, window: &mut Window, cx: &mut App) {
    let install = running_install();
    let updatable = matches!(install, Install::MacApp | Install::WindowsMsi)
        && release
            .assets
            .iter()
            .any(|asset| asset.name == engine_updates::SUMS);
    let mut message = format!("AdocFoundry {} is available.", release.version);
    if install == Install::AppImage {
        message.push_str(" AppImageUpdate can update this AppImage.");
    }
    let page = release.page.clone();
    let app = app.clone();
    let release = release.clone();
    window.push_notification(
        Notification::info(message)
            .id::<UpdateNotice>()
            .action(move |_, _, _| {
                if updatable {
                    let app = app.clone();
                    let release = release.clone();
                    Button::new("update-install")
                        .primary()
                        .label("Update…")
                        .on_click(move |_, window, cx| confirm(&app, &release, window, cx))
                } else {
                    let page = page.clone();
                    Button::new("update-notes")
                        .primary()
                        .label("Release notes")
                        .on_click(move |_, _, cx| cx.open_url(&page))
                }
            }),
        cx,
    );
}

/// Asks before updating: the app downloads, checks and installs the release, then restarts.
fn confirm(app: &Entity<AppState>, release: &Release, window: &mut Window, cx: &mut App) {
    let unsaved = app
        .read(cx)
        .engine
        .documents()
        .any(|document| document.dirty);
    if unsaved {
        window.push_notification(
            Notification::warning("Save your changes before updating AdocFoundry."),
            cx,
        );
        return;
    }
    let app = app.clone();
    let release = release.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let page = release.page.clone();
        let app = app.clone();
        let chosen = release.clone();
        dialog
            .title(format!("Update to AdocFoundry {}?", release.version))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        "AdocFoundry downloads the release, checks its checksum and signature, \
                         installs it and restarts.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("update-release-notes")
                                    .label("Release notes")
                                    .on_click(move |_, _, cx| cx.open_url(&page)),
                            )
                            .child(
                                Button::new("update-confirm")
                                    .primary()
                                    .label("Update")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        update(&app, &chosen, window, cx);
                                    }),
                            ),
                    ),
            )
    });
}

/// Downloads, checks and installs `release` on a background thread, then quits so the new
/// version can take over.
fn update(app: &Entity<AppState>, release: &Release, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(format!("Downloading AdocFoundry {}…", release.version))
            .id::<UpdateNotice>(),
        cx,
    );
    let fetch = app.read(cx).fetch.clone();
    let release = release.clone();
    let task = cx.background_spawn(async move {
        let sums = release
            .assets
            .iter()
            .find(|asset| asset.name == engine_updates::SUMS)
            .map(|asset| fetch(&asset.url))
            .transpose()?;
        match engine_updates::plan(
            &release,
            running_install(),
            std::env::consts::ARCH,
            sums.as_deref(),
        ) {
            UpdatePlan::InPlace { asset, checksum } => install(&asset, &checksum),
            UpdatePlan::AppImage { .. } | UpdatePlan::Notify { .. } => {
                Err("this release has no package this installation can update from".to_owned())
            }
        }
    });
    window
        .spawn(cx, async move |cx| {
            let outcome = task.await;
            cx.update(|window, cx| match outcome {
                Ok(mut next) => match next.spawn() {
                    Ok(_) => cx.quit(),
                    Err(error) => window.push_notification(
                        Notification::error(format!("The update could not start: {error}")),
                        cx,
                    ),
                },
                Err(error) => window.push_notification(
                    Notification::error(format!("AdocFoundry was not updated: {error}")),
                    cx,
                ),
            })
        })
        .detach();
}

/// Runs `command`, giving its output and errors together, or why it failed.
fn run(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("{command:?}: {error}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_owned())
    }
}

/// Downloads `asset` into `directory`, checked against `checksum` before anything opens it.
fn download(asset: &Asset, checksum: &str, directory: &std::path::Path) -> Result<PathBuf, String> {
    let curl = curl().ok_or_else(|| "curl was not found".to_owned())?;
    // The asset's name comes from the feed: keep only its last component.
    let name = std::path::Path::new(&asset.name)
        .file_name()
        .ok_or_else(|| format!("{} is not a file name", asset.name))?;
    let path = directory.join(name);
    run(Command::new(curl)
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--max-time", DOWNLOAD_TIMEOUT])
        .args([
            "--user-agent",
            concat!("AdocFoundry/", env!("CARGO_PKG_VERSION")),
        ])
        .arg("--output")
        .arg(&path)
        .arg(&asset.url))?;
    engine_updates::verify_checksum(&path, checksum).map_err(|error| error.to_string())?;
    Ok(path)
}

/// A new, empty directory for an update's files.
fn scratch() -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join(format!("adocfoundry-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory)
}

/// Installs the package `asset` over the running app, returning the command that finishes the
/// job once the app has quit. Each platform's way is compiled everywhere, so every build checks
/// it.
fn install(asset: &Asset, checksum: &str) -> Result<Command, String> {
    if cfg!(target_os = "macos") {
        install_macos(asset, checksum)
    } else if cfg!(windows) {
        install_windows(asset, checksum)
    } else {
        Err("this platform's package manager updates AdocFoundry".to_owned())
    }
}

/// Replaces the running bundle with the DMG's, checked first, and relaunches it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn install_macos(asset: &Asset, checksum: &str) -> Result<Command, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let bundle = executable
        .ancestors()
        .nth(3)
        .ok_or_else(|| "the app is not in a bundle".to_owned())?
        .to_path_buf();
    let team = |app: &std::path::Path| {
        // codesign writes what it displays to standard error, which `run` keeps.
        run(Command::new("codesign")
            .args(["--display", "--verbose=2"])
            .arg(app))
        .ok()
        .and_then(|output| engine_updates::team_identifier(&output))
    };
    let ours = team(&bundle).ok_or_else(|| {
        "this build is not signed with a Developer ID, so it cannot check one".to_owned()
    })?;

    let directory = scratch()?;
    let image = download(asset, checksum, &directory)?;
    let mount = directory.join("mount");
    run(Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(&image))?;
    let staged = bundle.with_file_name(".AdocFoundry-update.app");
    let copied = (|| {
        let new = mount.join("AdocFoundry.app");
        run(Command::new("codesign")
            .args(["--verify", "--strict", "--deep"])
            .arg(&new))?;
        run(Command::new("spctl")
            .args(["--assess", "--type", "execute"])
            .arg(&new))?;
        if team(&new).as_deref() != Some(ours.as_str()) {
            return Err("the new version is signed by someone else".to_owned());
        }
        let _ = std::fs::remove_dir_all(&staged);
        run(Command::new("ditto").arg(&new).arg(&staged))
    })();
    let _ = run(Command::new("hdiutil")
        .arg("detach")
        .arg(&mount)
        .arg("-quiet"));
    copied?;

    // Swapped by renaming within the bundle's folder; the running app keeps its open files.
    let old = bundle.with_file_name(".AdocFoundry-old.app");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&bundle, &old).map_err(|error| format!("{}: {error}", bundle.display()))?;
    if let Err(error) = std::fs::rename(&staged, &bundle) {
        let _ = std::fs::rename(&old, &bundle);
        return Err(format!("{}: {error}", bundle.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&directory);

    // Opened again once this process has gone.
    let mut relaunch = Command::new("/bin/sh");
    relaunch
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; open \"$2\"",
        ])
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(&bundle);
    Ok(relaunch)
}

/// `text` as a PowerShell string literal.
fn powershell_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The Authenticode status of `path` and its signer's subject.
#[cfg_attr(not(windows), allow(dead_code))]
fn signer(path: &std::path::Path) -> Result<(String, String), String> {
    let script = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath {}; $s.Status; $s.SignerCertificate.Subject",
        powershell_literal(&path.to_string_lossy())
    );
    let output = run(Command::new("powershell").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ]))?;
    let mut lines = output.lines().map(str::trim);
    Ok((
        lines.next().unwrap_or_default().to_owned(),
        lines.next().unwrap_or_default().to_owned(),
    ))
}

/// Runs the MSI, checked first, once the app has quit; its major upgrade replaces this version.
#[cfg_attr(not(windows), allow(dead_code))]
fn install_windows(asset: &Asset, checksum: &str) -> Result<Command, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let (status, ours) = signer(&executable)?;
    if status != "Valid" || ours.is_empty() {
        return Err("this build is not signed, so it cannot check a signature".to_owned());
    }
    let directory = scratch()?;
    let package = download(asset, checksum, &directory)?;
    let (status, theirs) = signer(&package)?;
    if status != "Valid" || theirs != ours {
        return Err("the installer is not signed as this build is".to_owned());
    }
    let mut installer = Command::new("msiexec");
    installer.arg("/i").arg(&package).arg("/passive");
    Ok(installer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_literals_escape_quotes() {
        assert_eq!(
            powershell_literal(r"C:\It's here.msi"),
            r"'C:\It''s here.msi'"
        );
    }

    #[test]
    fn the_shipped_curl_is_found() {
        if cfg!(target_os = "macos") {
            assert_eq!(curl(), Some(PathBuf::from("/usr/bin/curl")));
        }
    }
}
