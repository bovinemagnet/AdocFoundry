//! `adoc-mcp` driven through its binary over stdio, as an MCP client drives it.

mod common;

use std::{path::PathBuf, time::Duration};

use common::{fixture, scratch, Client, WAIT};
use serde_json::json;

const READ_TOOLS: [&str; 16] = [
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
fn link_graph_tool_returns_a_local_graph() {
    let root = adoc_engine::normalize_path(&fixture("graph"));
    let mut client = Client::start(&root, false);
    let local = client
        .call(
            "link_graph",
            json!({
                "scope": "page",
                "path": "guide/modules/ROOT/pages/configure.adoc",
                "depth": 1,
            }),
        )
        .unwrap();
    let ids: Vec<&str> = local["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "guide/modules/ROOT/pages/configure.adoc",
            "guide/modules/ROOT/pages/index.adoc",
            "guide/modules/ROOT/pages/install.adoc"
        ]
    );
    let kinds: Vec<(&str, &str)> = local["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| edge["to"] == "guide/modules/ROOT/pages/configure.adoc")
        .map(|edge| {
            (
                edge["from"].as_str().unwrap(),
                edge["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("guide/modules/ROOT/pages/install.adoc", "xref"),
            ("guide/modules/ROOT/pages/install.adoc", "nav")
        ]
    );

    let drawn = client
        .call(
            "link_graph",
            json!({
                "scope": "page",
                "path": "guide/modules/ROOT/pages/configure.adoc",
                "format": "svg",
            }),
        )
        .unwrap();
    let svg = drawn["svg"].as_str().unwrap();
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert_eq!(svg.matches("class=\"node page\"").count(), 3);
    assert!(client
        .call("link_graph", json!({ "format": "png" }))
        .is_err());

    let pages = "guide/modules/ROOT/pages";
    let path = client
        .call(
            "link_graph",
            json!({
                "scope": "path",
                "path": format!("{pages}/index.adoc"),
                "to": format!("{pages}/configure.adoc"),
            }),
        )
        .unwrap();
    let titles: Vec<&str> = path["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Home", "Install", "Configure"]);
    let either = client
        .call(
            "link_graph",
            json!({
                "scope": "path",
                "path": format!("{pages}/index.adoc"),
                "to": format!("{pages}/configure.adoc"),
                "directed": false,
            }),
        )
        .unwrap();
    assert_eq!(either["path"][1]["backwards"], true);
    let none = client
        .call(
            "link_graph",
            json!({
                "scope": "path",
                "path": format!("{pages}/index.adoc"),
                "to": format!("{pages}/orphan.adoc"),
            }),
        )
        .unwrap();
    assert!(none["path"].is_null());
    assert!(client
        .call(
            "link_graph",
            json!({ "scope": "path", "path": format!("{pages}/index.adoc") })
        )
        .is_err());

    let whole = client
        .call("link_graph", json!({ "unresolved": true }))
        .unwrap();
    assert!(whole["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["kind"] == "unresolved"));
    assert!(client
        .call(
            "link_graph",
            json!({ "scope": "page", "path": "../outside.adoc" })
        )
        .is_err());
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
    expected.extend([
        "apply_changes",
        "create_module",
        "create_page",
        "create_partial",
        "edit_page",
        "export_pdf",
        "rename_page",
    ]);
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
