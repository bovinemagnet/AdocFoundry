//! Delivery of rendered AsciiDoc to the user.
//!
//! Rendering produces HTML; this module decides where that HTML goes. The
//! [`PreviewSink`] seam exists so the destination can change — an external browser
//! today, a preview pane if Zed ever exposes one — without disturbing the renderer,
//! the code action, or the command handler above it.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    process,
};

use adoc_render::{RenderError, RenderOutput};

#[derive(Debug)]
pub enum PreviewError {
    NotOpen(String),
    Render(RenderError),
    Io(io::Error),
    LaunchFailed {
        program: String,
        status: Option<i32>,
    },
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(uri) => write!(formatter, "document is not open: {uri}"),
            Self::Render(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::LaunchFailed { program, status } => write!(
                formatter,
                "could not open the preview with `{program}` (status {status:?})"
            ),
        }
    }
}

impl std::error::Error for PreviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::NotOpen(_) | Self::LaunchFailed { .. } => None,
        }
    }
}

impl From<RenderError> for PreviewError {
    fn from(error: RenderError) -> Self {
        Self::Render(error)
    }
}

impl From<io::Error> for PreviewError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Directory for rewritten copies of included files.
///
/// Separate from the artefact directory so preview output stays browsable without
/// intermediate files mixed in.
#[must_use]
pub fn scratch_directory() -> PathBuf {
    std::env::temp_dir().join("adoc-ls-preview-includes")
}

/// Where rendered HTML is delivered.
pub trait PreviewSink: Send + Sync {
    /// Deliver `output` for `source`, returning the artefact that was produced.
    fn deliver(&self, output: &RenderOutput, source: &Path) -> Result<PathBuf, PreviewError>;
}

/// Opens a written artefact. Separated from [`BrowserSink`] so tests never open a browser.
pub trait Launcher: Send + Sync {
    fn launch(&self, path: &Path) -> Result<(), PreviewError>;
}

/// Hands the artefact to the platform's default handler.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLauncher;

impl Launcher for SystemLauncher {
    fn launch(&self, path: &Path) -> Result<(), PreviewError> {
        let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("open", &[])
        } else if cfg!(target_os = "windows") {
            ("cmd", &["/C", "start", ""])
        } else {
            ("xdg-open", &[])
        };

        let status = process::Command::new(program)
            .args(args)
            .arg(path)
            .status()
            .map_err(PreviewError::Io)?;

        if status.success() {
            Ok(())
        } else {
            Err(PreviewError::LaunchFailed {
                program: program.to_owned(),
                status: status.code(),
            })
        }
    }
}

/// Writes HTML to a directory and opens it outside the editor.
#[derive(Clone, Debug)]
pub struct BrowserSink<L: Launcher> {
    directory: PathBuf,
    launcher: L,
}

impl<L: Launcher> BrowserSink<L> {
    pub fn new(directory: impl Into<PathBuf>, launcher: L) -> Self {
        Self {
            directory: directory.into(),
            launcher,
        }
    }

    #[must_use]
    pub fn launcher(&self) -> &L {
        &self.launcher
    }

    /// Artefact path for `source`. Stable across renders so the browser tab can be reloaded.
    fn artefact_path(&self, source: &Path) -> PathBuf {
        let stem = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("preview");
        self.directory.join(format!("{stem}.html"))
    }
}

impl<L: Launcher> PreviewSink for BrowserSink<L> {
    fn deliver(&self, output: &RenderOutput, source: &Path) -> Result<PathBuf, PreviewError> {
        fs::create_dir_all(&self.directory)?;
        let artefact = self.artefact_path(source);
        fs::write(&artefact, &output.html)?;
        self.launcher.launch(&artefact)?;
        Ok(artefact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adoc_render::RenderOutput;
    use std::sync::Mutex;

    fn output(html: &str) -> RenderOutput {
        RenderOutput {
            html: html.to_owned(),
            warnings: Vec::new(),
        }
    }

    #[derive(Default)]
    struct RecordingLauncher {
        launched: Mutex<Vec<PathBuf>>,
    }

    impl Launcher for RecordingLauncher {
        fn launch(&self, path: &Path) -> Result<(), PreviewError> {
            self.launched.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    #[test]
    fn writes_html_next_to_a_stable_name_derived_from_the_source() {
        let dir = tempdir("writes-html");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(&output("<h1>Guide</h1>"), Path::new("/docs/guide.adoc"))
            .expect("deliver");

        assert_eq!(artefact.file_name().unwrap(), "guide.html");
        assert_eq!(fs::read_to_string(&artefact).unwrap(), "<h1>Guide</h1>");
    }

    #[test]
    fn opens_the_artefact_it_just_wrote() {
        let dir = tempdir("opens-artefact");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(&output("<p>x</p>"), Path::new("/docs/guide.adoc"))
            .expect("deliver");

        assert_eq!(
            sink.launcher().launched.lock().unwrap().as_slice(),
            &[artefact]
        );
    }

    #[test]
    fn rewriting_the_same_source_reuses_the_same_artefact_path() {
        let dir = tempdir("reuses-path");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());
        let source = Path::new("/docs/guide.adoc");

        let first = sink.deliver(&output("<p>one</p>"), source).expect("first");
        let second = sink.deliver(&output("<p>two</p>"), source).expect("second");

        assert_eq!(first, second);
        assert_eq!(fs::read_to_string(&second).unwrap(), "<p>two</p>");
    }

    #[test]
    fn a_source_without_a_stem_still_produces_an_artefact() {
        let dir = tempdir("no-stem");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(&output("<p>x</p>"), Path::new("/"))
            .expect("deliver");

        assert_eq!(artefact.file_name().unwrap(), "preview.html");
    }

    /// A directory unique to one test: these run in parallel and share a filename.
    fn tempdir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adoc-ls-{}-{test}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
