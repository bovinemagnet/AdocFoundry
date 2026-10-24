//! The language server on the engine, driven through its stdio binary: the compatibility it
//! keeps and the features the engine brings.

use std::{
    io::BufReader,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{
    notification::{Exit, Initialized, Notification as LspNotification, PublishDiagnostics},
    request::{Initialize, Request as LspRequest, Shutdown},
    InitializeResult, InitializedParams, Location, PublishDiagnosticsParams, WorkspaceEdit,
};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    adoc_index::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

fn uri_path(uri: &str) -> PathBuf {
    url::Url::parse(uri).unwrap().to_file_path().unwrap()
}

/// A client talking to `adoc-ls --stdio`.
struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Message>,
    /// Notifications read while waiting for something else.
    notifications: Vec<Notification>,
    next_id: i32,
    capabilities: lsp_types::ServerCapabilities,
}

impl Client {
    fn start(root: Option<&Path>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_adoc-ls"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start adoc-ls");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = Message::read(&mut stdout) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            messages,
            notifications: Vec::new(),
            next_id: 1,
            capabilities: lsp_types::ServerCapabilities::default(),
        };
        let mut params = json!({ "capabilities": {} });
        if let Some(root) = root {
            params["rootUri"] = json!(file_uri(root));
        }
        let response = client.request(Initialize::METHOD, params);
        let result: InitializeResult =
            serde_json::from_value(response.response_result.expect("initialized")).unwrap();
        client.capabilities = result.capabilities;
        client.notify(Initialized::METHOD, json!(InitializedParams {}));
        client
    }

    fn notify(&mut self, method: &str, params: Value) {
        Message::Notification(Notification::new(method.to_owned(), params))
            .write(&mut self.stdin)
            .unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Response {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        Message::Request(Request::new(id.clone(), method.to_owned(), params))
            .write(&mut self.stdin)
            .unwrap();
        loop {
            match self.messages.recv_timeout(WAIT).expect("a response") {
                Message::Response(response) if response.id == id => return response,
                Message::Notification(notification) => self.notifications.push(notification),
                _ => {}
            }
        }
    }

    fn open(&mut self, path: &Path, text: &str) -> String {
        let uri = file_uri(path);
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": "asciidoc", "version": 1, "text": text,
            }}),
        );
        uri
    }

    /// The next diagnostics published for `uri` that satisfy `wanted`.
    fn diagnostics(
        &mut self,
        uri: &str,
        wanted: impl Fn(&PublishDiagnosticsParams) -> bool,
    ) -> PublishDiagnosticsParams {
        let matches = |notification: &Notification| {
            notification.method == PublishDiagnostics::METHOD
                && serde_json::from_value::<PublishDiagnosticsParams>(notification.params.clone())
                    .is_ok_and(|params| params.uri.as_str() == uri && wanted(&params))
        };
        if let Some(index) = self.notifications.iter().position(matches) {
            let notification = self.notifications.remove(index);
            return serde_json::from_value(notification.params).unwrap();
        }
        loop {
            match self.messages.recv_timeout(WAIT).expect("diagnostics") {
                Message::Notification(notification) if matches(&notification) => {
                    return serde_json::from_value(notification.params).unwrap();
                }
                Message::Notification(notification) => self.notifications.push(notification),
                _ => {}
            }
        }
    }

    fn shutdown(mut self) {
        let _ = self.request(Shutdown::METHOD, Value::Null);
        self.notify(Exit::METHOD, Value::Null);
        assert!(self.child.wait().unwrap().success());
    }
}

fn codes(params: &PublishDiagnosticsParams) -> Vec<String> {
    let mut codes: Vec<String> = params
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match &diagnostic.code {
            Some(lsp_types::NumberOrString::String(code)) => Some(code.clone()),
            _ => None,
        })
        .collect();
    codes.sort();
    codes
}

#[test]
fn command_ids_and_stdio_are_unchanged() {
    let client = Client::start(None);
    let commands = client
        .capabilities
        .execute_command_provider
        .clone()
        .expect("commands")
        .commands;
    for expected in [
        "adoc.renderPreview",
        "adoc.renderLivePreview",
        "adoc.stopLivePreview",
        "adoc.checkWorkspace",
        // These tests build the server with Typst.
        "adoc.exportPdf",
    ] {
        assert!(
            commands.iter().any(|command| command == expected),
            "{commands:?}"
        );
    }
    assert!(client.capabilities.references_provider.is_some());
    assert!(client.capabilities.rename_provider.is_some());
    client.shutdown();
}

#[test]
fn diagnostics_come_from_the_engine() {
    let root = fixture("broken-references");
    let page = root.join("index.adoc");
    let text = std::fs::read_to_string(&page).unwrap();

    let mut engine = adoc_engine::Engine::new(adoc_engine::Settings::default());
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let id = engine.open(&page).unwrap();
    let mut expected: Vec<String> = engine
        .document(id)
        .unwrap()
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str().to_owned())
        .collect();
    expected.sort();
    assert!(!expected.is_empty());

    let mut client = Client::start(Some(&root));
    let uri = client.open(&page, &text);
    let published = client.diagnostics(&uri, |_| true);
    assert_eq!(codes(&published), expected);
    client.shutdown();
}

#[test]
fn incremental_changes_apply() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    let mut client = Client::start(Some(directory.path()));
    let uri = client.open(&page, "= Page\n\n[[here]]\n== Here\n\nSee <<there>>.\n");
    let first = client.diagnostics(&uri, |_| true);
    assert_eq!(codes(&first), ["adoc.unresolved-anchor"]);

    // Two changes in one notification, the second's range in the text the first left.
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [
                { "range": { "start": { "line": 5, "character": 6 },
                             "end": { "line": 5, "character": 11 } }, "text": "here" },
                { "range": { "start": { "line": 0, "character": 2 },
                             "end": { "line": 0, "character": 6 } }, "text": "Changed" },
            ],
        }),
    );
    let changed = client.diagnostics(&uri, |params| params.version == Some(2));
    assert!(changed.diagnostics.is_empty(), "{changed:?}");

    let response = client.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 5, "character": 7 } }),
    );
    let location: Location = serde_json::from_value(response.response_result.unwrap()).unwrap();
    assert_eq!(
        location.range.start.line, 2,
        "the anchor the edited text names"
    );
    client.shutdown();
}

#[test]
fn references_list_linking_pages() {
    let root = fixture("references");
    let page = root.join("index.adoc");
    let mut client = Client::start(Some(&root));
    let uri = client.open(&page, &std::fs::read_to_string(&page).unwrap());
    let response = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 2 },
            "context": { "includeDeclaration": false },
        }),
    );
    let locations: Vec<Location> =
        serde_json::from_value(response.response_result.unwrap()).unwrap();
    let found: Vec<_> = locations
        .iter()
        .map(|location| {
            (
                uri_path(location.uri.as_str())
                    .file_name()
                    .unwrap()
                    .to_owned(),
                location.range.start.line,
            )
        })
        .collect();
    assert_eq!(found, [("other.adoc".into(), 2), ("other.adoc".into(), 2)]);
    client.shutdown();
}

/// The files a workspace edit changes, with how many edits each.
fn edited_files(edit: &WorkspaceEdit) -> Vec<(String, usize)> {
    let mut files: Vec<_> = edit
        .changes
        .as_ref()
        .unwrap()
        .iter()
        .map(|(uri, edits)| {
            let path = uri_path(uri.as_str());
            let parent = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            let name = path.file_name().unwrap().to_string_lossy();
            (format!("{parent}/{name}"), edits.len())
        })
        .collect();
    files.sort();
    files
}

#[test]
fn will_rename_files_updates_xrefs_and_nav() {
    let root = fixture("antora-single-component");
    let page = root.join("modules/security/pages/authentication.adoc");
    let mut client = Client::start(Some(&root));
    let response = client.request(
        "workspace/willRenameFiles",
        json!({ "files": [{
            "oldUri": file_uri(&page),
            "newUri": file_uri(&page.with_file_name("login.adoc")),
        }]}),
    );
    let edit: WorkspaceEdit = serde_json::from_value(response.response_result.unwrap()).unwrap();
    assert_eq!(
        edited_files(&edit),
        [
            ("ROOT/nav.adoc".to_owned(), 1),
            ("pages/index.adoc".to_owned(), 1)
        ]
    );
    let new_text: Vec<_> = edit
        .changes
        .unwrap()
        .into_values()
        .flatten()
        .map(|edit| edit.new_text)
        .collect();
    assert_eq!(new_text, ["security:login.adoc", "security:login.adoc"]);
    client.shutdown();
}

#[test]
fn anchor_rename_updates_references() {
    let root = fixture("references");
    let page = root.join("index.adoc");
    let mut client = Client::start(Some(&root));
    let uri = client.open(&page, &std::fs::read_to_string(&page).unwrap());
    let response = client.request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 2, "character": 4 },
            "newName": "install",
        }),
    );
    let edit: WorkspaceEdit = serde_json::from_value(response.response_result.unwrap()).unwrap();
    assert_eq!(
        edited_files(&edit),
        [
            ("references/index.adoc".to_owned(), 2),
            ("references/other.adoc".to_owned(), 1)
        ]
    );

    // A page is renamed by renaming its file, not here.
    let refused = client.request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 2 },
            "newName": "install",
        }),
    );
    assert!(refused.response_result.is_err());
    client.shutdown();
}

#[test]
fn export_pdf_command_writes_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let page = directory.path().join("page.adoc");
    std::fs::write(&page, "= Exported\n\nA paragraph.\n").unwrap();
    let output = directory.path().join("page.pdf");
    let mut client = Client::start(Some(directory.path()));
    let uri = client.open(&page, "= Exported\n\nA paragraph, unsaved.\n");
    let response = client.request(
        "workspace/executeCommand",
        json!({
            "command": "adoc.exportPdf",
            "arguments": [{ "uri": uri, "output": output }],
        }),
    );
    assert!(
        response.response_result.is_ok(),
        "{:?}",
        response.response_result
    );
    let pdf = std::fs::read(&output).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    assert!(client
        .notifications
        .iter()
        .any(|notification| notification.method == "window/showMessage"
            && notification.params["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("Exported"))));
    client.shutdown();
}
