//! What the paths a front end is launched with, or handed by the system, open (#498): a folder
//! as the workspace, and files in tabs.

use std::path::{Path, PathBuf};

use crate::settings::WORKSPACE_SETTINGS_FILE;

/// The folder to open as the workspace, and the files to open in tabs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchTargets {
    pub folder: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// What `paths` open, as `adocfoundry [folder] [file…]` takes them: the first is the folder
/// when it is one; every other path is a file. Files with no folder open in the workspace the
/// first belongs to ([`workspace_for_file`]). Relative paths are taken from the current folder.
#[must_use]
pub fn launch_targets(paths: impl IntoIterator<Item = PathBuf>) -> LaunchTargets {
    let mut paths = paths
        .into_iter()
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .peekable();
    let folder = paths.next_if(|path| path.is_dir());
    let files: Vec<PathBuf> = paths.collect();
    let folder = folder.or_else(|| files.first().map(|file| workspace_for_file(file)));
    LaunchTargets { folder, files }
}

/// The folder to open as the workspace for `file`: the nearest folder above it holding an
/// Antora playbook or workspace settings, else the nearest holding an `antora.yml`, else the
/// file's own folder.
#[must_use]
pub fn workspace_for_file(file: &Path) -> PathBuf {
    let holding = |names: &[&str]| {
        file.ancestors()
            .skip(1)
            .find(|folder| names.iter().any(|name| folder.join(name).is_file()))
            .map(Path::to_path_buf)
    };
    holding(&[adoc_antora::PLAYBOOK_FILE, WORKSPACE_SETTINGS_FILE])
        .or_else(|| holding(&["antora.yml"]))
        .or_else(|| file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| file.to_path_buf())
}

/// The path a `file://` URL names, as the system hands over a file opened from it (Finder, on
/// macOS); `None` for any other URL, or a file on another host.
#[must_use]
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = match rest.find('/')? {
        0 => rest,
        at if &rest[..at] == "localhost" => &rest[at..],
        _ => return None,
    };
    let mut bytes = Vec::with_capacity(path.len());
    let mut encoded = path.bytes();
    while let Some(byte) = encoded.next() {
        if byte == b'%' {
            let digits = [encoded.next()?, encoded.next()?];
            let hex = std::str::from_utf8(&digits).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}
