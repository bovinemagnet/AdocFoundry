//! Running a program so that stopping it stops everything it started: a launcher script's
//! Java, npm's shell and Node.js. The diagram tools here and the engine's site build share it.

use std::process::{Child, Command};

/// Starts `command` in a process group of its own on Unix, so [`stop`] reaches every process
/// it starts. On Windows, [`stop`] follows the process tree instead.
pub fn in_own_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

/// Stops `child` and every process it started, and waits for it: by its process group on Unix
/// (`child` started through [`in_own_group`]), by its process tree on Windows.
pub fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        use rustix::process::{kill_process_group, Pid, Signal};
        let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
    }
    #[cfg(windows)]
    {
        use std::process::Stdio;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
