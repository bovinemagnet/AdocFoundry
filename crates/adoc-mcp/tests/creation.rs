//! Antora-aware creation (MCP PRD §§7.6 and 11.3, tests 18 and 20): `create_page` and
//! `create_partial` through the binary over stdio on copies of `tests/fixtures/discovery`, and
//! every mutating tool refused without `--allow-write`. That a made page is one undo step with
//! its navigation entry, as in the app, is checked inside the server (`src/creation.rs`).

mod common;

use common::{scratch, Client};
use serde_json::{json, Value};

const MUTATING: [&str; 8] = [
    "apply_changes",
    "create_module",
    "create_page",
    "create_project",
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

/// MCP PRD §11.3 test 19: `create_module` is New Module's engine command, with no layout or
/// `antora.yml` writing of its own: the module, its navigation and its one line in the
/// descriptor are what `module.new` makes.
#[test]
fn mcp_create_module_reuses_engine_command() {
    let (_directory, root) = scratch("discovery");
    let descriptor = root.join("product/v3/antora.yml");
    let before = std::fs::read_to_string(&descriptor).unwrap();
    let mut client = Client::start(&root, true);
    let made = client
        .call(
            "create_module",
            json!({ "component": "product", "version": "3.0", "name": "billing" }),
        )
        .unwrap();
    assert_eq!(made["path"], "product/v3/modules/billing");
    assert_eq!(
        made["made"],
        json!([
            "product/v3/modules/billing/images/.gitkeep",
            "product/v3/modules/billing/nav.adoc",
            "product/v3/modules/billing/pages/index.adoc",
            "product/v3/modules/billing/partials/.gitkeep",
        ])
    );
    assert_eq!(made["index_page"], "3.0@product:billing:page$index.adoc");
    let after = std::fs::read_to_string(&descriptor).unwrap();
    assert_eq!(after, format!("{before}  - modules/billing/nav.adoc\n"));
    // The module resolves at once: its page is listed, in its own navigation.
    let pages = client
        .call(
            "list_pages",
            json!({ "component": "product", "version": "3.0", "module": "billing" }),
        )
        .unwrap();
    assert_eq!(
        pages["items"][0]["resource_id"],
        "3.0@product:billing:page$index.adoc"
    );

    for (arguments, code) in [
        (
            json!({ "component": "product", "version": "3.0", "name": "billing" }),
            "conflict",
        ),
        (
            json!({ "component": "product", "version": "3.0", "name": "admin" }),
            "conflict",
        ),
        (
            json!({ "component": "product", "name": "other" }),
            "invalid_argument",
        ),
        (
            json!({ "component": "product", "version": "3.0", "name": "Bad Name" }),
            "invalid_argument",
        ),
        (json!({ "component": "nope", "name": "x" }), "not_found"),
    ] {
        let refused = client.call_result("create_module", arguments.clone());
        assert_eq!(error_code(&refused), code, "{arguments}");
    }
    client.stop();
}

/// `create_project` is New Antora Project's engine command (#292), confined to the server's
/// roots: it never makes a root of its own.
#[test]
fn mcp_create_project_reuses_engine_command() {
    let (directory, root) = scratch("discovery");
    std::fs::create_dir(root.join("handbook")).unwrap();
    let mut client = Client::start(&root, true);
    let made = client
        .call(
            "create_project",
            json!({ "path": "handbook", "name": "handbook", "title": "The Handbook" }),
        )
        .unwrap();
    assert_eq!(made["path"], "handbook");
    assert_eq!(
        made["made"],
        json!([
            "handbook/.adocfoundry.toml",
            "handbook/.gitignore",
            "handbook/README.adoc",
            "handbook/antora-playbook.yml",
            "handbook/antora.yml",
            "handbook/modules/ROOT/images/.gitkeep",
            "handbook/modules/ROOT/nav.adoc",
            "handbook/modules/ROOT/pages/index.adoc",
            "handbook/modules/ROOT/partials/.gitkeep",
            "handbook/package.json",
        ])
    );
    assert_eq!(made["index_page"], "handbook:ROOT:page$index.adoc");
    assert_eq!(made["repository"], false, "no repository unless asked");
    assert!(!root.join("handbook/.git").exists());
    // The component resolves at once.
    let pages = client
        .call("list_pages", json!({ "component": "handbook" }))
        .unwrap();
    assert_eq!(
        pages["items"][0]["resource_id"],
        "handbook:ROOT:page$index.adoc"
    );

    let outside = directory.path().join("elsewhere");
    std::fs::create_dir(&outside).unwrap();
    for (arguments, code) in [
        (
            json!({ "path": "handbook", "name": "again" }),
            "invalid_argument",
        ),
        (
            json!({ "path": "product", "name": "x", "subfolder": "v3" }),
            "conflict",
        ),
        (
            json!({ "path": "missing", "name": "x" }),
            "invalid_argument",
        ),
        (
            json!({ "path": "guide", "name": "Bad Name" }),
            "invalid_argument",
        ),
        (
            json!({ "path": outside.to_string_lossy(), "name": "x" }),
            "outside_workspace",
        ),
    ] {
        let refused = client.call_result("create_project", arguments.clone());
        assert_eq!(error_code(&refused), code, "{arguments}");
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    client.stop();
}
