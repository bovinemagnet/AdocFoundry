use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{RenderError, RenderOutput, RenderRequest};

pub trait Renderer: Send + Sync {
    fn render(&self, request: &RenderRequest) -> Result<RenderOutput, RenderError>;
}

/// How long Asciidoctor may take to render a page before it is stopped.
pub const ASCIIDOCTOR_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct SystemAsciidoctor {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for SystemAsciidoctor {
    fn default() -> Self {
        Self::new("asciidoctor")
    }
}

impl SystemAsciidoctor {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            timeout: ASCIIDOCTOR_TIMEOUT,
        }
    }

    /// Stops a render that takes longer than `timeout`, rather than [`ASCIIDOCTOR_TIMEOUT`].
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Everything `pipe` gives until it closes, read on a thread of its own.
fn read_all(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

impl Renderer for SystemAsciidoctor {
    fn render(&self, request: &RenderRequest) -> Result<RenderOutput, RenderError> {
        let mut command = Command::new(&self.executable);
        command.args([
            "--backend=html5",
            "--out-file=-",
            &format!("--safe-mode={}", request.safe_mode.as_cli_value()),
        ]);
        for (name, value) in &request.attributes {
            command.args(["--attribute", &format!("{name}={value}")]);
        }
        if let Some(base_dir) = &request.base_dir {
            command.arg(format!("--base-dir={}", base_dir.display()));
        }
        if let Some(stylesheet) = &request.stylesheet {
            command.args([
                "--attribute",
                &format!("stylesheet={}", stylesheet.display()),
            ]);
        }

        let source_directory = request
            .source_file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty() && parent.is_dir());
        if let Some(parent) = source_directory {
            command.current_dir(parent);
        }
        if request.source_text.is_some() {
            command.arg("-");
        } else if let Some(file_name) =
            source_directory.and_then(|_| request.source_file.file_name())
        {
            command.arg(not_an_option(Path::new(file_name)));
        } else {
            command.arg(not_an_option(&request.source_file));
        }
        command
            .stdin(if request.source_text.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                RenderError::ExecutableNotFound(self.executable.display().to_string())
            } else {
                RenderError::Io(error)
            }
        })?;

        // Written and read on threads of their own, so a renderer that stops reading or writing
        // holds up this thread no longer than the time limit.
        let writer = request
            .source_text
            .clone()
            .zip(child.stdin.take())
            .map(|(source, mut stdin)| thread::spawn(move || stdin.write_all(source.as_bytes())));
        let stdout = read_all(child.stdout.take());
        let stderr = read_all(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    // Reaped all the same.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RenderError::Io(error));
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RenderError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Asciidoctor did not finish within {} seconds",
                        self.timeout.as_secs_f32()
                    ),
                )));
            }
            thread::sleep(Duration::from_millis(10));
        };
        // A renderer that exits before reading the page breaks the pipe: its own failure is the
        // one reported, and a failed write only when it succeeded.
        let written = writer.map_or(Ok(()), |writer| {
            writer
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("writing the source failed")))
        });
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        if !status.success() {
            return Err(RenderError::ProcessFailed {
                status: status.code(),
                stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        written.map_err(RenderError::Io)?;

        Ok(RenderOutput {
            html: String::from_utf8_lossy(&stdout).into_owned(),
            warnings: stderr_lines(&stderr),
        })
    }
}

/// `path`, as `./path` when it would be read as an option (`-n.adoc`). Asciidoctor does not
/// take `--` to end its options.
fn not_an_option(path: &Path) -> PathBuf {
    if path.as_os_str().to_string_lossy().starts_with('-') {
        Path::new(".").join(path)
    } else {
        path.to_path_buf()
    }
}

fn stderr_lines(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug)]
pub struct MockRenderer {
    html: String,
}

impl MockRenderer {
    #[must_use]
    pub fn new(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }
}

impl Renderer for MockRenderer {
    fn render(&self, _request: &RenderRequest) -> Result<RenderOutput, RenderError> {
        Ok(RenderOutput {
            html: self.html.clone(),
            warnings: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        sync::{Mutex, MutexGuard},
    };

    use super::{MockRenderer, Renderer, SystemAsciidoctor};
    #[cfg(unix)]
    use crate::RenderSafeMode;
    use crate::{RenderError, RenderRequest};

    /// Serializes every test that spawns a process.
    ///
    /// `invokes_a_renderer_directly_with_structured_arguments` writes an executable and then
    /// runs it. A concurrent `fork` in this process inherits the still-open write descriptor,
    /// and `execve` reports `ETXTBSY` while any process holds the file open for writing, so an
    /// overlapping spawn from another test made that test fail intermittently. Holding this lock
    /// across both the write and the spawn removes the overlap.
    static SPAWN: Mutex<()> = Mutex::new(());

    fn spawn_guard() -> MutexGuard<'static, ()> {
        SPAWN.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn mock_renderer_is_deterministic() {
        let renderer = MockRenderer::new("<h1>Guide</h1>");
        let output = renderer
            .render(&RenderRequest::from_source("guide.adoc", "= Guide"))
            .unwrap();

        assert_eq!(output.html, "<h1>Guide</h1>");
        assert!(output.warnings.is_empty());
    }

    /// A renderer that never finishes is stopped at the time limit, so whoever waits for it
    /// (the language server's one thread) goes on.
    #[cfg(unix)]
    #[test]
    fn a_renderer_that_hangs_is_stopped() {
        use std::{fs, os::unix::fs::PermissionsExt, time::Duration, time::Instant};

        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("hanging-asciidoctor");
        fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();

        let error = SystemAsciidoctor::new(&executable)
            .with_timeout(Duration::from_millis(300))
            .render(&RenderRequest::from_source("guide.adoc", "= Guide"))
            .unwrap_err();

        assert!(
            matches!(&error, RenderError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut),
            "{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_executable_is_structured() {
        let _guard = spawn_guard();
        let renderer = SystemAsciidoctor::new("adoc-render-executable-that-does-not-exist");
        let error = renderer
            .render(&RenderRequest::from_source("guide.adoc", "= Guide"))
            .unwrap_err();

        assert!(matches!(error, RenderError::ExecutableNotFound(_)));
    }

    #[cfg(unix)]
    #[test]
    fn invokes_a_renderer_directly_with_structured_arguments() {
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            time::{SystemTime, UNIX_EPOCH},
        };

        let _guard = spawn_guard();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("adoc-render-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-asciidoctor");
        fs::write(
            &executable,
            "#!/bin/sh\nset -eu\n[ \"$1\" = \"--backend=html5\" ]\n[ \"$2\" = \"--out-file=-\" ]\ncase \"$#\" in\n  8)\n    [ \"$3\" = \"--safe-mode=server\" ]\n    [ \"$4\" = \"--attribute\" ]\n    [ \"$5\" = \"sectnums=\" ]\n    [ \"$6\" = \"--attribute\" ]\n    case \"$7\" in stylesheet=*) ;; *) exit 1 ;; esac\n    [ \"$8\" = \"-\" ]\n    input=-\n    ;;\n  4)\n    [ \"$3\" = \"--safe-mode=safe\" ]\n    [ \"$4\" = \"guide.adoc\" ]\n    input=guide.adoc\n    ;;\n  *) exit 1 ;;\nesac\nprintf '<article>'\nif [ \"$input\" = - ]; then cat; else cat \"$input\"; fi\nprintf '</article>'\nprintf 'fixture warning\\n' >&2\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let mut request = RenderRequest::from_source(root.join("guide.adoc"), "= Guide");
        request.safe_mode = RenderSafeMode::Server;
        request
            .attributes
            .insert("sectnums".to_owned(), String::new());
        request.stylesheet = Some(root.join("theme.css"));
        let output = SystemAsciidoctor::new(&executable).render(&request);

        fs::write(root.join("guide.adoc"), "= On Disk").unwrap();
        let file_output = SystemAsciidoctor::new(&executable)
            .render(&RenderRequest::from_file(root.join("guide.adoc")));
        fs::remove_dir_all(&root).unwrap();
        let output = output.unwrap();
        let file_output = file_output.unwrap();

        assert_eq!(output.html, "<article>= Guide</article>");
        assert_eq!(output.warnings, ["fixture warning"]);
        assert_eq!(file_output.html, "<article>= On Disk</article>");
    }

    /// A safe-mode jail refuses includes outside its base directory, so an Antora page
    /// pulling in a sibling module's partial renders only when `--base-dir` widens it.
    #[cfg(unix)]
    #[test]
    fn passes_base_dir_only_when_the_request_sets_one() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let _guard = spawn_guard();
        let root =
            std::env::temp_dir().join(format!("adoc-render-base-dir-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("echo-args");
        fs::write(
            &executable,
            // It reads the page first, as Asciidoctor does: exiting with the page unread would
            // break the renderer's pipe.
            "#!/bin/sh\ncat > /dev/null\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        let renderer = SystemAsciidoctor::new(&executable);

        let without = renderer
            .render(&RenderRequest::from_source(
                root.join("guide.adoc"),
                "= Guide",
            ))
            .unwrap();

        let mut request = RenderRequest::from_source(root.join("guide.adoc"), "= Guide");
        request.base_dir = Some(root.join("component"));
        let with = renderer.render(&request).unwrap();

        fs::remove_dir_all(&root).unwrap();

        assert!(!without.html.contains("--base-dir"), "{}", without.html);
        assert!(
            with.html
                .contains(&format!("--base-dir={}", root.join("component").display())),
            "{}",
            with.html
        );
    }

    /// An executable at `path` running `script`, written under the spawn guard the caller holds.
    #[cfg(unix)]
    fn fake_renderer(path: &std::path::Path, script: &str) {
        use std::{fs, os::unix::fs::PermissionsExt};

        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A renderer that fails before reading the page is reported by its status and message,
    /// whatever the size of the page, not as the pipe it left broken.
    #[cfg(unix)]
    #[test]
    fn a_renderer_that_fails_without_reading_the_page_reports_its_error() {
        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("failing-asciidoctor");
        fake_renderer(
            &executable,
            "#!/bin/sh\nprintf 'cannot load such file -- asciidoctor\\n' >&2\nexit 1\n",
        );
        let page = "word ".repeat(40_000);

        let error = SystemAsciidoctor::new(&executable)
            .render(&RenderRequest::from_source(
                directory.path().join("guide.adoc"),
                page,
            ))
            .unwrap_err();

        match error {
            RenderError::ProcessFailed { status, stderr } => {
                assert_eq!(status, Some(1));
                assert_eq!(stderr, "cannot load such file -- asciidoctor");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A page whose name starts with `-` is read as a file, not as an option.
    #[cfg(unix)]
    #[test]
    fn a_page_named_like_an_option_is_rendered() {
        let _guard = spawn_guard();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("echo-last");
        fake_renderer(
            &executable,
            "#!/bin/sh\nfor a in \"$@\"; do last=$a; done\ncat \"$last\"\n",
        );
        let page = directory.path().join("-n.adoc");
        std::fs::write(&page, "= Dash").unwrap();

        let output = SystemAsciidoctor::new(&executable)
            .render(&RenderRequest::from_file(&page))
            .unwrap();

        assert_eq!(output.html, "= Dash");
    }

    #[test]
    fn renders_with_system_asciidoctor_when_available() {
        let _guard = spawn_guard();
        let available = Command::new("asciidoctor")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !available {
            return;
        }

        let source_file = std::env::current_dir().unwrap().join("guide.adoc");
        let output = SystemAsciidoctor::default()
            .render(&RenderRequest::from_source(source_file, "= Guide"))
            .unwrap();

        assert!(output.html.contains("<h1>Guide</h1>"));
    }
}
