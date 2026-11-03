//! Antora-aware creation (MCP PRD §§7.6 and 11.3, tests 18 and 20): `create_page` and
//! `create_partial` through the binary over stdio on copies of `tests/fixtures/discovery`, and
//! every mutating tool refused without `--allow-write`. That a made page is one undo step with
//! its navigation entry, as in the app, is checked inside the server (`src/creation.rs`).

mod common;

use common::{scratch, Client};
use serde_json::{json, Value};

const MUTATING: [&str; 6] = [
    "apply_changes",
    "create_page",
    "create_partial",
    "edit_page",
    "export_pdf",
    "rename_page",
];

fn error_code(result: &Value) -> &str {
    assert_eq!(result["isError"], true, "{result}");
    result["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap()
}

#[test]
fn mcp_write_denied_by_default() {
    let (_directory, root) = scratch("discovery");
    let before = common::snapshot(&root);
    let mut client = Client::start(&root, false);
    let names = client.tool_names();
    for tool in MUTATING {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} is not offered"
        );
        let refused = client.call_result(
            tool,
            json!({
                "component": "product", "version": "~", "module": "ROOT", "title": "Made",
                "file_name": "made.adoc", "path": "guide/modules/ROOT/pages/index.adoc",
                "edits": [], "new_name": "moved.adoc", "output": "out.pdf", "preview_id": "0",
            }),
        );
        assert_eq!(error_code(&refused), "permission_denied", "{tool}");
    }
    client.stop();
    assert_eq!(
        common::snapshot(&root),
        before,
        "no file made, changed or removed"
    );

    let mut client = Client::start(&root, true);
    let names = client.tool_names();
    for tool in MUTATING {
        assert!(
            names.iter().any(|name| name == tool),
            "{tool} with --allow-write"
        );
    }
    client.stop();
}

#[test]
fn create_page_resolves_and_is_listed_in_its_navigation() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, true);
    let made = client
        .call(
            "create_page",
            json!({
                "component": "product",
                "version": "~",
                "module": "ROOT",
                "title": "Configuring Authentication",
                "add_to_navigation": true,
            }),
        )
        .unwrap();
    assert_eq!(
        made["resource_id"],
        "product:ROOT:page$configuring-authentication.adoc"
    );
    assert_eq!(
        made["path"],
        "product/main/modules/ROOT/pages/configuring-authentication.adoc"
    );
    assert_eq!(
        made["navigation_file"],
        "product/main/modules/ROOT/nav.adoc"
    );
    assert_eq!(made["saved"], true);
    let nav = std::fs::read_to_string(root.join("product/main/modules/ROOT/nav.adoc")).unwrap();
    assert!(
        nav.ends_with("* xref:configuring-authentication.adoc[]\n"),
        "{nav}"
    );
    let page = client
        .call(
            "read_page",
            json!({ "resource_id": "product:ROOT:page$configuring-authentication.adoc" }),
        )
        .unwrap();
    assert_eq!(page["title"], "Configuring Authentication");
    // The link graph follows the new entry as soon as the engine has applied it.
    let deadline = std::time::Instant::now() + common::WAIT;
    loop {
        let context = client
            .call(
                "get_page_context",
                json!({ "resource_id": "product:ROOT:page$configuring-authentication.adoc" }),
            )
            .unwrap();
        if context["navigation"]["in_navigation"] == true {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{context}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // The same again collides; an ambiguous version or a bad name is refused.
    for (arguments, code) in [
        (
            json!({ "component": "product", "version": "~", "module": "ROOT", "title": "Configuring Authentication" }),
            "conflict",
        ),
        (
            json!({ "component": "product", "module": "ROOT", "title": "Elsewhere" }),
            "invalid_argument",
        ),
        (
            json!({ "component": "product", "version": "~", "module": "ROOT", "title": "X", "file_name": "../x.adoc" }),
            "invalid_argument",
        ),
        (
            json!({ "component": "product", "version": "~", "module": "ROOT", "title": "Y", "add_to_navigation": true, "expected_nav_hash": "00" }),
            "conflict",
        ),
    ] {
        let refused = client.call_result("create_page", arguments.clone());
        assert_eq!(error_code(&refused), code, "{arguments}");
    }
    client.stop();
}

#[test]
fn create_partial_gives_the_include_that_names_it() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, true);
    let made = client
        .call(
            "create_partial",
            json!({
                "component": "product",
                "version": "3.0",
                "module": "admin",
                "file_name": "shared/limits.adoc",
                "text": "Limits apply.\n",
            }),
        )
        .unwrap();
    assert_eq!(
        made["path"],
        "product/v3/modules/admin/partials/shared/limits.adoc"
    );
    assert_eq!(
        made["resource_id"],
        "3.0@product:admin:partial$shared/limits.adoc"
    );
    assert_eq!(
        made["include"]["from_its_module"],
        "include::partial$shared/limits.adoc[]"
    );
    assert_eq!(
        made["include"]["from_anywhere"],
        "include::3.0@product:admin:partial$shared/limits.adoc[]"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("product/v3/modules/admin/partials/shared/limits.adoc"))
            .unwrap(),
        "Limits apply.\n"
    );
    let again = client.call_result(
        "create_partial",
        json!({ "component": "product", "version": "3.0", "module": "admin", "file_name": "shared/limits.adoc" }),
    );
    assert_eq!(error_code(&again), "conflict");
    client.stop();
}
