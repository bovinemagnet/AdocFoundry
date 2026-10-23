use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{RenderError, RenderOutput, RenderRequest};

pub trait Renderer: Send + Sync {
    fn render(&self, request: &RenderRequest) -> Result<RenderOutput, RenderError>;
}

#[derive(Clone, Debug)]
pub struct SystemAsciidoctor {
    executable: PathBuf,
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
        }
    }
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
            command.arg(file_name);
        } else {
            command.arg(&request.source_file);
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

        if let Some(source) = &request.source_text {
            child
                .stdin
                .take()
                .expect("piped stdin must be present for source overlays")
                .write_all(source.as_bytes())
                .map_err(RenderError::Io)?;
        }
        let output = child.wait_with_output().map_err(RenderError::Io)?;
        if !output.status.success() {
            return Err(RenderError::ProcessFailed {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        Ok(RenderOutput {
            html: String::from_utf8_lossy(&output.stdout).into_owned(),
            warnings: stderr_lines(&output.stderr),
        })
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
    use crate::{RenderError, RenderRequest, RenderSafeMode};

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
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
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
