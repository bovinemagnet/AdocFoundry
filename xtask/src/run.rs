//! `cargo xtask run [--release] [-- <args>]`: builds the desktop app and runs it.
//!
//! On macOS a bare executable shows the generic one in the Dock, so the binary is run from a
//! small development bundle in the target folder, `AdocFoundry.app`, with the release's icon.
//! An executable started from inside a bundle is that bundle's app, so the Dock, the app
//! switcher and the menu bar show AdocFoundry, and its output still reaches the terminal.
//! Elsewhere this is `cargo run -p adocfoundry`.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

/// What `cargo xtask run` was asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub release: bool,
    /// The arguments passed to the app.
    pub args: Vec<OsString>,
}

/// Reads the arguments after `run`: `--release` before an optional `--`, the app's arguments
/// after it (or every other argument when there is no `--`).
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Request {
    let mut release = false;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            rest.extend(args);
            break;
        }
        if arg == "--release" {
            release = true;
        } else {
            rest.push(arg);
        }
    }
    Request {
        release,
        args: rest,
    }
}

/// The release's `Info.plist` made a development bundle's: its own identifier, so the system
/// never mistakes it for the installed app, and no document types, so it never becomes the
/// app that opens AsciiDoc files from the Finder.
pub fn dev_info_plist(template: &str, version: &str) -> Result<String, String> {
    const IDENTIFIER: &str =
        "<key>CFBundleIdentifier</key><string>io.github.bovinemagnet.AdocFoundry</string>";
    const DEV_IDENTIFIER: &str =
        "<key>CFBundleIdentifier</key><string>io.github.bovinemagnet.AdocFoundry.dev</string>";
    const TYPES: &str = "  <key>CFBundleDocumentTypes</key>\n";
    if !template.contains(IDENTIFIER) {
        return Err("Info.plist: no CFBundleIdentifier to replace".to_owned());
    }
    let mut plist = template
        .replace(IDENTIFIER, DEV_IDENTIFIER)
        .replace("@VERSION@", version);
    if let Some(start) = plist.find(TYPES) {
        // The document types are the last entry: an array closed at the dictionary's end.
        let end = plist[start..]
            .find("  </array>\n")
            .map(|at| start + at + "  </array>\n".len())
            .ok_or("Info.plist: CFBundleDocumentTypes is not closed")?;
        plist.replace_range(start..end, "");
    }
    Ok(plist)
}

/// Builds the app and runs it, returning its exit code.
pub fn run(root: &Path, cargo: &OsString, request: Request) -> Result<i32, String> {
    let mut build = Command::new(cargo);
    build.current_dir(root).args(["build", "-p", "adocfoundry"]);
    if request.release {
        build.arg("--release");
    }
    let status = build
        .status()
        .map_err(|error| format!("cargo build: {error}"))?;
    if !status.success() {
        return Err("cargo build failed".to_owned());
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let profile = target.join(if request.release { "release" } else { "debug" });
    let executable = if cfg!(target_os = "macos") {
        bundle(root, &profile)?
    } else {
        profile.join(format!("adocfoundry{}", std::env::consts::EXE_SUFFIX))
    };
    let status = Command::new(&executable)
        .args(&request.args)
        .status()
        .map_err(|error| format!("{}: {error}", executable.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Makes or refreshes `<profile>/AdocFoundry.app` around the binary just built, returning the
/// executable inside it.
fn bundle(root: &Path, profile: &Path) -> Result<PathBuf, String> {
    let app = profile.join("AdocFoundry.app");
    let contents = app.join("Contents");
    let (macos, resources) = (contents.join("MacOS"), contents.join("Resources"));
    for folder in [&macos, &resources] {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    let packaging = root.join("packaging");
    let template = read(&packaging.join("macos/Info.plist"))?;
    let plist = dev_info_plist(&template, env!("CARGO_PKG_VERSION"))?;
    write_if_changed(&contents.join("Info.plist"), &plist)?;

    // Linked rather than copied, so a debug binary of a few hundred megabytes costs nothing.
    let binary = profile.join("adocfoundry");
    let executable = macos.join("adocfoundry");
    let _ = std::fs::remove_file(&executable);
    std::fs::hard_link(&binary, &executable)
        .or_else(|_| std::fs::copy(&binary, &executable).map(|_| ()))
        .map_err(|error| format!("{}: {error}", executable.display()))?;

    let master = packaging.join("icons/adocfoundry.png");
    let icon = resources.join("AdocFoundry.icns");
    if is_older(&icon, &master) {
        make_icns(&master, &icon, profile)?;
    }
    Ok(executable)
}

/// An `.icns` from the 1024-pixel master at every size macOS asks for, as
/// `packaging/macos/bundle.sh` makes the release's.
fn make_icns(master: &Path, icon: &Path, scratch: &Path) -> Result<(), String> {
    let iconset = scratch.join("AdocFoundry.iconset");
    let _ = std::fs::remove_dir_all(&iconset);
    std::fs::create_dir_all(&iconset).map_err(|error| format!("{}: {error}", iconset.display()))?;
    for size in [16, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let pixels = (size * scale).to_string();
            let out = iconset.join(format!("icon_{size}x{size}{suffix}.png"));
            tool(
                Command::new("sips")
                    .args(["-z", &pixels, &pixels])
                    .arg(master)
                    .arg("--out")
                    .arg(&out),
            )?;
        }
    }
    tool(
        Command::new("iconutil")
            .args(["-c", "icns"])
            .arg(&iconset)
            .arg("-o")
            .arg(icon),
    )?;
    let _ = std::fs::remove_dir_all(&iconset);
    Ok(())
}

fn tool(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{:?}: {error}", command.get_program()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?}: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Whether `made` is missing or older than `source`.
fn is_older(made: &Path, source: &Path) -> bool {
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    };
    match (modified(made), modified(source)) {
        (Some(made), Some(source)) => made < source,
        _ => true,
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn release_and_the_apps_arguments_are_told_apart() {
        assert_eq!(
            parse(args(&["--release", "--", "docs", "--release"])),
            Request {
                release: true,
                args: args(&["docs", "--release"]),
            }
        );
        assert_eq!(
            parse(args(&["docs/page.adoc"])),
            Request {
                release: false,
                args: args(&["docs/page.adoc"]),
            }
        );
    }

    #[test]
    fn the_dev_bundle_is_not_the_installed_app() {
        let template = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/macos/Info.plist"),
        )
        .unwrap();
        let plist = dev_info_plist(&template, "1.2.3").unwrap();
        assert!(plist.contains("<string>io.github.bovinemagnet.AdocFoundry.dev</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key><string>AdocFoundry</string>"));
        assert!(plist.contains("<key>CFBundleShortVersionString</key><string>1.2.3</string>"));
        assert!(!plist.contains("@VERSION@"));
        assert!(!plist.contains("CFBundleDocumentTypes"));
        assert!(!plist.contains("adoc</string>"));
        assert!(plist.trim_end().ends_with("</dict>\n</plist>"));
    }

    #[test]
    fn a_template_without_an_identifier_is_refused() {
        assert!(dev_info_plist("<plist/>", "1.0.0").is_err());
    }
}
