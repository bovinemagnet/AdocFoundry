//! `adoc-mcp --http` driven through its binary, as a client of MCP's Streamable HTTP transport
//! drives it. The client is a few lines of HTTP/1.1 over a socket, so the tests bring no HTTP
//! client into the tree.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(30);
const TOKEN: &str = "test-token";

fn fixture(name: &str) -> PathBuf {
    adoc_engine::normalize_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name),
    )
}

/// An HTTP response: its status, headers (names in lower case) and body.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    reader: BufReader<TcpStream>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body's bytes as they arrive, chunked or not, until `done` holds or the body ends.
    fn read_until(&mut self, mut done: impl FnMut(&str) -> bool) -> String {
        let mut body = String::new();
        if self
            .header("transfer-encoding")
            .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
        {
            loop {
                let mut size = String::new();
                if self.reader.read_line(&mut size).unwrap_or(0) == 0 {
                    break;
                }
                let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                if size == 0 {
                    break;
                }
                let mut chunk = vec![0; size + 2];
                self.reader.read_exact(&mut chunk).unwrap();
                body.push_str(&String::from_utf8_lossy(&chunk[..size]));
                if done(&body) {
                    break;
                }
            }
        } else if let Some(length) = self.header("content-length") {
            let mut bytes = vec![0; length.parse().unwrap()];
            self.reader.read_exact(&mut bytes).unwrap();
            body.push_str(&String::from_utf8_lossy(&bytes));
        }
        body
    }
}

/// The JSON-RPC message answering `id` in `body`, plain JSON or server-sent events.
fn message(body: &str, id: u64) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return (value["id"] == json!(id)).then_some(value);
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .find(|value| value["id"] == json!(id))
}

fn post(port: u16, token: Option<&str>, session: Option<&str>, payload: &Value) -> Reply {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let body = payload.to_string();
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n\
         MCP-Protocol-Version: 2025-06-18\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(session) = session {
        request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    (&stream).write_all(request.as_bytes()).unwrap();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        let Some((name, value)) = line.trim_end().split_once(':') else {
            break;
        };
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    Reply {
        status,
        headers,
        reader,
    }
}

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(root: &Path, allow_write: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoc-mcp"));
        command.arg(root).args(["--http", "127.0.0.1:0"]);
        if allow_write {
            command.arg("--allow-write");
        }
        let mut child = command
            .env("ADOC_MCP_TOKEN", TOKEN)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start adoc-mcp");
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stderr.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let port = loop {
            let line = lines.recv_timeout(WAIT).expect("the listening address");
            if let Some(address) = line.strip_prefix("adoc-mcp: listening on http://") {
                let address = address.strip_suffix("/mcp").unwrap();
                break address.rsplit_once(':').unwrap().1.parse().unwrap();
            }
        };
        Self { child, port }
    }

    /// A client in an initialised session.
    fn session(&self) -> Client {
        let mut reply = post(
            self.port,
            Some(TOKEN),
            None,
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "tests", "version": "0.0.0" },
                },
            }),
        );
        assert_eq!(reply.status, 200);
        let session = reply.header("mcp-session-id").map(str::to_owned);
        let body = reply.read_until(|body| message(body, 1).is_some());
        let initialized = message(&body, 1).expect("an initialize response");
        assert_eq!(initialized["result"]["serverInfo"]["name"], "adoc-mcp");
        let mut client = Client {
            port: self.port,
            session,
            next_id: 2,
        };
        let reply =
            client.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        assert!(reply.status < 300, "{}", reply.status);
        client
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    port: u16,
    session: Option<String>,
    next_id: u64,
}

impl Client {
    fn post(&mut self, payload: &Value) -> Reply {
        post(self.port, Some(TOKEN), self.session.as_deref(), payload)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut reply =
            self.post(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        assert_eq!(reply.status, 200);
        let body = reply.read_until(|body| message(body, id).is_some());
        message(&body, id).unwrap_or_else(|| panic!("no response to {method}: {body}"))
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
}

#[test]
fn serves_over_http_with_a_token() {
    let server = Server::start(&fixture("antora-single-component"), false);
    let mut client = server.session();
    assert!(client.tool_names().contains(&"read_page".to_owned()));
    let response = client.request(
        "tools/call",
        json!({
            "name": "read_page",
            "arguments": { "resource_id": "latest@demo:security:page$authentication.adoc" },
        }),
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let page: Value = serde_json::from_str(text).unwrap();
    assert_eq!(page["title"], "Authentication");
}

#[test]
fn requests_without_the_token_are_refused() {
    let server = Server::start(&fixture("references"), false);
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "tests", "version": "0.0.0" },
        },
    });
    assert_eq!(post(server.port, None, None, &initialize).status, 401);
    assert_eq!(
        post(server.port, Some("not-the-token"), None, &initialize).status,
        401
    );
    assert_eq!(
        post(server.port, Some(TOKEN), None, &initialize).status,
        200
    );
}

#[test]
fn http_binds_loopback_only() {
    let output = Command::new(env!("CARGO_BIN_EXE_adoc-mcp"))
        .arg(fixture("references"))
        .args(["--http", "0.0.0.0:0"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
}

#[test]
fn write_tools_need_allow_write_over_http() {
    let server = Server::start(&fixture("references"), false);
    let names = server.session().tool_names();
    assert!(!names.iter().any(|name| name == "edit_page"), "{names:?}");
    drop(server);

    let server = Server::start(&fixture("references"), true);
    let names = server.session().tool_names();
    for tool in ["edit_page", "export_pdf", "rename_page"] {
        assert!(names.iter().any(|name| name == tool), "{names:?}");
    }
}
