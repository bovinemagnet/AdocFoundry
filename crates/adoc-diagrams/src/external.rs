use std::{
    env,
    io::{Read, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
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

/// How long a program not found stays not found before it is looked for again, so a tool
/// installed while the app runs is found without looking on every diagram of every preview.
pub(crate) const MISS_RECHECK: Duration = Duration::from_secs(1);

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
    /// Which program `program` finds, part of every cache key: kept once found (until
    /// [`ExternalProvider::look_again`]), and for [`MISS_RECHECK`] when not found.
    fingerprint: Arc<Mutex<Option<Fingerprint>>>,
}

#[derive(Clone, Debug)]
struct Fingerprint {
    text: String,
    /// When the program was looked for and not found.
    missed: Option<Instant>,
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
            fingerprint: Arc::default(),
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

    /// Forgets which program `program` finds, so the next diagram looks for it again: one
    /// installed or upgraded since is used, and renders afresh.
    pub fn look_again(&self) {
        *self
            .fingerprint
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    /// The program `program` finds, by where it is, its size and when it last changed, so
    /// another program, or an upgrade of it, renders afresh. Worked out the first time it is
    /// needed and kept; a program not found is looked for again after [`MISS_RECHECK`].
    fn fingerprint(&self) -> String {
        let mut kept = self
            .fingerprint
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let fresh = kept.as_ref().is_some_and(|fingerprint| {
            fingerprint
                .missed
                .is_none_or(|missed| missed.elapsed() < MISS_RECHECK)
        });
        if !fresh {
            *kept = Some(match find_program(&self.program) {
                Some(found) => Fingerprint {
                    text: format!("{} {}", found.display(), file_stamp(&found)),
                    missed: None,
                },
                None => Fingerprint {
                    text: format!("{} (not found)", self.program.display()),
                    missed: Some(Instant::now()),
                },
            });
        }
        kept.as_ref()
            .map(|fingerprint| fingerprint.text.clone())
            .unwrap_or_default()
    }
}

/// A file's size and modification time, which change when it does: `0 0` when it cannot be
/// read.
pub(crate) fn file_stamp(path: &Path) -> String {
    let metadata = std::fs::metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let size = metadata.map_or(0, |metadata| metadata.len());
    format!("{size} {modified}")
}

impl DiagramProvider for ExternalProvider {
    fn kinds(&self) -> &[&str] {
        &self.kinds
    }

    fn look_again(&self) {
        ExternalProvider::look_again(self);
    }

    /// The program, the page's folder, and the size and modification time of each file the
    /// diagram names for the tool to read there, so editing an included file draws it again.
    fn identity(&self, request: &DiagramRequest) -> String {
        let folder = request
            .directory
            .as_deref()
            .map(|folder| folder.display().to_string())
            .unwrap_or_default();
        let mut identity = format!("{}\n{folder}", self.fingerprint());
        for input in self.inputs(request) {
            identity.push_str(&format!("\n{} {}", input.display(), file_stamp(&input)));
        }
        identity
    }

    fn inputs(&self, request: &DiagramRequest) -> Vec<PathBuf> {
        request
            .directory
            .as_deref()
            .map(|folder| crate::reads::files_read(&request.kind, &request.source, folder))
            .unwrap_or_default()
    }

    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
        let missing = || DiagramError::ToolMissing {
            kind: request.kind.clone(),
            program: self.program.display().to_string(),
        };
        let program = find_program(&self.program).ok_or_else(missing)?;
        let mut command = Command::new(program);
        // In the page's folder, so the diagram's own relative includes resolve against it.
        if let Some(folder) = request
            .directory
            .as_deref()
            .filter(|folder| folder.is_dir())
        {
            command.current_dir(folder);
        }
        // A group of its own, so stopping it stops what it starts too: the `plantuml`
        // launcher's Java, a tool's own helpers.
        crate::process::in_own_group(&mut command);
        let mut child = command
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
                // tool left a process outside its group holding them open.
                crate::process::stop(&mut child);
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
