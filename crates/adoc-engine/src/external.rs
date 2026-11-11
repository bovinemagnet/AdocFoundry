//! What a link may hand to the operating system to open (#495). A page can name anything, and a
//! page may come from a repository the user only cloned to read, so following a link must never
//! run a program: the system opens only web and mail addresses (in the browser or mail client)
//! and PDFs inside the workspace (in its viewer). Images and text open in AdocFoundry, which only
//! displays them; anything else is refused, and can still be opened deliberately from the Files
//! panel's Open Externally.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Engine;

/// The URL schemes handed to the system, in lower case: the browser's and the mail client's.
/// Every other scheme is refused: `file:` opens whatever it names with the application for it,
/// and a custom scheme (`vscode:`, `ms-settings:`, `smb:`) is a registered program's to act on.
pub const WEB_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The extensions, in lower case, of the files a link may hand to the system's own viewer:
/// documents that viewers only display, as the Files panel opens on a click.
pub const SYSTEM_VIEWER_EXTENSIONS: &[&str] = &["pdf"];

/// Whether `url` is a web or mail address the system may open: `http://` or `https://` with a
/// host, or `mailto:` with an address, in any case, with no white space or control character.
/// A relative address, a path, a UNC path and any other scheme are refused.
#[must_use]
pub fn is_web_address(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if !WEB_SCHEMES.contains(&scheme.as_str()) {
        return false;
    }
    if scheme == "mailto" {
        return !rest.is_empty();
    }
    rest.strip_prefix("//")
        .is_some_and(|after| !after.is_empty() && !after.starts_with('/'))
}

impl Engine {
    /// The file to hand the system's viewer for a link to `path`: a regular file whose real
    /// name, once links are followed, has an extension in [`SYSTEM_VIEWER_EXTENSIONS`] and lies
    /// inside a workspace root. `None` for anything else, including a file that does not
    /// exist, a folder (a macOS application bundle is one) and a link to a program.
    #[must_use]
    pub fn system_viewer_path(&self, path: &Path) -> Option<PathBuf> {
        let real = fs::canonicalize(path).ok()?;
        if !fs::metadata(&real).ok()?.is_file() {
            return None;
        }
        let viewable = real
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                SYSTEM_VIEWER_EXTENSIONS
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(extension))
            });
        let inside = self
            .roots()
            .iter()
            .any(|root| fs::canonicalize(root).is_ok_and(|root| real.starts_with(root)));
        (viewable && inside).then_some(real)
    }
}
