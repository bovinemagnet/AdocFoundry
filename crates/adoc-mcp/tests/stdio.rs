//! `adoc-mcp` driven through its binary over stdio, as an MCP client drives it.

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(30);

fn fixture(name: &str) -> PathBuf {
    adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

fn copy_tree(from: &Path, to: &Path) {
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
fn scratch(name: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join(name));
    copy_tree(&fixture(name), &root);
    (directory, root)
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    fn start(root: &Path, allow_write: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoc-mcp"));
        command.arg(root);
        if allow_write {
            command.arg("--allow-write");
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
        };
        let initialized = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "tests", "version": "0.0.0" },
            }),
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "adoc-mcp");
        client.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        client
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The whole JSON-RPC response to `method`.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self.lines.recv_timeout(WAIT).expect("a response");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    fn tool_names(&mut self) -> Vec<String> {
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
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
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

    fn stop(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

const READ_TOOLS: [&str; 9] = [
    "check",
    "get_nav",
    "git_status",
    "list_components",
    "read_page",
    "render_page",
    "resolve",
    "search",
    "symbols",
];

#[test]
fn lists_read_only_tools_by_default() {
    let mut client = Client::start(&fixture("references"), false);
    assert_eq!(client.tool_names(), READ_TOOLS);
    let resources = client.request("resources/list", json!({}));
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris.len(), 3, "{uris:?}");
    assert!(uris.iter().all(|uri| uri.starts_with("file://")));
    client.stop();
}

#[test]
fn reads_a_page_by_resource_id() {
    let mut client = Client::start(&fixture("antora-single-component"), false);
    let page = client
        .call(
            "read_page",
            json!({ "resource_id": "latest@demo:security:page$authentication.adoc" }),
        )
        .unwrap();
    assert_eq!(page["path"], "modules/security/pages/authentication.adoc");
    assert_eq!(page["title"], "Authentication");
    assert_eq!(
        page["includes"],
        json!([{
            "target": "partial$token-note.adoc",
            "resolves_to": "modules/security/partials/token-note.adoc",
        }])
    );
    assert!(page["text"]
        .as_str()
        .unwrap()
        .starts_with("= Authentication"));

    let resource = client.request(
        "resources/read",
        json!({ "uri": "antora:latest@demo:security:page$authentication.adoc" }),
    );
    assert!(resource["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("= Authentication"));
    client.stop();
}

#[test]
fn check_matches_the_cli() {
    let root = fixture("broken-references");
    let mut engine = adoc_engine::Engine::new(adoc_engine::Settings::default());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let expected = serde_json::to_value(engine.check_report(&root, None)).unwrap();
    assert!(!expected.as_array().unwrap().is_empty());

    let mut client = Client::start(&root, false);
    let checked = client.call("check", json!({})).unwrap();
    assert_eq!(checked["diagnostics"], expected);
    assert_eq!(checked["failed"], false, "only warnings: {checked}");
    let strict = client
        .call("check", json!({ "fail_on": "warning" }))
        .unwrap();
    assert_eq!(strict["failed"], true);
    client.stop();
}

#[test]
fn search_finds_matches() {
    let mut client = Client::start(&fixture("references"), false);
    let found = client.call("search", json!({ "query": "setup" })).unwrap();
    let matches: Vec<(String, u64)> = found["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|found| {
            (
                found["path"].as_str().unwrap().to_owned(),
                found["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert!(
        matches.contains(&("index.adoc".to_owned(), 3)),
        "{matches:?}"
    );
    assert!(
        matches.contains(&("other.adoc".to_owned(), 3)),
        "{matches:?}"
    );
    client.stop();
}

#[test]
fn render_page_as_text() {
    let mut client = Client::start(&fixture("references"), false);
    let rendered = client
        .call("render_page", json!({ "path": "index.adoc" }))
        .unwrap();
    let text = rendered["content"].as_str().unwrap();
    assert!(text.starts_with("Index\n\nSetup\n"), "{text}");
    assert!(
        text.contains("Shared text."),
        "the include is rendered: {text}"
    );
    assert!(!text.contains("xref:"), "{text}");
    client.stop();
}

#[test]
fn write_tools_need_allow_write() {
    let (_directory, root) = scratch("references");
    let mut client = Client::start(&root, false);
    let refused = client.call(
        "edit_page",
        json!({ "path": "index.adoc", "edits": [{ "old_text": "Index", "new_text": "Home" }] }),
    );
    assert!(refused.is_err());
    assert!(std::fs::read_to_string(root.join("index.adoc"))
        .unwrap()
        .starts_with("= Index"));
    client.stop();

    let mut client = Client::start(&root, true);
    let mut expected: Vec<&str> = READ_TOOLS.to_vec();
    expected.extend(["edit_page", "export_pdf", "rename_page"]);
    expected.sort_unstable();
    assert_eq!(client.tool_names(), expected);
    client.stop();
}

#[test]
fn edit_page_applies_through_commands() {
    let (_directory, root) = scratch("references");
    let mut client = Client::start(&root, true);

    // Unsaved, the edit is in the engine's buffer, which reading the page shows.
    client
        .call(
            "edit_page",
            json!({ "path": "index.adoc", "edits": [{ "old_text": "= Index", "new_text": "= Home" }] }),
        )
        .unwrap();
    let page = client
        .call("read_page", json!({ "path": "index.adoc" }))
        .unwrap();
    assert!(page["text"].as_str().unwrap().starts_with("= Home"));
    assert!(std::fs::read_to_string(root.join("index.adoc"))
        .unwrap()
        .starts_with("= Index"));

    // Text that is not there exactly once is refused, and nothing of that call applies.
    let ambiguous = client.call(
        "edit_page",
        json!({ "path": "index.adoc", "edits": [
            { "old_text": "= Home", "new_text": "= Start" },
            { "old_text": "setup", "new_text": "install" },
        ]}),
    );
    assert!(ambiguous.unwrap_err().contains("occurs"));
    let page = client
        .call("read_page", json!({ "path": "index.adoc" }))
        .unwrap();
    assert!(page["text"].as_str().unwrap().starts_with("= Home"));

    let saved = client
        .call(
            "edit_page",
            json!({ "path": "index.adoc", "edits": [{ "range": { "start": 2, "end": 6 }, "new_text": "Start" }], "save": true }),
        )
        .unwrap();
    assert_eq!(saved["saved"], true);
    assert!(std::fs::read_to_string(root.join("index.adoc"))
        .unwrap()
        .starts_with("= Start"));
    client.stop();
}

#[test]
fn rename_page_updates_xrefs() {
    let (_directory, root) = scratch("references");
    let mut client = Client::start(&root, true);
    let renamed = client
        .call(
            "rename_page",
            json!({ "path": "other.adoc", "new_name": "guides/other.adoc" }),
        )
        .unwrap();
    assert_eq!(renamed["to"], "guides/other.adoc");
    assert!(!root.join("other.adoc").exists());
    assert!(root.join("guides/other.adoc").is_file());
    let index = std::fs::read_to_string(root.join("index.adoc")).unwrap();
    assert!(
        index.contains("xref:guides/other.adoc[the other page]"),
        "{index}"
    );
    client.stop();
}

#[test]
fn paths_outside_the_roots_are_refused() {
    let mut client = Client::start(&fixture("references"), false);
    for path in ["../simple/index.adoc", "/etc/hosts"] {
        let refused = client.call("read_page", json!({ "path": path }));
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("outside")),
            "{path}: {refused:?}"
        );
    }
    let resource = client.request("resources/read", json!({ "uri": "file:///etc/hosts" }));
    assert!(resource.get("error").is_some(), "{resource}");
    client.stop();
}

/// A workspace root `docs` with `page.adoc`, beside files outside it that the page names.
fn escaping_workspace(page: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let outside = adoc_engine::normalize_path(directory.path());
    let root = outside.join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(outside.join("outside.adoc"), "OUTSIDE-MARKER\n").unwrap();
    std::fs::write(outside.join("theme.yml"), "extends: default\n").unwrap();
    std::fs::copy(
        fixture("pdf/reference/images/pipeline.svg"),
        outside.join("outside.svg"),
    )
    .unwrap();
    std::fs::write(root.join("page.adoc"), page).unwrap();
    (directory, root)
}

#[test]
fn includes_stay_inside_the_roots() {
    let (_directory, root) = escaping_workspace("= Page\n\ninclude::../outside.adoc[]\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("../outside.adoc"), root.join("link.adoc")).unwrap();
    let mut client = Client::start(&root, false);

    let rendered = client
        .call("render_page", json!({ "path": "page.adoc" }))
        .unwrap();
    let text = rendered["content"].as_str().unwrap();
    assert!(!text.contains("OUTSIDE-MARKER"), "{text}");

    // Through a link inside the root, too.
    #[cfg(unix)]
    {
        std::fs::write(root.join("page.adoc"), "= Page\n\ninclude::link.adoc[]\n").unwrap();
        let rendered = client
            .call("render_page", json!({ "path": "page.adoc" }))
            .unwrap();
        let text = rendered["content"].as_str().unwrap();
        assert!(!text.contains("OUTSIDE-MARKER"), "{text}");
    }
    client.stop();
}

#[test]
fn pdf_export_reads_nothing_outside_the_roots() {
    let (_directory, root) =
        escaping_workspace("= Page\n\nimage::../outside.svg[]\n\ninclude::../outside.adoc[]\n");
    let mut client = Client::start(&root, true);

    let refused = client.call(
        "export_pdf",
        json!({
            "path": "page.adoc",
            "output": "page.pdf",
            "theme": root.join("../theme.yml").to_string_lossy(),
        }),
    );
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.contains("outside")),
        "the theme outside the roots is refused: {refused:?}"
    );

    let exported = client
        .call(
            "export_pdf",
            json!({ "path": "page.adoc", "output": "page.pdf" }),
        )
        .unwrap();
    let warnings = exported["warnings"].to_string();
    assert!(
        warnings.contains("outside"),
        "the image is refused: {warnings}"
    );
    let pdf = std::fs::read(root.join("page.pdf")).unwrap();
    assert!(
        !pdf.windows(14).any(|window| window == b"OUTSIDE-MARKER"),
        "the include is not in the PDF"
    );
    client.stop();
}

#[test]
fn a_page_added_on_disk_is_indexed_while_the_server_is_idle() {
    let (_directory, root) = scratch("references");
    let mut client = Client::start(&root, false);
    let titles = |client: &mut Client| {
        client
            .call("symbols", json!({ "query": "Added later" }))
            .unwrap()["symbols"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(titles(&mut client), 0);

    std::fs::write(root.join("added.adoc"), "= Added later\n\nText.\n").unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while titles(&mut client) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the change was never applied"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    client.stop();
}
