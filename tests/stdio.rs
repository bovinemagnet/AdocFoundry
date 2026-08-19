use std::{
    io::BufReader,
    process::{Command, Stdio},
};

use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{
    notification::{Exit, Initialized, Notification as LspNotification},
    request::{Initialize, Request as LspRequest, Shutdown},
    InitializeParams, InitializeResult, InitializedParams,
};

#[test]
fn binary_completes_the_stdio_lifecycle() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoc-ls"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start adoc-ls");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    Message::Request(Request::new(
        RequestId::from(1),
        Initialize::METHOD.to_owned(),
        InitializeParams::default(),
    ))
    .write(&mut stdin)
    .expect("send initialize");
    assert_success_response(read_message(&mut stdout), RequestId::from(1));
    Message::Notification(Notification::new(
        Initialized::METHOD.to_owned(),
        InitializedParams {},
    ))
    .write(&mut stdin)
    .expect("send initialized");

    Message::Request(Request::new(
        RequestId::from(2),
        Shutdown::METHOD.to_owned(),
        (),
    ))
    .write(&mut stdin)
    .expect("send shutdown");
    assert_success_response(read_message(&mut stdout), RequestId::from(2));

    Message::Notification(Notification::new(Exit::METHOD.to_owned(), ()))
        .write(&mut stdin)
        .expect("send exit");
    drop(stdin);

    assert!(child.wait().expect("wait for adoc-ls").success());
}

fn read_message(stdout: &mut BufReader<impl std::io::Read>) -> Message {
    Message::read(stdout)
        .expect("read server message")
        .expect("server closed stdout")
}

fn assert_success_response(message: Message, expected_id: RequestId) {
    let Message::Response(Response {
        id,
        response_result,
    }) = message
    else {
        panic!("expected response, received {message:?}");
    };

    assert_eq!(id, expected_id);
    assert!(response_result.is_ok(), "response was {response_result:?}");
}

/// The preview flow is unreachable unless the server advertises both halves of it.
#[test]
fn binary_advertises_the_preview_command() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoc-ls"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start adoc-ls");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    Message::Request(Request::new(
        RequestId::from(1),
        Initialize::METHOD.to_owned(),
        InitializeParams::default(),
    ))
    .write(&mut stdin)
    .expect("send initialize");

    let Message::Response(response) = read_message(&mut stdout) else {
        panic!("expected an initialize response");
    };
    let value = response.response_result.expect("initialize succeeded");
    let result: InitializeResult = serde_json::from_value(value).expect("decode result");
    let capabilities = result.capabilities;

    assert!(
        capabilities.code_action_provider.is_some(),
        "code actions must be advertised or the preview cannot be invoked"
    );
    let commands = capabilities
        .execute_command_provider
        .expect("execute command provider")
        .commands;
    // Zed drops any code action whose command is absent from this list, so every
    // command a code action carries has to be advertised here.
    for expected in [
        "adoc.renderPreview",
        "adoc.renderLivePreview",
        "adoc.stopLivePreview",
    ] {
        assert!(
            commands.iter().any(|command| command == expected),
            "{expected} missing from {commands:?}"
        );
    }

    // A live preview re-renders on save, which a client only sends if asked.
    let sync = capabilities
        .text_document_sync
        .expect("text document sync capability");
    let lsp_types::TextDocumentSyncCapability::Options(options) = sync else {
        panic!("expected sync options rather than a bare kind");
    };
    assert!(
        options.save.is_some(),
        "save notifications must be requested"
    );

    Message::Notification(Notification::new(
        Initialized::METHOD.to_owned(),
        InitializedParams {},
    ))
    .write(&mut stdin)
    .expect("send initialized");
    Message::Request(Request::new(
        RequestId::from(2),
        Shutdown::METHOD.to_owned(),
        (),
    ))
    .write(&mut stdin)
    .expect("send shutdown");
    let _ = read_message(&mut stdout);
    Message::Notification(Notification::new(Exit::METHOD.to_owned(), ()))
        .write(&mut stdin)
        .expect("send exit");
    assert!(child.wait().expect("wait for adoc-ls").success());
}

/// The mirror of the capability check: a command may be advertised, offered by a code
/// action, and still not routed. Executing each one proves it reaches a handler — the
/// document is deliberately not open, so the expected outcome is that specific failure
/// rather than "unsupported command".
#[test]
fn binary_routes_every_advertised_command() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoc-ls"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start adoc-ls");
    let mut stdin = child.stdin.take().expect("child stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));

    Message::Request(Request::new(
        RequestId::from(1),
        Initialize::METHOD.to_owned(),
        InitializeParams::default(),
    ))
    .write(&mut stdin)
    .expect("send initialize");
    let Message::Response(response) = read_message(&mut stdout) else {
        panic!("expected an initialize response");
    };
    let result: InitializeResult =
        serde_json::from_value(response.response_result.expect("initialize succeeded"))
            .expect("decode result");
    let commands = result
        .capabilities
        .execute_command_provider
        .expect("execute command provider")
        .commands;
    Message::Notification(Notification::new(
        Initialized::METHOD.to_owned(),
        InitializedParams {},
    ))
    .write(&mut stdin)
    .expect("send initialized");

    for (index, command) in commands.iter().enumerate() {
        let id = RequestId::from(index as i32 + 2);
        Message::Request(Request::new(
            id.clone(),
            "workspace/executeCommand".to_owned(),
            serde_json::json!({
                "command": command,
                "arguments": ["file:///not/open.adoc"],
            }),
        ))
        .write(&mut stdin)
        .expect("send executeCommand");

        let Message::Response(response) = read_message(&mut stdout) else {
            panic!("expected a response for {command}");
        };
        if let Err(error) = response.response_result {
            assert!(
                !error.message.contains("unsupported command"),
                "`{command}` is advertised but not routed: {}",
                error.message
            );
        }
    }

    Message::Request(Request::new(
        RequestId::from(1000),
        Shutdown::METHOD.to_owned(),
        (),
    ))
    .write(&mut stdin)
    .expect("send shutdown");
    let _ = read_message(&mut stdout);
    Message::Notification(Notification::new(Exit::METHOD.to_owned(), ()))
        .write(&mut stdin)
        .expect("send exit");
    assert!(child.wait().expect("wait for adoc-ls").success());
}
