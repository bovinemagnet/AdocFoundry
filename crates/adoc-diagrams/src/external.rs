use std::{
    env,
    io::{Read, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{DiagramError, DiagramProvider, DiagramRequest};

/// How long a tool may run before it is stopped, unless told otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The most a tool may write to standard output or standard error before it is stopped.
pub const MAX_OUTPUT: usize = 32 * 1024 * 1024;

/// How often a running tool is checked for having finished.
const POLL: Duration = Duration::from_millis(5);

/// The programs the external providers run, and how long each may run: a bare name is looked
/// up on the PATH, anything else is used as a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolPaths {
    pub dot: PathBuf,
    pub plantuml: PathBuf,
    pub d2: PathBuf,
    pub timeout: Duration,
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self {
            dot: PathBuf::from("dot"),
            plantuml: PathBuf::from("plantuml"),
            d2: PathBuf::from("d2"),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// A diagram tool run as a process: the source on standard input, SVG on standard output.
/// Optional: when the program is not found, rendering fails with [`DiagramError::ToolMissing`].
#[derive(Clone, Debug)]
pub struct ExternalProvider {
    kinds: Vec<&'static str>,
    program: PathBuf,
    args: Vec<String>,
    timeout: Duration,
    max_output: usize,
}

impl ExternalProvider {
    #[must_use]
    pub fn new(kinds: Vec<&'static str>, program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            kinds,
            program: program.into(),
            args,
            timeout: DEFAULT_TIMEOUT,
            max_output: MAX_OUTPUT,
        }
    }

    /// Stops the tool once it has run for `timeout`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Stops the tool once it writes more than `bytes` to either output.
    #[must_use]
    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }

    /// Graphviz (`dot`), PlantUML and D2, looked up on the PATH when used.
    #[must_use]
    pub fn defaults() -> Vec<Self> {
        Self::for_tools(&ToolPaths::default())
    }

    /// Graphviz, PlantUML and D2 run from `tools`.
    #[must_use]
    pub fn for_tools(tools: &ToolPaths) -> Vec<Self> {
        let args = |args: &[&str]| args.iter().map(|arg| (*arg).to_owned()).collect();
        vec![
            Self::new(vec!["graphviz", "dot"], &tools.dot, args(&["-Tsvg"])),
            Self::new(
                vec!["plantuml", "uml"],
                &tools.plantuml,
                args(&["-tsvg", "-pipe"]),
            ),
            Self::new(vec!["d2"], &tools.d2, args(&["-", "-"])),
        ]
        .into_iter()
        .map(|provider| provider.with_timeout(tools.timeout))
        .collect()
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
        // Writing and reading on threads of their own, so a tool that fills one pipe before
        // reading the other cannot stall, and the wait below can keep the deadline.
        if let Some(mut stdin) = child.stdin.take() {
            let source = request.source.clone();
            // A tool that exits early closes the pipe; its status reports why.
            thread::spawn(move || stdin.write_all(source.as_bytes()));
        }
        let overflow = Arc::new(AtomicBool::new(false));
        let read = |pipe: Option<Box<dyn Read + Send>>| {
            let overflow = overflow.clone();
            let limit = self.max_output;
            thread::spawn(move || read_capped(pipe, limit, &overflow))
        };
        let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
        let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));

        let too_much = || {
            DiagramError::Failed(format!(
                "{} wrote more than {} bytes and was stopped",
                self.program.display(),
                self.max_output
            ))
        };
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            let stop = if overflow.load(Ordering::Relaxed) {
                Some(too_much())
            } else if Instant::now() >= deadline {
                Some(DiagramError::TimedOut {
                    program: self.program.display().to_string(),
                    after: self.timeout,
                })
            } else {
                None
            };
            if let Some(error) = stop {
                // The readers end once the pipes close; they are not waited for, in case the
                // tool left a process of its own holding them open.
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            thread::sleep(POLL);
        };
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        if overflow.load(Ordering::Relaxed) {
            return Err(too_much());
        }
        if !status.success() {
            let message = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(DiagramError::Failed(if message.is_empty() {
                format!("{} exited with {status}", self.program.display())
            } else {
                message
            }));
        }
        String::from_utf8(stdout)
            .map_err(|_| DiagramError::Failed("the tool's output is not UTF-8".to_owned()))
    }
}

/// Reads `pipe` to its end, or until it holds more than `limit` bytes, when `overflow` is set.
fn read_capped(pipe: Option<Box<dyn Read + Send>>, limit: usize, overflow: &AtomicBool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe
            .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut bytes);
    }
    if bytes.len() > limit {
        overflow.store(true, Ordering::Relaxed);
        bytes.truncate(limit);
    }
    bytes
}

/// True when `program` names a file by its path rather than a program to look up on the PATH.
#[must_use]
pub fn is_path(program: &Path) -> bool {
    program.components().count() > 1
}

/// `program` itself when it is a path to a file, otherwise the first match on the PATH.
#[must_use]
pub fn find_program(program: &Path) -> Option<PathBuf> {
    if is_path(program) {
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
