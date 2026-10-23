use std::{
    env,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{DiagramError, DiagramProvider, DiagramRequest};

/// A diagram tool run as a process: the source on standard input, SVG on standard output.
/// Optional: when the program is not found, rendering fails with [`DiagramError::ToolMissing`].
#[derive(Clone, Debug)]
pub struct ExternalProvider {
    kinds: Vec<&'static str>,
    program: PathBuf,
    args: Vec<String>,
}

impl ExternalProvider {
    #[must_use]
    pub fn new(kinds: Vec<&'static str>, program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            kinds,
            program: program.into(),
            args,
        }
    }

    /// Graphviz (`dot`), PlantUML and D2, looked up on the PATH when used.
    #[must_use]
    pub fn defaults() -> Vec<Self> {
        let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect();
        vec![
            Self::new(vec!["graphviz", "dot"], "dot", args(&["-Tsvg"])),
            Self::new(
                vec!["plantuml", "uml"],
                "plantuml",
                args(&["-tsvg", "-pipe"]),
            ),
            Self::new(vec!["d2"], "d2", args(&["-", "-"])),
        ]
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }
}

impl DiagramProvider for ExternalProvider {
    fn kinds(&self) -> &[&str] {
        &self.kinds
    }

    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
        let missing = || DiagramError::ToolMissing {
            kind: request.kind.clone(),
            program: self.program.display().to_string(),
        };
        let program = find_program(&self.program).ok_or_else(missing)?;
        let mut child = Command::new(program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            // A tool that exits early closes the pipe; its status reports why.
            let _ = stdin.write_all(request.source.as_bytes());
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(DiagramError::Failed(if message.is_empty() {
                format!("{} exited with {}", self.program.display(), output.status)
            } else {
                message
            }));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| DiagramError::Failed("the tool's output is not UTF-8".to_owned()))
    }
}

/// `program` itself when it is a path to a file, otherwise the first match on the PATH.
#[must_use]
pub fn find_program(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return program.is_file().then(|| program.to_path_buf());
    }
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    env::split_paths(&env::var_os("PATH")?).find_map(|dir| {
        extensions.iter().find_map(|extension| {
            let mut name = program.as_os_str().to_owned();
            name.push(extension);
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}
