use std::{
    io::BufReader,
    process::{Command, Stdio},
};

use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{
    notification::{Exit, Initialized, Notification as LspNotification},
    request::{Initialize, Request as LspRequest, Shutdown},
    InitializeParams, InitializedParams,
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
