//! Build Site (#293): `project.build_site` runs the site's build with npm, in the folder of the
//! playbook the catalog found, on a worker. With a `package.json`, its dependencies are
//! installed first while `node_modules` is missing, then its `build` script runs, else
//! Antora from `node_modules` or the `PATH` (`npm exec --no`, which installs nothing). The
//! whole build is bounded by `[tools] build_timeout` and its output by [`MAX_BUILD_OUTPUT`],
//! past either of which it is stopped with every process it started; its output arrives line
//! by line. This is the one place the engine runs Node.js, only when asked, and nothing needs
//! it: without npm the command is unavailable and [`Engine::site_build_blocker`] says why.
//! `npm install` and a `build` script run the project's own code, so a front end shows
//! [`Engine::site_build_plan`] and asks before a project's first build.

use std::{
    env,
    ffi::OsString,
    io::{BufRead as _, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use adoc_diagrams::find_program;

use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
    EngineError, EngineEvent,
};

/// How long a build may run, `npm install` included, unless `[tools] build_timeout` says.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// The most a build may write, both outputs together, before it is stopped.
pub const MAX_BUILD_OUTPUT: usize = 8 * 1024 * 1024;

/// How often a running step is checked for having finished.
const POLL: Duration = Duration::from_millis(20);

/// The last site build: the playbook it built, its output so far, and how it stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteBuild {
    pub playbook: PathBuf,
    /// Each line the build wrote to either output, and before each step the command it runs
    /// (`$ npm run build`).
    pub log: Vec<String>,
    pub state: SiteBuildState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SiteBuildState {
    Running,
    /// Every step succeeded; `site` is `build/site/index.html` beside the playbook, when the
    /// build wrote it.
    Built {
        site: Option<PathBuf>,
    },
    /// A step failed, could not start, or was stopped; the message says which and why.
    Failed(String),
}

/// Why Build Site cannot run now; its text says so in a sentence's middle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SiteBuildBlocker {
    Running,
    NoPlaybook,
    /// No npm was found: on `PATH`, or beside the `[tools] node` named here.
    NoNode {
        configured: Option<String>,
    },
}

impl std::fmt::Display for SiteBuildBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => f.write_str("a site build is already running"),
            Self::NoPlaybook => write!(
                f,
                "building the site needs an {} in a workspace folder or beside a component's \
                 antora.yml",
                adoc_antora::PLAYBOOK_FILE
            ),
            Self::NoNode { configured: None } => f.write_str(
                "building the site needs Node.js and npm: install Node.js, or set [tools] node \
                 to its node executable",
            ),
            Self::NoNode {
                configured: Some(node),
            } => write!(
                f,
                "[tools] node is {node}, but Node.js and npm are not there"
            ),
        }
    }
}

/// One command of a build.
struct Step {
    program: PathBuf,
    args: Vec<String>,
}

impl Step {
    fn shown(&self) -> String {
        format!("$ npm {}", self.args.join(" "))
    }
}

pub(crate) fn register_site_build(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::PROJECT_BUILD_SITE,
            "Build Site",
            CommandCategory::Workspace,
            None,
            CommandContext::SiteBuild,
        ),
        build_site,
    );
}

impl Engine {
    /// The last site build since the engine started, running or finished.
    #[must_use]
    pub fn site_build(&self) -> Option<&SiteBuild> {
        self.site_build.as_ref()
    }

    /// The `npm` a site build runs: the one beside `[tools] node` when set, else the first on
    /// `PATH`.
    #[must_use]
    pub fn npm_executable(&self) -> Option<PathBuf> {
        let node = Path::new(&self.settings.tools.node);
        if node.as_os_str().is_empty() {
            return find_program(Path::new("npm"));
        }
        if !node.is_file() {
            return None;
        }
        let names: &[&str] = if cfg!(windows) {
            &["npm.cmd", "npm.exe"]
        } else {
            &["npm"]
        };
        let folder = node.parent()?;
        names
            .iter()
            .map(|name| folder.join(name))
            .find(|candidate| candidate.is_file())
    }

    /// Why Build Site cannot run now, or `None` when it can: a build is running, the workspace
    /// has no playbook, or there is no Node.js to build with.
    #[must_use]
    pub fn site_build_blocker(&self) -> Option<SiteBuildBlocker> {
        if self.site_build_running() {
            return Some(SiteBuildBlocker::Running);
        }
        if self.workspace.antora.playbook().is_none() {
            return Some(SiteBuildBlocker::NoPlaybook);
        }
        if self.npm_executable().is_none() {
            let node = &self.settings.tools.node;
            return Some(SiteBuildBlocker::NoNode {
                configured: (!node.is_empty()).then(|| node.clone()),
            });
        }
        None
    }

    /// What Build Site would run now, and where: the playbook's folder and each command, so a
    /// front end can ask before it runs a project's scripts. `None` while it cannot run.
    #[must_use]
    pub fn site_build_plan(&self) -> Option<(PathBuf, Vec<String>)> {
        if self.site_build_blocker().is_some() {
            return None;
        }
        let playbook = &self.workspace.antora.playbook()?.path;
        let folder = playbook.parent()?;
        let commands = steps(&self.npm_executable()?, playbook, folder)
            .iter()
            .map(|step| step.shown().trim_start_matches("$ ").to_owned())
            .collect();
        Some((folder.to_path_buf(), commands))
    }

    fn site_build_running(&self) -> bool {
        self.site_build
            .as_ref()
            .is_some_and(|build| build.state == SiteBuildState::Running)
    }

    pub(crate) fn site_build_line(&mut self, generation: u64, line: String) {
        if generation != self.site_build_generation {
            return;
        }
        if let Some(build) = &mut self.site_build {
            build.log.push(line);
            self.emit(EngineEvent::SiteBuildOutput);
        }
    }

    pub(crate) fn site_built(&mut self, generation: u64, result: Result<Option<PathBuf>, String>) {
        if generation != self.site_build_generation {
            return;
        }
        let Some(build) = &mut self.site_build else {
            return;
        };
        build.state = match &result {
            Ok(site) => SiteBuildState::Built { site: site.clone() },
            Err(error) => SiteBuildState::Failed(error.clone()),
        };
        self.emit(EngineEvent::SiteBuildFinished { result });
    }
}

/// Starts a build of the workspace's site on a worker, refused while one runs and unavailable
/// without a playbook or npm.
fn build_site(engine: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
    if let Some(blocker) = engine.site_build_blocker() {
        return Err(if blocker == SiteBuildBlocker::Running {
            EngineError::Refused(blocker.to_string())
        } else {
            EngineError::NotFound(blocker.to_string())
        });
    }
    let (Some(playbook), Some(npm)) = (
        engine
            .workspace
            .antora
            .playbook()
            .map(|playbook| playbook.path.clone()),
        engine.npm_executable(),
    ) else {
        return Err(EngineError::NotFound(
            "there is no playbook or npm to build with".to_owned(),
        ));
    };
    let folder = playbook.parent().map(Path::to_path_buf).unwrap_or_default();
    let timeout = Duration::from_secs(engine.settings.tools.build_timeout);
    engine.site_build_generation += 1;
    let generation = engine.site_build_generation;
    engine.site_build = Some(SiteBuild {
        playbook: playbook.clone(),
        log: Vec::new(),
        state: SiteBuildState::Running,
    });
    engine.emit(EngineEvent::SiteBuildOutput);
    let sender = engine.worker_sender.clone();
    engine.workers_running += 1;
    let crashed = move |error| WorkerMessage::SiteBuilt {
        generation,
        result: Err(format!("the site build crashed: {error}")),
    };
    crate::worker::spawn(sender, crashed, move |sender| {
        let result = run_build(&npm, &playbook, &folder, timeout, sender, generation);
        let _ = sender.send(WorkerMessage::SiteBuilt { generation, result });
    });
    Ok(CommandOutcome::Done)
}

/// Runs each step of the build in `folder`, all within `timeout`, the site's index page the
/// result when the build wrote one.
fn run_build(
    npm: &Path,
    playbook: &Path,
    folder: &Path,
    timeout: Duration,
    sender: &Sender<WorkerMessage>,
    generation: u64,
) -> Result<Option<PathBuf>, String> {
    let deadline = Instant::now() + timeout;
    let written = Arc::new(AtomicUsize::new(0));
    // npm's own scripts find `node` on the `PATH`: the folder `[tools] node` names comes first.
    let path = npm.parent().map(|bin| {
        let rest = env::var_os("PATH").unwrap_or_default();
        env::join_paths(std::iter::once(bin.to_path_buf()).chain(env::split_paths(&rest)))
            .unwrap_or(rest)
    });
    for step in steps(npm, playbook, folder) {
        let _ = sender.send(WorkerMessage::SiteBuildLine {
            generation,
            line: step.shown(),
        });
        run_step(
            &step,
            folder,
            path.as_ref(),
            (deadline, timeout),
            &written,
            sender,
            generation,
        )?;
    }
    let site = folder.join("build/site/index.html");
    Ok(site.is_file().then_some(site))
}

/// What a build of `playbook` runs: with a `package.json`, `npm install` while there is no
/// `node_modules`, then its `build` script; without one, or without the script, Antora itself.
fn steps(npm: &Path, playbook: &Path, folder: &Path) -> Vec<Step> {
    let step = |args: &[&str]| Step {
        program: npm.to_path_buf(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    };
    let package = folder.join("package.json");
    let mut steps = Vec::new();
    let mut scripted = false;
    if let Ok(text) = std::fs::read_to_string(&package) {
        if !folder.join("node_modules").is_dir() {
            steps.push(step(&["install"]));
        }
        scripted = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|package| package.get("scripts")?.get("build").cloned())
            .is_some_and(|script| script.is_string());
    }
    if scripted {
        steps.push(step(&["run", "build"]));
    } else {
        let name = playbook
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| adoc_antora::PLAYBOOK_FILE.to_owned());
        steps.push(step(&["exec", "--no", "--", "antora", "--fetch", &name]));
    }
    steps
}

/// Runs one step, sending each line it writes, until it exits, the build's deadline passes or
/// the build has written too much.
fn run_step(
    step: &Step,
    folder: &Path,
    path: Option<&OsString>,
    (deadline, timeout): (Instant, Duration),
    written: &Arc<AtomicUsize>,
    sender: &Sender<WorkerMessage>,
    generation: u64,
) -> Result<(), String> {
    let shown = step.shown();
    let shown = shown.trim_start_matches("$ ");
    let mut command = Command::new(&step.program);
    command
        .args(&step.args)
        .current_dir(folder)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // A group of its own, so stopping it stops what npm started too.
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {shown}: {error}"))?;
    let overflow = Arc::new(AtomicBool::new(false));
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let (sender, written, overflow) = (sender.clone(), written.clone(), overflow.clone());
        thread::spawn(move || read_lines(pipe, &sender, generation, &written, &overflow))
    };
    let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                stop(&mut child);
                return Err(format!("{shown} could not be waited for: {error}"));
            }
        }
        let stopped = if overflow.load(Ordering::Relaxed) {
            Some(format!(
                "the build wrote more than {} MiB and was stopped",
                MAX_BUILD_OUTPUT / (1024 * 1024)
            ))
        } else if Instant::now() >= deadline {
            Some(format!(
                "the build was stopped after {} s ([tools] build_timeout)",
                timeout.as_secs()
            ))
        } else {
            None
        };
        if let Some(message) = stopped {
            // The readers end once the pipes close; they are not waited for, in case a process
            // outside the group still holds them.
            stop(&mut child);
            return Err(message);
        }
        thread::sleep(POLL);
    };
    let _ = stdout.join();
    let _ = stderr.join();
    if overflow.load(Ordering::Relaxed) {
        return Err(format!(
            "the build wrote more than {} MiB and was stopped",
            MAX_BUILD_OUTPUT / (1024 * 1024)
        ));
    }
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

/// Sends each line of `pipe` until it ends or the build's output passes [`MAX_BUILD_OUTPUT`],
/// when `overflow` is set.
fn read_lines(
    pipe: Option<Box<dyn Read + Send>>,
    sender: &Sender<WorkerMessage>,
    generation: u64,
    written: &AtomicUsize,
    overflow: &AtomicBool,
) {
    let Some(pipe) = pipe else {
        return;
    };
    let mut reader = BufReader::new(pipe);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if written.fetch_add(read, Ordering::Relaxed) + read > MAX_BUILD_OUTPUT {
            overflow.store(true, Ordering::Relaxed);
            return;
        }
        let line = String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        if sender
            .send(WorkerMessage::SiteBuildLine { generation, line })
            .is_err()
        {
            return;
        }
    }
}

/// Stops `child` and every process in its group.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        use rustix::process::{kill_process_group, Pid, Signal};
        let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
    }
    #[cfg(windows)]
    {
        // The tree beneath it too: npm runs its scripts through a shell.
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(steps: &[Step]) -> Vec<String> {
        steps.iter().map(Step::shown).collect()
    }

    #[test]
    fn steps_install_once_and_prefer_the_build_script() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path();
        let playbook = folder.join("antora-playbook.yml");
        let npm = Path::new("npm");
        assert_eq!(
            args(&steps(npm, &playbook, folder)),
            ["$ npm exec --no -- antora --fetch antora-playbook.yml"]
        );
        std::fs::write(folder.join("package.json"), "{\"private\": true}").unwrap();
        assert_eq!(
            args(&steps(npm, &playbook, folder)),
            [
                "$ npm install",
                "$ npm exec --no -- antora --fetch antora-playbook.yml"
            ]
        );
        std::fs::write(
            folder.join("package.json"),
            "{\"scripts\": {\"build\": \"antora antora-playbook.yml\"}}",
        )
        .unwrap();
        std::fs::create_dir(folder.join("node_modules")).unwrap();
        assert_eq!(args(&steps(npm, &playbook, folder)), ["$ npm run build"]);
    }
}
