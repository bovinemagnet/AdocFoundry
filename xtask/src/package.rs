//! `cargo xtask package`: a release build of the three binaries, staged with the licence, README,
//! desktop entry and icon into `dist/` and archived for the host platform.
//!
//! The release workflow builds the platform installers (DMG, MSI, AppImage, deb, rpm); this task
//! produces the same binaries locally as a dry run.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const BINARIES: &[&str] = &["adocfoundry", "adocfoundry-cli", "adoc-ls", "adoc-mcp"];

fn workspace_version(root: &Path) -> Result<String, String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|error| error.to_string())?;
    let manifest: toml::Table = text.parse().map_err(|error| format!("{error}"))?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "workspace.package.version is missing".to_owned())
}

fn run(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{command:?}: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{command:?} failed with {status}"))
}

/// The archive name for this host, such as `adocfoundry-0.1.0-linux-x86_64`.
pub fn archive_name(version: &str) -> String {
    format!(
        "adocfoundry-{version}-{}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

pub fn run_package(root: &Path) -> Result<PathBuf, String> {
    let version = workspace_version(root)?;
    let mut build = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    build.current_dir(root).args(["build", "--release"]);
    for binary in BINARIES {
        build.args(["-p", binary]);
    }
    // The packaged language server is the full one, as the release workflow builds it.
    build.args(["--features", "adoc-ls/typst,adoc-ls/git"]);
    run(&mut build)?;

    let name = archive_name(&version);
    let dist = root.join("dist");
    let staging = dist.join(&name);
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let copy = |from: PathBuf, to: &str| -> Result<(), String> {
        fs::copy(&from, staging.join(to))
            .map(|_| ())
            .map_err(|error| format!("{}: {error}", from.display()))
    };
    for binary in BINARIES {
        let file = format!("{binary}{}", std::env::consts::EXE_SUFFIX);
        copy(root.join("target/release").join(&file), &file)?;
    }
    copy(root.join("LICENSE"), "LICENSE")?;
    copy(root.join("README.md"), "README.md")?;
    if cfg!(target_os = "linux") {
        copy(
            root.join("packaging/linux/io.github.bovinemagnet.AdocFoundry.desktop"),
            "io.github.bovinemagnet.AdocFoundry.desktop",
        )?;
        copy(
            root.join("packaging/icons/adocfoundry-256.png"),
            "io.github.bovinemagnet.AdocFoundry.png",
        )?;
    }

    let archive = dist.join(format!("{name}.tar.gz"));
    run(Command::new("tar")
        .current_dir(&dist)
        .arg("-czf")
        .arg(&archive)
        .arg(&name))?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_workspace_version() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let version = workspace_version(&root).unwrap();
        assert_eq!(version.split('.').count(), 3);
        assert!(archive_name(&version).starts_with(&format!("adocfoundry-{version}-")));
    }

    /// The app updates itself on Windows by running a newer MSI, which must replace the
    /// installed version: one fixed upgrade code, a new product code each build, and a major
    /// upgrade.
    #[test]
    fn msi_has_a_major_upgrade() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let wix = fs::read_to_string(root.join("crates/adocfoundry/wix/main.wxs")).unwrap();
        assert!(wix.contains("<MajorUpgrade "), "no MajorUpgrade");
        assert!(wix.contains("UpgradeCode=\"6B1D7C52-6E31-4C1E-9F0B-3D7A2C1E5A40\""));
        assert!(
            wix.contains("Id=\"*\""),
            "the product code must change with each build"
        );
    }
}
