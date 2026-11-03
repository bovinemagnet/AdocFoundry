//! The tools' contracts (MCP PRD §§7.1 and 9), driven through the binary over stdio with real
//! JSON-RPC bodies: the existing names, permissions and text kept; structured content and
//! output schemas for sessions whose protocol has them; stable error codes; and cursors that
//! page resources and diagnostics, bound to one query and one workspace revision.

mod common;

use std::{collections::BTreeSet, path::Path, time::Instant};

use common::{scratch, Client, WAIT};
use serde_json::{json, Value};

/// The read-only tools: the ten that came before the contracts, kept as they were, and the
/// discovery tools added since.
const READ_TOOLS: [&str; 18] = [
    "analyse_impact",
    "check",
    "find_references",
    "get_nav",
    "get_outline",
    "get_page_context",
    "git_status",
    "link_graph",
    "list_components",
    "list_pages",
    "preview_changes",
    "quality_audit",
    "read_page",
    "render_page",
    "resolve",
    "search",
    "symbols",
    "validate_publication",
];

/// The tools a client had before the contracts, whose names and answers stay.
const EXISTING_READ_TOOLS: [&str; 10] = [
    "check",
    "get_nav",
    "git_status",
    "link_graph",
    "list_components",
    "read_page",
    "render_page",
    "resolve",
    "search",
    "symbols",
];

const WRITE_TOOLS: [&str; 8] = [
    "apply_changes",
    "create_module",
    "create_page",
    "create_project",
    "create_partial",
    "edit_page",
    "export_pdf",
    "rename_page",
];

/// A call of each read tool on `antora-single-component`.
fn read_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("list_components", json!({})),
        ("get_nav", json!({ "component": "demo" })),
        (
            "read_page",
            json!({ "resource_id": "latest@demo:security:page$authentication.adoc" }),
        ),
        (
            "resolve",
            json!({
                "target": "xref:security:authentication.adoc[]",
                "from": "modules/ROOT/pages/index.adoc",
            }),
        ),
        ("check", json!({ "fail_on": "warning" })),
        ("symbols", json!({ "query": "a" })),
        ("search", json!({ "query": "the" })),
        (
            "render_page",
            json!({ "path": "modules/ROOT/pages/index.adoc" }),
        ),
        ("link_graph", json!({})),
        ("git_status", json!({})),
        ("list_pages", json!({})),
        (
            "get_outline",
            json!({ "path": "modules/ROOT/pages/index.adoc" }),
        ),
        (
            "get_page_context",
            json!({ "path": "modules/ROOT/pages/index.adoc" }),
        ),
        (
            "find_references",
            json!({ "resource_id": "latest@demo:ROOT:partial$welcome.adoc" }),
        ),
        ("quality_audit", json!({})),
        ("validate_publication", json!({})),
        (
            "analyse_impact",
            json!({ "source": { "type": "paths", "paths": ["modules/ROOT/partials/welcome.adoc"] } }),
        ),
        (
            "preview_changes",
            json!({ "operations": [{
                "kind": "create_partial",
                "component": "demo",
                "module": "ROOT",
                "file_name": "fresh.adoc",
            }] }),
        ),
    ]
}

/// The tools listed, by name.
fn tools(client: &mut Client) -> Vec<Value> {
    client.request("tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .clone()
}

fn tool<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
}

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().expect("text content")
}

/// Whether `value` has the JSON Schema `schema`'s types, required fields and items, as far as
/// the output schemas use them.
fn conforms(value: &Value, schema: &Value, at: &str) -> Result<(), String> {
    if let Some(types) = schema.get("type") {
        let types: Vec<&str> = match types {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => {
                return Err(format!(
                    "{at}: a schema type that is neither a name nor a list"
                ))
            }
        };
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if !types.contains(&kind) && !(kind == "integer" && types.contains(&"number")) {
            return Err(format!("{at}: a {kind}, where the schema has {types:?}"));
        }
    }
    if let (Some(object), Some(required)) = (value.as_object(), schema.get("required")) {
        for name in required.as_array().unwrap() {
            let name = name.as_str().unwrap();
            if !object.contains_key(name) {
                return Err(format!("{at}: no `{name}`, which the schema requires"));
            }
        }
    }
    if let (Some(object), Some(properties)) = (value.as_object(), schema.get("properties")) {
        for (name, field) in object {
            if let Some(field_schema) = properties.get(name) {
                conforms(field, field_schema, &format!("{at}.{name}"))?;
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for (number, item) in items.iter().enumerate() {
            conforms(item, item_schema, &format!("{at}[{number}]"))?;
        }
    }
    Ok(())
}

#[test]
fn mcp_existing_tools_are_backwards_compatible() {
    let (_directory, root) = scratch("antora-single-component");
    let mut client = Client::start(&root, false);
    let names = client.tool_names();
    for name in EXISTING_READ_TOOLS {
        assert!(names.iter().any(|listed| listed == name), "{name} kept");
    }
    assert_eq!(names, READ_TOOLS);
    let listed = tools(&mut client);
    for name in READ_TOOLS {
        let annotations = &tool(&listed, name)["annotations"];
        assert_eq!(annotations["readOnlyHint"], true, "{name}");
        assert_eq!(annotations["openWorldHint"], false, "{name}");
    }
    // Every read tool still answers with its JSON as text, as before.
    for (name, arguments) in read_calls() {
        let value = client
            .call(name, arguments)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(value.is_object(), "{name}: {value}");
    }
    // Annotations are only metadata: without --allow-write no write tool is listed, and one
    // called anyway is refused with its code, the file untouched.
    for name in WRITE_TOOLS {
        let refused = client.call_result(
            name,
            json!({ "path": "modules/ROOT/pages/index.adoc", "edits": [], "new_name": "x.adoc", "output": "x.pdf" }),
        );
        assert_eq!(refused["isError"], true, "{name}");
        assert_eq!(
            refused["structuredContent"]["error"]["code"], "permission_denied",
            "{name}"
        );
        assert!(text_of(&refused).contains("--allow-write"));
    }
    assert!(!root.join("x.pdf").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    let mut expected: Vec<&str> = READ_TOOLS.to_vec();
    expected.extend(WRITE_TOOLS);
    expected.sort_unstable();
    assert_eq!(client.tool_names(), expected);
    let listed = tools(&mut client);
    for name in WRITE_TOOLS {
        assert_eq!(
            tool(&listed, name)["annotations"]["readOnlyHint"],
            false,
            "{name}"
        );
    }
    client.stop();
}

#[test]
fn mcp_structured_result_matches_text_fallback() {
    let (_directory, root) = scratch("antora-single-component");
    let mut client = Client::start(&root, false);
    let listed = tools(&mut client);
    for (name, arguments) in read_calls() {
        let result = client.call_result(name, arguments);
        assert_eq!(result["isError"], false, "{name}: {result}");
        let text: Value = serde_json::from_str(text_of(&result)).unwrap();
        assert_eq!(result["structuredContent"], text, "{name}");
        let schema = &tool(&listed, name)["outputSchema"];
        assert_eq!(schema["type"], "object", "{name} declares its output");
        conforms(&result["structuredContent"], schema, name).unwrap();
    }

    // A failure's text is its message, and its structured content adds the stable code.
    for (name, arguments, code) in [
        (
            "read_page",
            json!({ "path": "../outside.adoc" }),
            "outside_workspace",
        ),
        (
            "read_page",
            json!({ "resource_id": "latest@demo:ROOT:page$missing.adoc" }),
            "not_found",
        ),
        ("read_page", json!({}), "invalid_argument"),
        ("check", json!({ "fail_on": "fatal" }), "invalid_argument"),
        (
            "check",
            json!({ "cursor": "not-a-cursor" }),
            "invalid_argument",
        ),
        (
            "link_graph",
            json!({ "scope": "page", "path": "modules/ROOT/pages/absent.adoc" }),
            "not_found",
        ),
    ] {
        let result = client.call_result(name, arguments.clone());
        assert_eq!(result["isError"], true, "{name} {arguments}");
        let error = &result["structuredContent"]["error"];
        assert_eq!(error["code"], code, "{name} {arguments}: {result}");
        assert_eq!(error["message"], text_of(&result));
    }
    client.stop();
}

#[test]
fn mcp_negotiated_protocol_versions_are_pinned() {
    let (_directory, root) = scratch("antora-single-component");
    for (asked, structured) in [
        ("2025-11-25", true),
        ("2025-06-18", true),
        ("2025-03-26", false),
        ("2024-11-05", false),
    ] {
        let (mut client, initialized) = Client::start_with(&root, false, asked);
        assert_eq!(initialized["protocolVersion"], asked, "negotiated as asked");
        let listed = tools(&mut client);
        assert_eq!(
            tool(&listed, "list_components")
                .get("outputSchema")
                .is_some(),
            structured,
            "{asked}"
        );
        let result = client.call_result("list_components", json!({}));
        assert_eq!(
            result.get("structuredContent").is_some(),
            structured,
            "{asked}: {result}"
        );
        // Every session reads the text, the older ones only it.
        let text: Value = serde_json::from_str(text_of(&result)).unwrap();
        assert_eq!(text["components"][0]["name"], "demo");
        let failed = client.call_result("read_page", json!({ "path": "../outside.adoc" }));
        assert_eq!(failed["isError"], true);
        assert_eq!(failed.get("structuredContent").is_some(), structured);
        assert!(text_of(&failed).contains("outside the workspace"));
        client.stop();
    }
}

/// A workspace of one component with `pages` pages, two files outside it, and three pages with
/// two unresolved xrefs each.
fn big_workspace(pages: usize) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("docs"));
    let pages_dir = root.join("big/modules/ROOT/pages");
    std::fs::create_dir_all(&pages_dir).unwrap();
    std::fs::write(root.join("big/antora.yml"), "name: big\nversion: ~\n").unwrap();
    for number in 0..pages {
        std::fs::write(
            pages_dir.join(format!("p{number:04}.adoc")),
            format!("= Page {number}\n\nText.\n"),
        )
        .unwrap();
    }
    for number in 0..3 {
        std::fs::write(
            pages_dir.join(format!("broken{number}.adoc")),
            "= Broken\n\nSee xref:gone.adoc[] and xref:missing.adoc[].\n",
        )
        .unwrap();
    }
    std::fs::write(root.join("loose-a.adoc"), "= A\n").unwrap();
    std::fs::write(root.join("loose-b.adoc"), "= B\n").unwrap();
    (directory, root)
}

/// Every resource, listed a page at a time, and the size of each page.
fn all_resources(client: &mut Client) -> (Vec<String>, Vec<usize>) {
    let mut uris = Vec::new();
    let mut sizes = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(cursor) => json!({ "cursor": cursor }),
            None => json!({}),
        };
        let response = client.request("resources/list", params);
        let result = &response["result"];
        let page: Vec<String> = result["resources"]
            .as_array()
            .unwrap_or_else(|| panic!("{response}"))
            .iter()
            .map(|resource| resource["uri"].as_str().unwrap().to_owned())
            .collect();
        sizes.push(page.len());
        uris.extend(page);
        match result["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    (uris, sizes)
}

fn file_name(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap()
}

#[test]
fn mcp_resources_cursor_stable_and_bounded() {
    let (_directory, root) = big_workspace(1_203);
    let mut client = Client::start(&root, false);
    let (uris, sizes) = all_resources(&mut client);
    assert!(sizes.iter().all(|size| *size <= 500), "{sizes:?}");
    assert_eq!(sizes.len(), 3, "{sizes:?}");
    let unique: BTreeSet<&String> = uris.iter().collect();
    assert_eq!(unique.len(), uris.len(), "no resource twice");
    assert_eq!(uris.len(), 1_203 + 3 + 2, "every indexed file");

    // Files outside a component first, by path; then the component's by resource id.
    let names: Vec<&str> = uris.iter().map(|uri| file_name(uri)).collect();
    assert_eq!(&names[..2], ["loose-a.adoc", "loose-b.adoc"]);
    let mut by_id = names[2..].to_vec();
    by_id.sort_unstable();
    assert_eq!(names[2..], by_id[..]);

    let (again, _) = all_resources(&mut client);
    assert_eq!(again, uris, "the same pages in the same order");
    client.stop();
}

/// The workspace revision `check` reports.
fn revision(client: &mut Client) -> u64 {
    client.call("check", json!({ "limit": 1 })).unwrap()["workspace_revision"]
        .as_u64()
        .unwrap()
}

#[test]
fn mcp_cursor_rejects_stale_index_revision() {
    let (_directory, root) = big_workspace(600);
    let mut client = Client::start(&root, false);
    let first = client.request("resources/list", json!({}));
    let cursor = first["result"]["nextCursor"].as_str().unwrap().to_owned();
    let before = revision(&mut client);

    add_page_and_wait(&mut client, &root, before);
    let stale = client.request("resources/list", json!({ "cursor": cursor }));
    let error = &stale["error"];
    assert!(
        error["message"].as_str().unwrap().contains("stale"),
        "{stale}"
    );
    assert_eq!(error["data"]["code"], "invalid_argument");

    let (uris, _) = all_resources(&mut client);
    assert!(uris.iter().any(|uri| uri.ends_with("/added.adoc")));
    client.stop();
}

/// Writes a page on disk and waits until the server has indexed it. While it indexes, `check`
/// answers `index_incomplete`, which is a wait too.
fn add_page_and_wait(client: &mut Client, root: &Path, before: u64) {
    std::fs::write(root.join("big/modules/ROOT/pages/added.adoc"), "= Added\n").unwrap();
    let deadline = Instant::now() + WAIT;
    let indexed = |client: &mut Client| match client.call("check", json!({ "limit": 1 })) {
        Ok(result) => result["workspace_revision"].as_u64() != Some(before),
        Err(error) if error.contains("still being indexed") => false,
        Err(error) => panic!("{error}"),
    };
    while !indexed(client) {
        assert!(Instant::now() < deadline, "the page was never indexed");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn check_pages_its_diagnostics_with_a_cursor() {
    let (_directory, root) = big_workspace(2);
    let mut client = Client::start(&root, false);
    let whole = client
        .call("check", json!({ "fail_on": "warning" }))
        .unwrap();
    assert_eq!(whole["total"], 6, "{whole}");
    assert!(whole["next_cursor"].is_null());
    assert!(whole.get("truncated").is_none());

    let first = client
        .call("check", json!({ "fail_on": "warning", "limit": 4 }))
        .unwrap();
    assert_eq!(first["diagnostics"].as_array().unwrap().len(), 4);
    assert_eq!(first["failed"], true);
    assert_eq!(first["truncated"], json!({ "shown": 4, "total": 6 }));
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let second = client
        .call(
            "check",
            json!({ "fail_on": "warning", "limit": 4, "cursor": cursor }),
        )
        .unwrap();
    assert_eq!(second["diagnostics"].as_array().unwrap().len(), 2);
    assert!(second["next_cursor"].is_null());
    assert_eq!(second["failed"], true, "judged on every page, not this one");
    let mut paged = first["diagnostics"].as_array().unwrap().clone();
    paged.extend(second["diagnostics"].as_array().unwrap().iter().cloned());
    assert_eq!(&paged, whole["diagnostics"].as_array().unwrap());

    // The cursor holds to its query.
    let other = client.call_result("check", json!({ "fail_on": "error", "cursor": cursor }));
    assert_eq!(
        other["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
    client.stop();
}
