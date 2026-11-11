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
    sync::OnceLock,
};

use adoc_render::{RenderError, RenderOutput};

#[derive(Debug)]
pub enum PreviewError {
    NotOpen(String),
    /// The engine did not finish rendering the page in time.
    NotRendered(PathBuf),
    Render(RenderError),
    Io(io::Error),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen(uri) => write!(formatter, "document is not open: {uri}"),
            Self::NotRendered(path) => write!(formatter, "{} did not render", path.display()),
            Self::Render(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PreviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::NotOpen(_) | Self::NotRendered(_) => None,
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

    /// Where rewritten copies of included files go: apart from the artefacts, so the preview
    /// folder stays browsable without them mixed in.
    fn scratch_directory(&self) -> Result<PathBuf, PreviewError>;
}

/// Opens a written artefact. Separated from [`BrowserSink`] so tests never open a browser.
pub trait Launcher: Send + Sync {
    fn launch(&self, path: &Path) -> Result<(), PreviewError>;
}

/// Hands the artefact to the platform's default handler.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLauncher;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    Mac,
    Windows,
    Other,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// The program that opens a file with its default handler, and the arguments before the
/// file. Never `cmd /C start` on Windows: `cmd.exe` reads `&`, `^` and `%NAME%` in the path
/// as its own syntax, so a page named `a&calc&b.adoc` would run `calc`. `rundll32` takes the
/// path as one argument and reads nothing in it.
fn launch_command(platform: Platform) -> (&'static str, &'static [&'static str]) {
    match platform {
        Platform::Mac => ("open", &[]),
        Platform::Windows => ("rundll32", &["url.dll,FileProtocolHandler"]),
        Platform::Other => ("xdg-open", &[]),
    }
}

impl Launcher for SystemLauncher {
    fn launch(&self, path: &Path) -> Result<(), PreviewError> {
        let (program, args) = launch_command(Platform::current());
        launch_with(program, args, path)
    }
}

/// Starts `program` with `args` and `path`, detached from the server: its standard streams
/// are `/dev/null`, since the server's stdin and stdout are the LSP connection, and it is not
/// waited for, since `xdg-open` may run the browser in the foreground until it closes. A
/// thread of its own reaps it. Only a failure to start is reported.
fn launch_with(program: &str, args: &[&str], path: &Path) -> Result<(), PreviewError> {
    let mut child = process::Command::new(program)
        .args(args)
        .arg(path)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()?;
    // Without the thread the child stays a zombie until the server ends, nothing worse.
    let _ = std::thread::Builder::new()
        .name("adoc-ls-launcher".to_owned())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

/// Where a [`BrowserSink`] writes.
#[derive(Debug)]
enum Folder {
    /// A folder given, as tests give one.
    Given(PathBuf),
    /// A folder of this session's own, made on first use in the temporary directory with an
    /// unpredictable name and, on Unix, mode `0700`, and removed when the server ends. Never a
    /// fixed, shared name: in `/tmp` another user could make it first and plant links in it,
    /// and read every page previewed.
    Private(OnceLock<Result<tempfile::TempDir, String>>),
}

/// Writes HTML to a directory and opens it outside the editor.
#[derive(Debug)]
pub struct BrowserSink<L: Launcher> {
    folder: Folder,
    launcher: L,
}

impl<L: Launcher> BrowserSink<L> {
    pub fn new(directory: impl Into<PathBuf>, launcher: L) -> Self {
        Self {
            folder: Folder::Given(directory.into()),
            launcher,
        }
    }

    /// A sink writing to a folder of this session's own (see [`Folder::Private`]).
    pub fn private(launcher: L) -> Self {
        Self {
            folder: Folder::Private(OnceLock::new()),
            launcher,
        }
    }

    /// The folder artefacts are written to, made if it is not there.
    fn directory(&self) -> Result<PathBuf, PreviewError> {
        match &self.folder {
            Folder::Given(directory) => {
                fs::create_dir_all(directory)?;
                Ok(directory.clone())
            }
            Folder::Private(made) => made
                .get_or_init(|| {
                    let mut builder = tempfile::Builder::new();
                    builder.prefix("adoc-ls-preview-");
                    // Made so from the start, never loosened then tightened.
                    #[cfg(unix)]
                    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
                    builder.tempdir().map_err(|error| error.to_string())
                })
                .as_ref()
                .map(|directory| directory.path().to_path_buf())
                .map_err(|error| PreviewError::Io(io::Error::other(error.clone()))),
        }
    }

    #[must_use]
    pub fn launcher(&self) -> &L {
        &self.launcher
    }

    /// Artefact name for `source`: its stem and a hash of its whole path, so pages with the
    /// same name in different folders (every module's `index.adoc`) keep their own. Stable
    /// across renders, so the browser tab can be reloaded. Of the stem, only ASCII letters,
    /// digits, `-` and `_` are kept, the rest written `_`, so no shell or program reads the
    /// name as anything but a name.
    fn artefact_name(source: &Path) -> String {
        use sha2::{Digest, Sha256};
        let stem: String = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("preview")
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let digest = Sha256::digest(source.as_os_str().as_encoded_bytes());
        let hash: String = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{stem}-{hash}.html")
    }
}

impl<L: Launcher> BrowserSink<L> {
    fn write(
        &self,
        output: &RenderOutput,
        source: &Path,
        mode: PreviewMode,
    ) -> Result<PathBuf, PreviewError> {
        let directory = self.directory()?;
        let artefact = directory.join(Self::artefact_name(source));
        let html = match mode {
            PreviewMode::Static => output.html.clone(),
            PreviewMode::Live => with_reloader(&output.html),
        };
        // Written beside and renamed over the name, which replaces whatever is there, a link
        // included, and never writes into what a link leads to.
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        io::Write::write_all(&mut file, html.as_bytes())?;
        file.persist(&artefact).map_err(|error| error.error)?;
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

    fn scratch_directory(&self) -> Result<PathBuf, PreviewError> {
        let includes = match &self.folder {
            Folder::Given(directory) => directory.with_file_name(format!(
                "{}-includes",
                directory
                    .file_name()
                    .map_or("adoc-ls-preview".into(), |name| name.to_string_lossy())
            )),
            Folder::Private(_) => self.directory()?.join("includes"),
        };
        fs::create_dir_all(&includes)?;
        Ok(includes)
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

    use crate::spawn_guard;

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

        let name = artefact.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("guide-") && name.ends_with(".html"),
            "{name}"
        );
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
    fn pages_with_the_same_name_in_different_folders_get_their_own_artefacts() {
        let dir = tempdir("same-name");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let admin = sink
            .deliver(
                &output("<p>admin</p>"),
                Path::new("/docs/modules/admin/pages/index.adoc"),
                PreviewMode::Static,
            )
            .expect("admin");
        let root = sink
            .deliver(
                &output("<p>root</p>"),
                Path::new("/docs/modules/ROOT/pages/index.adoc"),
                PreviewMode::Static,
            )
            .expect("root");

        assert_ne!(admin, root);
        assert_eq!(fs::read_to_string(&admin).unwrap(), "<p>admin</p>");
        assert_eq!(fs::read_to_string(&root).unwrap(), "<p>root</p>");
    }

    #[test]
    fn an_artefact_has_the_same_name_in_every_run() {
        let dir = tempdir("same-every-run");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<p>x</p>"),
                Path::new("/docs/guide.adoc"),
                PreviewMode::Static,
            )
            .expect("deliver");

        // The first 16 hex digits of the SHA-256 of `/docs/guide.adoc`.
        assert_eq!(artefact.file_name().unwrap(), "guide-ce009aab83da2c06.html");
    }

    #[test]
    fn a_source_without_a_stem_still_produces_an_artefact() {
        let dir = tempdir("no-stem");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(&output("<p>x</p>"), Path::new("/"), PreviewMode::Static)
            .expect("deliver");

        let name = artefact.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("preview-") && name.ends_with(".html"),
            "{name}"
        );
    }

    #[test]
    fn an_artefact_name_holds_nothing_a_shell_would_read() {
        let dir = tempdir("shell-safe-name");
        let sink = BrowserSink::new(dir, RecordingLauncher::default());

        let artefact = sink
            .deliver(
                &output("<p>x</p>"),
                Path::new("/docs/a&calc&b %PATH%^x,y;z=w é.adoc"),
                PreviewMode::Static,
            )
            .expect("deliver");

        let name = artefact.file_name().unwrap().to_str().unwrap();
        assert!(
            name.chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.')),
            "{name}"
        );
        assert!(name.starts_with("a_calc_b_"), "{name}");
    }

    #[test]
    fn windows_opens_the_artefact_without_cmd() {
        let (program, arguments) = launch_command(Platform::Windows);
        assert_ne!(
            program, "cmd",
            "cmd.exe reads & ^ % in a path as its own syntax"
        );
        assert_eq!(
            (program, arguments),
            ("rundll32", &["url.dll,FileProtocolHandler"][..])
        );
    }

    /// A launcher that prints, notes which of its standard streams are `/dev/null`, and stays
    /// open as a browser started in the foreground does, returns at once and keeps its
    /// output out of the server's stdout, which is the LSP connection.
    #[cfg(unix)]
    #[test]
    fn a_launcher_runs_detached_from_the_server_s_streams() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("streams");
        let script = directory.path().join("launcher");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 for fd in 0 1 2; do\n\
                 if [ /dev/fd/$fd -ef /dev/null ]; then state=null; else state=inherited; fi\n\
                 echo \"$fd $state\" >> '{marker}.tmp'\n\
                 done\n\
                 mv '{marker}.tmp' '{marker}'\n\
                 echo 'Opening in existing browser session.'\n\
                 sleep 10\n",
                marker = marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        launch_with(script.to_str().unwrap(), &[], Path::new("/docs/page.html")).expect("launch");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the server waited for the launcher: {:?}",
            started.elapsed()
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(&marker).expect("the launcher ran"),
            "0 null\n1 null\n2 null\n"
        );
    }

    #[test]
    fn a_launcher_that_cannot_start_is_reported() {
        let error = launch_with("/no/such/launcher", &[], Path::new("/docs/page.html"))
            .expect_err("nothing to start");
        assert!(matches!(error, PreviewError::Io(_)), "{error:?}");
    }

    #[test]
    fn a_private_folder_is_the_session_s_own() {
        let first = BrowserSink::private(RecordingLauncher::default());
        let second = BrowserSink::private(RecordingLauncher::default());
        let source = Path::new("/docs/guide.adoc");

        let artefact = first
            .deliver(&output("<p>x</p>"), source, PreviewMode::Static)
            .expect("deliver");
        let other = second
            .deliver(&output("<p>y</p>"), source, PreviewMode::Static)
            .expect("deliver");

        let folder = artefact.parent().unwrap().to_path_buf();
        assert_ne!(folder, other.parent().unwrap(), "no folder is shared");
        assert_ne!(folder, std::env::temp_dir().join("adoc-ls-preview"));
        // The same name within the session, so the browser's tab reloads it.
        let again = first.refresh(&output("<p>z</p>"), source).expect("refresh");
        assert_eq!(artefact, again);
        let includes = first.scratch_directory().expect("includes");
        assert!(includes.starts_with(&folder), "{includes:?}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&folder).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only its user may enter: {mode:o}");
        }

        drop(first);
        assert!(!folder.exists(), "the folder goes with the session");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_an_artefact_s_name_is_replaced_not_written_through() {
        let dir = tempdir("link-at-artefact");
        let victim = dir.join("victim.txt");
        fs::write(&victim, "keep").unwrap();
        let sink = BrowserSink::new(dir.join("previews"), RecordingLauncher::default());
        let source = Path::new("/docs/guide.adoc");
        let artefact = sink
            .deliver(&output("<p>one</p>"), source, PreviewMode::Static)
            .expect("deliver");
        fs::remove_file(&artefact).unwrap();
        std::os::unix::fs::symlink(&victim, &artefact).unwrap();

        sink.refresh(&output("<p>two</p>"), source)
            .expect("refresh");

        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
        assert!(!fs::symlink_metadata(&artefact)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::read_to_string(&artefact)
            .unwrap()
            .contains("<p>two</p>"));
    }

    /// A directory unique to one test: these run in parallel and share a filename.
    fn tempdir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("adoc-ls-{}-{test}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
