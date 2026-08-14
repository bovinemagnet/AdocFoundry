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
        command.args(["--backend=html5", "--embedded", "--out-file=-"]);
        for (name, value) in &request.attributes {
            command.args(["--attribute", &format!("{name}={value}")]);
        }
        if let Some(parent) = request
            .source_path
            .as_deref()
            .and_then(|path| path.parent())
        {
            command.current_dir(parent);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                RenderError::ExecutableNotFound(self.executable.display().to_string())
            } else {
                RenderError::Io(error)
            }
        })?;

        child
            .stdin
            .take()
            .expect("piped stdin must be present")
            .write_all(request.source.as_bytes())
            .map_err(RenderError::Io)?;
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
    use super::{MockRenderer, Renderer, SystemAsciidoctor};
    use crate::{RenderError, RenderRequest};

    #[test]
    fn mock_renderer_is_deterministic() {
        let renderer = MockRenderer::new("<h1>Guide</h1>");
        let output = renderer.render(&RenderRequest::new("= Guide")).unwrap();

        assert_eq!(output.html, "<h1>Guide</h1>");
        assert!(output.warnings.is_empty());
    }

    #[test]
    fn missing_executable_is_structured() {
        let renderer = SystemAsciidoctor::new("adoc-render-executable-that-does-not-exist");
        let error = renderer.render(&RenderRequest::new("= Guide")).unwrap_err();

        assert!(matches!(error, RenderError::ExecutableNotFound(_)));
    }
}
