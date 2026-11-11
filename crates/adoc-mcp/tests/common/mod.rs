//! The stdio client the integration tests drive `adoc-mcp` with, as an MCP client does.

#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use serde_json::{json, Value};

pub const WAIT: Duration = Duration::from_secs(30);

pub fn fixture(name: &str) -> PathBuf {
    adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of fixture `name` to change, under a temporary directory.
pub fn scratch(name: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join(name));
    copy_tree(&fixture(name), &root);
    (directory, root)
}

pub struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    /// Notifications the server sent while a request waited for its answer.
    notifications: Vec<Value>,
}

impl Client {
    pub fn start(root: &Path, allow_write: bool) -> Self {
        Self::start_with(root, allow_write, "2025-06-18").0
    }

    /// A client of a server on `root` in a session that asked for protocol `version`, and the
    /// server's answer to `initialize`.
    pub fn start_with(root: &Path, allow_write: bool, version: &str) -> (Self, Value) {
        let options: &[&str] = if allow_write { &["--allow-write"] } else { &[] };
        Self::start_with_options(root, options, version)
    }

    /// A client of a server on `root` started with `options`, such as `--allow-build`.
    pub fn start_with_options(root: &Path, options: &[&str], version: &str) -> (Self, Value) {
        Self::start_with_env(root, options, version, &[])
    }

    /// A client of a server on `root` started with `options` and its environment changed by
    /// `env`, where `None` removes a variable.
    pub fn start_with_env(
        root: &Path,
        options: &[&str],
        version: &str,
        env: &[(&str, Option<&std::ffi::OsStr>)],
    ) -> (Self, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoc-mcp"));
        command.arg(root).args(options);
        for (name, value) in env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start adoc-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            lines,
            next_id: 1,
            notifications: Vec::new(),
        };
        let initialized = client.request(
            "initialize",
            json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": { "name": "tests", "version": "0.0.0" },
            }),
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "adoc-mcp");
        client.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        (client, initialized["result"].clone())
    }

    pub fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The whole JSON-RPC response to `method`.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self.lines.recv_timeout(WAIT).expect("a response");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
            if message.get("id").is_none() && message.get("method").is_some() {
                self.notifications.push(message);
            }
        }
    }

    pub fn tool_names(&mut self) -> Vec<String> {
        let listed = self.request("tools/list", json!({}));
        let mut names: Vec<String> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }

    /// The JSON a tool answered with, or `Err` with its error message.
    pub fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        if let Some(error) = response.get("error") {
            return Err(error["message"].as_str().unwrap_or_default().to_owned());
        }
        let result = &response["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        if result["isError"] == json!(true) {
            return Err(text.to_owned());
        }
        Ok(serde_json::from_str(text).unwrap())
    }

    /// The whole result of a tool call, text and structured content and all; a protocol
    /// error panics.
    pub fn call_result(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    /// The notifications with `method` the server sends within `wait`, and any it sent while
    /// earlier requests waited.
    pub fn notifications(&mut self, method: &str, wait: Duration) -> Vec<Value> {
        let deadline = std::time::Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let message: Value = serde_json::from_str(&line).unwrap();
                    if message.get("id").is_none() {
                        self.notifications.push(message);
                    }
                }
                Err(_) => break,
            }
        }
        let (wanted, rest): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.notifications)
            .into_iter()
            .partition(|message| message["method"] == method);
        self.notifications = rest;
        wanted
    }

    pub fn stop(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

/// Every file beneath `root` and what it holds, to tell whether anything was made, changed or
/// removed.
pub fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                folders.push(entry.path());
            } else {
                files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            }
        }
    }
    files
}
