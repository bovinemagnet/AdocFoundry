//! `adocfoundry-cli check` and `adoc-ls` must report the same diagnostics for the same files.
//!
//! The CLI prints the engine's workspace diagnostics, so the engine stands in for it here; the
//! language server is driven through its real stdio binary with every AsciiDoc file opened.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use adoc_core::LineIndex;
use adoc_engine::{Engine, Settings};
use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types::{
    notification::{Exit, Initialized, Notification as LspNotification},
    request::{Initialize, Request as LspRequest, Shutdown},
    InitializedParams, PublishDiagnosticsParams,
};

/// `(line, character, code)` for every diagnostic, by file.
type Findings = BTreeMap<PathBuf, BTreeSet<(u32, u32, String)>>;

fn fixtures() -> PathBuf {
    adoc_index::normalize_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures"))
}

fn asciidoc_files(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            asciidoc_files(&path, out);
        } else if adoc_index::is_asciidoc_path(&path) {
            out.push(path);
        }
    }
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

fn engine_findings(root: &Path) -> Findings {
    let mut engine = Engine::new(Settings::default());
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    engine
        .workspace_diagnostics()
        .into_iter()
        // The server publishes for open AsciiDoc documents only, never for `antora.yml`.
        .filter(|(path, _)| adoc_index::is_asciidoc_path(path))
        .map(|(path, diagnostics)| {
            let lines = LineIndex::new(&fs::read_to_string(&path).unwrap());
            let found = diagnostics
                .into_iter()
                .map(|diagnostic| {
                    let (line, column) = lines.line_column(diagnostic.range.start);
                    (
                        line as u32,
                        column as u32,
                        diagnostic.code.as_str().to_owned(),
                    )
                })
                .collect();
            (path, found)
        })
        .collect()
}

fn server_findings(root: &Path, files: &[PathBuf]) -> Findings {
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoc-ls"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start adoc-ls");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    Message::Request(Request::new(
        RequestId::from(1),
        Initialize::METHOD.to_owned(),
        serde_json::json!({ "capabilities": {}, "rootUri": file_uri(root) }),
    ))
    .write(&mut stdin)
    .unwrap();
    let _ = Message::read(&mut stdout).unwrap();
    Message::Notification(Notification::new(
        Initialized::METHOD.to_owned(),
        InitializedParams {},
    ))
    .write(&mut stdin)
    .unwrap();
    for (version, path) in files.iter().enumerate() {
        Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri(path),
                    "languageId": "asciidoc",
                    "version": version,
                    "text": fs::read_to_string(path).unwrap(),
                },
            }),
        ))
        .write(&mut stdin)
        .unwrap();
    }
    Message::Request(Request::new(
        RequestId::from(2),
        Shutdown::METHOD.to_owned(),
        (),
    ))
    .write(&mut stdin)
    .unwrap();

    // Every open republishes all open documents, so the last notification per file is final.
    let mut latest: Findings = BTreeMap::new();
    loop {
        match Message::read(&mut stdout).unwrap().expect("server output") {
            Message::Notification(notification)
                if notification.method == "textDocument/publishDiagnostics" =>
            {
                let params: PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                let path = url::Url::parse(params.uri.as_str())
                    .unwrap()
                    .to_file_path()
                    .unwrap();
                let found = params
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| {
                        let code = match diagnostic.code {
                            Some(lsp_types::NumberOrString::String(code)) => code,
                            other => format!("{other:?}"),
                        };
                        (
                            diagnostic.range.start.line,
                            diagnostic.range.start.character,
                            code,
                        )
                    })
                    .collect();
                latest.insert(adoc_index::normalize_path(&path), found);
            }
            Message::Response(response) if response.id == RequestId::from(2) => break,
            _ => {}
        }
    }
    Message::Notification(Notification::new(Exit::METHOD.to_owned(), ()))
        .write(&mut stdin)
        .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    latest.retain(|_, found: &mut BTreeSet<_>| !found.is_empty());
    latest
}

#[test]
fn cli_check_and_adoc_ls_report_the_same_diagnostics() {
    let mut checked = 0;
    let mut compared = 0;
    let mut roots: Vec<PathBuf> = fs::read_dir(fixtures())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    roots.sort();
    for root in roots {
        let mut files = Vec::new();
        asciidoc_files(&root, &mut files);
        if files.is_empty() {
            continue;
        }
        let engine = engine_findings(&root);
        compared += engine.values().map(BTreeSet::len).sum::<usize>();
        assert_eq!(
            engine,
            server_findings(&root, &files),
            "diagnostics differ for fixture {}",
            root.display()
        );
        checked += 1;
    }
    assert!(checked >= 10, "every fixture is compared");
    assert!(
        compared >= 5,
        "the fixtures exercise real diagnostics ({compared} compared)"
    );
}
