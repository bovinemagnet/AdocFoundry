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

/// Whether a preview is a one-off or is expected to keep up with the buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewMode {
    /// Rendered once. The artefact is exactly what the renderer produced.
    Static,
    /// Rendered repeatedly. The artefact carries a reloader so the page keeps up.
    Live,
}

/// Where rendered HTML is delivered.
pub trait PreviewSink: Send + Sync {
    /// Deliver `output` for `source` and present it, returning the artefact produced.
    fn deliver(
        &self,
        output: &RenderOutput,
        source: &Path,
        mode: PreviewMode,
    ) -> Result<PathBuf, PreviewError>;

    /// Update an artefact already on screen, without presenting it again.
    fn refresh(&self, output: &RenderOutput, source: &Path) -> Result<PathBuf, PreviewError>;
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

impl<L: Launcher> BrowserSink<L> {
    fn write(
        &self,
        output: &RenderOutput,
        source: &Path,
        mode: PreviewMode,
    ) -> Result<PathBuf, PreviewError> {
        fs::create_dir_all(&self.directory)?;
        let artefact = self.artefact_path(source);
        let html = match mode {
            PreviewMode::Static => output.html.clone(),
            PreviewMode::Live => with_reloader(&output.html),
        };
        fs::write(&artefact, html)?;
        Ok(artefact)
    }
}

impl<L: Launcher> PreviewSink for BrowserSink<L> {
    fn deliver(
        &self,
        output: &RenderOutput,
        source: &Path,
        mode: PreviewMode,
    ) -> Result<PathBuf, PreviewError> {
        let artefact = self.write(output, source, mode)?;
        self.launcher.launch(&artefact)?;
        Ok(artefact)
    }

    fn refresh(&self, output: &RenderOutput, source: &Path) -> Result<PathBuf, PreviewError> {
        // Always live: only a live preview is ever refreshed, and the reloader has to
        // survive the rewrite or the page stops keeping up after the first save.
        self.write(output, source, PreviewMode::Live)
    }
}

/// How often a live preview re-reads itself, in seconds.
const RELOAD_INTERVAL: u8 = 2;

/// `html` with a reloader, and scroll position preserved across the reload.
///
/// A `file://` page cannot fetch its own source to test for changes — browsers block it —
/// so this reloads on a timer rather than on change. `sessionStorage` is unavailable on
/// `file://` in some browsers, hence the `try`.
fn with_reloader(html: &str) -> String {
    let reloader = format!(
        "<meta http-equiv=\"refresh\" content=\"{RELOAD_INTERVAL}\">\n\
         <script>try{{addEventListener('beforeunload',function(){{\
         sessionStorage.setItem('adoc-ls-scroll',String(scrollY))}});\
         addEventListener('load',function(){{\
         scrollTo(0,Number(sessionStorage.getItem('adoc-ls-scroll'))||0)}})}}catch(e){{}}</script>\n"
    );

    match html.find("</head>") {
        Some(index) => {
            let mut out = String::with_capacity(html.len() + reloader.len());
            out.push_str(&html[..index]);
            out.push_str(&reloader);
            out.push_str(&html[index..]);
            out
        }
        // A fragment without a head still reloads if the tag leads.
        None => reloader + html,
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
    fn a_static_preview_is_delivered_verbatim() {
        let dir = tempdir("static-verbatim");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<h1>G</h1>"),
                Path::new("/docs/g.adoc"),
                PreviewMode::Static,
            )
            .expect("deliver");

        assert_eq!(fs::read_to_string(&artefact).unwrap(), "<h1>G</h1>");
    }

    #[test]
    fn a_live_preview_carries_a_reloader() {
        let dir = tempdir("live-reloader");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<h1>G</h1>"),
                Path::new("/docs/g.adoc"),
                PreviewMode::Live,
            )
            .expect("deliver");

        let html = fs::read_to_string(&artefact).unwrap();
        assert!(html.contains("<h1>G</h1>"), "{html}");
        assert!(html.contains("http-equiv=\"refresh\""), "{html}");
    }

    #[test]
    fn refreshing_rewrites_the_artefact_without_presenting_it() {
        let dir = tempdir("refresh-silent");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());
        let source = Path::new("/docs/g.adoc");
        let first = sink
            .deliver(&output("<p>one</p>"), source, PreviewMode::Live)
            .expect("deliver");

        let again = sink
            .refresh(&output("<p>two</p>"), source)
            .expect("refresh");

        assert_eq!(first, again);
        assert!(fs::read_to_string(&again).unwrap().contains("<p>two</p>"));
        // Presented once, by the initial deliver; a refresh must not steal focus.
        assert_eq!(sink.launcher().launched.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_refreshed_artefact_keeps_its_reloader() {
        let dir = tempdir("refresh-reloader");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());
        let source = Path::new("/docs/g.adoc");
        sink.deliver(&output("<p>one</p>"), source, PreviewMode::Live)
            .expect("deliver");

        let again = sink
            .refresh(&output("<p>two</p>"), source)
            .expect("refresh");

        assert!(fs::read_to_string(&again)
            .unwrap()
            .contains("http-equiv=\"refresh\""));
    }

    #[test]
    fn writes_html_next_to_a_stable_name_derived_from_the_source() {
        let dir = tempdir("writes-html");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<h1>Guide</h1>"),
                Path::new("/docs/guide.adoc"),
                PreviewMode::Static,
            )
            .expect("deliver");

        assert_eq!(artefact.file_name().unwrap(), "guide.html");
        assert_eq!(fs::read_to_string(&artefact).unwrap(), "<h1>Guide</h1>");
    }

    #[test]
    fn opens_the_artefact_it_just_wrote() {
        let dir = tempdir("opens-artefact");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<p>x</p>"),
                Path::new("/docs/guide.adoc"),
                PreviewMode::Static,
            )
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

        let first = sink
            .deliver(&output("<p>one</p>"), source, PreviewMode::Static)
            .expect("first");
        let second = sink
            .deliver(&output("<p>two</p>"), source, PreviewMode::Static)
            .expect("second");

        assert_eq!(first, second);
        assert_eq!(fs::read_to_string(&second).unwrap(), "<p>two</p>");
    }

    #[test]
    fn a_source_without_a_stem_still_produces_an_artefact() {
        let dir = tempdir("no-stem");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(&output("<p>x</p>"), Path::new("/"), PreviewMode::Static)
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
