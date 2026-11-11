//! Checking for a newer release: the app reads the releases feed through the system's `curl`
//! and hands it to the engine, which decides what to announce and remembers it. No HTTP client
//! or TLS stack is compiled into the app.

use std::{
    path::{Path, PathBuf},
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

/// Asks before updating: the app downloads and checks the release, quits as File › Quit does
/// (asking about unsaved pages), installs it and restarts.
fn confirm(app: &Entity<AppState>, release: &Release, window: &mut Window, cx: &mut App) {
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
                        "AdocFoundry downloads the release and checks its checksum and \
                         signature, then quits, asking first about unsaved changes, installs \
                         it and restarts.",
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

/// Readies the update to `release`, reading its checksums with the `Fetch` given: downloads its
/// package, checks it and stages it, giving what puts it in place once the app quits.
pub type Prepare = Arc<dyn Fn(&Release, &Fetch) -> Result<Staged, String> + Send + Sync>;

/// An update whose package is downloaded, checked and staged, waiting for the app to quit.
/// Dropped unfinished, it removes the files it staged.
pub struct Staged {
    finish: Option<Box<dyn FnOnce() -> Result<(), String> + Send>>,
    leftovers: Vec<PathBuf>,
}

impl Staged {
    /// `finish` puts the new version in place and starts it; `leftovers` are the files and
    /// folders removed should the update not be finished.
    pub fn new(
        finish: impl FnOnce() -> Result<(), String> + Send + 'static,
        leftovers: Vec<PathBuf>,
    ) -> Self {
        Self {
            finish: Some(Box::new(finish)),
            leftovers,
        }
    }

    /// Puts the new version in place and starts it, as the app quits. What was staged is then
    /// the finishing step's to use and remove.
    pub fn finish(mut self) -> Result<(), String> {
        self.leftovers.clear();
        self.finish.take().map_or(Ok(()), |finish| finish())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        for path in &self.leftovers {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Readies an update the way this installation can take it: the package its release publishes
/// for it, checked against the release's `SHA256SUMS` and its signature against this build's.
#[must_use]
pub fn system_prepare() -> Prepare {
    Arc::new(|release, fetch| {
        let sums = release
            .assets
            .iter()
            .find(|asset| asset.name == engine_updates::SUMS)
            .map(|asset| fetch(&asset.url))
            .transpose()?;
        match engine_updates::plan(
            release,
            running_install(),
            std::env::consts::ARCH,
            sums.as_deref(),
        ) {
            UpdatePlan::InPlace { asset, checksum } => stage(&asset, &checksum),
            UpdatePlan::AppImage { .. } | UpdatePlan::Notify { .. } => {
                Err("this release has no package this installation can update from".to_owned())
            }
        }
    })
}

/// Readies `release` on a background thread, then quits as File › Quit does, asking about
/// unsaved pages first. The new version is put in place only as the app quits; Cancel discards
/// it (`discard_staged`).
pub(crate) fn update(app: &Entity<AppState>, release: &Release, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(format!("Downloading AdocFoundry {}…", release.version))
            .id::<UpdateNotice>(),
        cx,
    );
    let (fetch, prepare) = {
        let app = app.read(cx);
        (app.fetch.clone(), app.prepare.clone())
    };
    let version = release.version.to_string();
    let release = release.clone();
    let task = cx.background_spawn(async move { prepare(&release, &fetch) });
    let app = app.clone();
    window
        .spawn(cx, async move |cx| {
            let outcome = task.await;
            cx.update(|window, cx| match outcome {
                Ok(staged) => {
                    window.push_notification(
                        Notification::info(format!("AdocFoundry {version} is ready to install."))
                            .id::<UpdateNotice>(),
                        cx,
                    );
                    app.update(cx, |app, _| app.staged_update = Some(staged));
                    crate::app::quit(&app, cx);
                }
                Err(error) => window.push_notification(
                    Notification::error(format!("AdocFoundry was not updated: {error}")),
                    cx,
                ),
            })
        })
        .detach();
}

/// The quit an update asked for was cancelled: the update is discarded, its files removed off
/// this thread, and the user told.
pub(crate) fn discard_staged(app: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    if let Some(staged) = app.update(cx, |app, _| app.staged_update.take()) {
        cx.background_spawn(async move { drop(staged) }).detach();
        window.push_notification(
            Notification::warning("AdocFoundry was not updated.").id::<UpdateNotice>(),
            cx,
        );
    }
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
fn download(asset: &Asset, checksum: &str, directory: &Path) -> Result<PathBuf, String> {
    let curl = curl().ok_or_else(|| "curl was not found".to_owned())?;
    // The asset's name comes from the feed: keep only its last component.
    let name = Path::new(&asset.name)
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

/// Stages the package `asset` beside the running app, giving what puts it in place once the
/// app quits. Each platform's way is compiled everywhere, so every build checks it.
fn stage(asset: &Asset, checksum: &str) -> Result<Staged, String> {
    if cfg!(target_os = "macos") {
        stage_macos(asset, checksum)
    } else if cfg!(windows) {
        stage_windows(asset, checksum)
    } else {
        Err("this platform's package manager updates AdocFoundry".to_owned())
    }
}

/// Copies the DMG's app, checked first, beside the running bundle; finishing swaps the two and
/// relaunches.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn stage_macos(asset: &Asset, checksum: &str) -> Result<Staged, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let bundle = executable
        .ancestors()
        .nth(3)
        .ok_or_else(|| "the app is not in a bundle".to_owned())?
        .to_path_buf();
    let team = |app: &Path| {
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
    let _ = std::fs::remove_dir_all(&directory);
    if let Err(error) = copied {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    let leftovers = vec![staged.clone()];
    Ok(Staged::new(
        move || finish_macos(&bundle, &staged),
        leftovers,
    ))
}

/// Swaps the staged bundle in for the running one, by renaming within the bundle's folder (the
/// running app keeps its open files), and opens the bundle again once this process has gone,
/// whether or not the swap succeeded.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn finish_macos(bundle: &Path, staged: &Path) -> Result<(), String> {
    let old = bundle.with_file_name(".AdocFoundry-old.app");
    let swapped = (|| {
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(bundle, &old).map_err(|error| format!("{}: {error}", bundle.display()))?;
        if let Err(error) = std::fs::rename(staged, bundle) {
            let _ = std::fs::rename(&old, bundle);
            return Err(format!("{}: {error}", bundle.display()));
        }
        Ok(())
    })();
    if swapped.is_err() {
        let _ = std::fs::remove_dir_all(staged);
    }
    relaunch_macos(std::process::id(), bundle, &old)
        .spawn()
        .map_err(|error| format!("the app could not be opened again: {error}"))?;
    swapped
}

/// Waits for the process `pid` to exit, removes the old bundle and opens `bundle`. Paths are
/// the script's arguments, never part of its text.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn relaunch_macos(pid: u32, bundle: &Path, old: &Path) -> Command {
    let mut relaunch = Command::new("/bin/sh");
    relaunch
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; rm -rf \"$3\"; open \"$2\"",
        ])
        .arg("sh")
        .arg(pid.to_string())
        .arg(bundle)
        .arg(old)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    relaunch
}

/// `text` as a PowerShell string literal. PowerShell also takes the typographic single quotes
/// for `'`, so each is doubled too.
fn powershell_literal(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('\'');
    for character in text.chars() {
        if matches!(
            character,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}'
        ) {
            literal.push(character);
        }
        literal.push(character);
    }
    literal.push('\'');
    literal
}

/// The Authenticode status of `path` and its signer's subject.
#[cfg_attr(not(windows), allow(dead_code))]
fn signer(path: &Path) -> Result<(String, String), String> {
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

/// Downloads the MSI and checks it; finishing starts the step that installs it once the app
/// has quit, then starts the app again. Its major upgrade replaces this version.
#[cfg_attr(not(windows), allow(dead_code))]
fn stage_windows(asset: &Asset, checksum: &str) -> Result<Staged, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    // The MSI installs `bin\adocfoundry.exe` under the folder chosen for it.
    let folder = executable
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "the app is not in an installed folder".to_owned())?
        .to_path_buf();
    let (status, ours) = signer(&executable)?;
    if status != "Valid" || ours.is_empty() {
        return Err("this build is not signed, so it cannot check a signature".to_owned());
    }
    let directory = scratch()?;
    let checked = download(asset, checksum, &directory).and_then(|package| {
        let (status, theirs) = signer(&package)?;
        if status == "Valid" && theirs == ours {
            Ok(package)
        } else {
            Err("the installer is not signed as this build is".to_owned())
        }
    });
    let package = match checked {
        Ok(package) => package,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&directory);
            return Err(error);
        }
    };
    let leftovers = vec![directory.clone()];
    Ok(Staged::new(
        move || {
            let mut finishing = finish_windows(
                std::process::id(),
                &package,
                &folder,
                &executable,
                &directory,
            );
            detach(&mut finishing);
            finishing
                .spawn()
                .map(drop)
                .map_err(|error| format!("the installer could not start: {error}"))
        },
        leftovers,
    ))
}

/// Runs `command` with no console window and none of this process's standard streams, so it
/// carries on once the app has gone.
#[cfg_attr(not(windows), allow(dead_code))]
fn detach(command: &mut Command) {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// The step that finishes an update on Windows: PowerShell, given its script base64-encoded so
/// that no path passes through a command line's quoting (and none through `cmd.exe`).
#[cfg_attr(not(windows), allow(dead_code))]
fn finish_windows(
    pid: u32,
    package: &Path,
    folder: &Path,
    executable: &Path,
    scratch: &Path,
) -> Command {
    use base64::Engine as _;
    let script = finish_windows_script(pid, package, folder, executable, scratch);
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        &base64::engine::general_purpose::STANDARD.encode(utf16),
    ]);
    command
}

/// Waits for the process `pid` (the app) to exit, runs the MSI `package` over the installed
/// `folder`, removes `scratch` and starts `executable` again, unless the installer is
/// restarting Windows (exit code 1641). An installation that failed or was cancelled leaves
/// the old version, which starts again.
///
/// Every path is a PowerShell literal. `msiexec` is started directly, never through a shell,
/// with each path in double quotes, which a Windows path cannot contain.
#[cfg_attr(not(windows), allow(dead_code))]
fn finish_windows_script(
    pid: u32,
    package: &Path,
    folder: &Path,
    executable: &Path,
    scratch: &Path,
) -> String {
    let quoted = |path: &Path| format!("\"{}\"", path.display());
    let arguments = format!(
        "/i {} /passive APPLICATIONFOLDER={}",
        quoted(package),
        quoted(folder)
    );
    let literal = |path: &Path| powershell_literal(&path.to_string_lossy());
    [
        "$ErrorActionPreference = 'SilentlyContinue'".to_owned(),
        format!("Wait-Process -Id {pid}"),
        "$installer = New-Object System.Diagnostics.ProcessStartInfo 'msiexec.exe'".to_owned(),
        format!("$installer.Arguments = {}", powershell_literal(&arguments)),
        "$installer.UseShellExecute = $false".to_owned(),
        "$run = [System.Diagnostics.Process]::Start($installer)".to_owned(),
        "$run.WaitForExit()".to_owned(),
        format!(
            "Remove-Item -LiteralPath {} -Recurse -Force",
            literal(scratch)
        ),
        "if ($run.ExitCode -ne 1641) {".to_owned(),
        format!(
            "  $app = New-Object System.Diagnostics.ProcessStartInfo {}",
            literal(executable)
        ),
        "  $app.UseShellExecute = $false".to_owned(),
        "  [void][System.Diagnostics.Process]::Start($app)".to_owned(),
        "}".to_owned(),
    ]
    .join("\n")
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
        // PowerShell ends a literal at a typographic quote as well.
        assert_eq!(powershell_literal("Jo\u{2019}s"), "'Jo\u{2019}\u{2019}s'");
    }

    /// The paths an update meets on Windows may hold spaces and what `cmd.exe` would read as
    /// commands or variables (`&`, `^`, `%`), and `'`, which ends a PowerShell literal.
    #[test]
    fn the_windows_update_waits_for_the_app_installs_and_starts_it_again() {
        use base64::Engine as _;
        let scratch = Path::new(r"C:\Users\Jo & Co\100%PATH%'s ^update");
        let package =
            Path::new(r"C:\Users\Jo & Co\100%PATH%'s ^update\adocfoundry-1.0.0-x86_64.msi");
        let folder = Path::new(r"C:\Program Files\Adoc & 100%'s");
        let executable = Path::new(r"C:\Program Files\Adoc & 100%'s\bin\adocfoundry.exe");
        let command = finish_windows(4242, package, folder, executable, scratch);

        assert_eq!(command.get_program(), "powershell");
        let arguments: Vec<&str> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(
            arguments[..3],
            ["-NoProfile", "-NonInteractive", "-EncodedCommand"]
        );
        assert_eq!(arguments.len(), 4);
        // Nothing but base64 reaches the command line, so no path is quoted or read there.
        assert!(arguments[3]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(arguments[3])
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let script = String::from_utf16(&units).unwrap();
        assert_eq!(
            script,
            finish_windows_script(4242, package, folder, executable, scratch)
        );

        let lines: Vec<&str> = script.lines().map(str::trim).collect();
        let at = |text: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(text))
                .unwrap_or_else(|| panic!("no line starts {text}: {script}"))
        };
        // The app is gone before the installer runs, and the installer done before the app
        // starts again.
        let waited = at("Wait-Process -Id 4242");
        let installed = at("$run = [System.Diagnostics.Process]::Start($installer)");
        let finished = at("$run.WaitForExit()");
        let started = at("[void][System.Diagnostics.Process]::Start($app)");
        assert!(waited < installed && installed < finished && finished < started);
        assert_eq!(
            lines[at("$installer.Arguments")],
            r#"$installer.Arguments = '/i "C:\Users\Jo & Co\100%PATH%''s ^update\adocfoundry-1.0.0-x86_64.msi" /passive APPLICATIONFOLDER="C:\Program Files\Adoc & 100%''s"'"#
        );
        assert!(lines.contains(
            &r"$app = New-Object System.Diagnostics.ProcessStartInfo 'C:\Program Files\Adoc & 100%''s\bin\adocfoundry.exe'"
        ));
        assert!(lines.contains(
            &r"Remove-Item -LiteralPath 'C:\Users\Jo & Co\100%PATH%''s ^update' -Recurse -Force"
        ));
        assert!(lines.contains(&"$installer.UseShellExecute = $false"));
        assert!(lines.contains(&"$app.UseShellExecute = $false"));
    }

    /// A staged update that is not finished removes what it staged; one that is finished leaves
    /// it to the step that installs it.
    #[test]
    fn a_staged_update_removes_its_files_only_when_discarded() {
        let directory = tempfile::tempdir().unwrap();
        let discarded = directory.path().join("discarded");
        let kept = directory.path().join("kept");
        for folder in [&discarded, &kept] {
            std::fs::create_dir(folder).unwrap();
            std::fs::write(folder.join("package"), "").unwrap();
        }
        drop(Staged::new(|| Ok(()), vec![discarded.clone()]));
        assert!(!discarded.exists());

        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let running = ran.clone();
        Staged::new(
            move || {
                running.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            vec![kept.clone()],
        )
        .finish()
        .unwrap();
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(kept.join("package").exists());
    }

    /// On macOS the bundle is opened again only once this process has gone, and the paths are
    /// the script's arguments, not its text.
    #[test]
    fn the_macos_relaunch_waits_for_the_app_to_exit() {
        let bundle = Path::new("/Applications/It's \"Adoc\" $(x).app");
        let old = Path::new("/Applications/.AdocFoundry-old.app");
        let relaunch = relaunch_macos(4242, bundle, old);
        assert_eq!(relaunch.get_program(), "/bin/sh");
        let arguments: Vec<&std::ffi::OsStr> = relaunch.get_args().collect();
        assert_eq!(arguments[0], "-c");
        let script = arguments[1].to_str().unwrap();
        assert!(script.starts_with("while kill -0 \"$1\""));
        assert!(!script.contains("Adoc"));
        assert_eq!(
            arguments[2..],
            [
                "sh".as_ref(),
                "4242".as_ref(),
                bundle.as_os_str(),
                old.as_os_str()
            ]
        );
    }

    #[test]
    fn the_shipped_curl_is_found() {
        if cfg!(target_os = "macos") {
            assert_eq!(curl(), Some(PathBuf::from("/usr/bin/curl")));
        }
    }
}
