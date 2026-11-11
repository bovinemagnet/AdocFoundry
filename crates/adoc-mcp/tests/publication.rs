//! Publication and prompts (MCP PRD §§7.7 and 11.4, test 24): `validate_publication` on copies
//! of `tests/fixtures/discovery`, the workflow prompts, and site builds kept out of reach of
//! `--allow-write`.

mod common;

use common::{fixture, scratch, Client};
use serde_json::{json, Value};

fn version<'a>(report: &'a Value, component: &str, version: Option<&str>) -> &'a Value {
    report["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["component"] == component && entry["version"].as_str() == version)
        .unwrap_or_else(|| panic!("no {component} {version:?} in {report}"))
}

#[test]
fn validate_publication_reports_what_the_engine_checks() {
    let (_directory, root) = scratch("discovery");
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf.profiles.manual]\ncomponent = \"3.0@product\"\n\n\
         [pdf.profiles.broken]\npage = \"product/main/modules/ROOT/pages/gone.adoc\"\n",
    )
    .unwrap();
    let mut client = Client::start(&root, false);
    let report = client.call("validate_publication", json!({})).unwrap();
    assert_eq!(report["schema_version"], 1);

    let main = version(&report, "product", None);
    assert_eq!(main["start_page"]["target"], "ROOT:index.adoc");
    assert_eq!(main["start_page"]["default"], true);
    assert_eq!(main["start_page"]["resolves"], true);
    assert_eq!(
        main["start_page"]["path"],
        "product/main/modules/ROOT/pages/index.adoc"
    );
    assert_eq!(main["navigation_files"][0]["exists"], true);
    assert_eq!(main["coverage"]["pages"], 3);
    assert_eq!(main["coverage"]["listed"], 2, "the draft page is unlisted");

    let profiles = report["profiles"].as_array().unwrap();
    let manual = profiles
        .iter()
        .find(|profile| profile["name"] == "manual")
        .unwrap();
    assert_eq!(manual["ready"], true, "{manual}");
    let broken = profiles
        .iter()
        .find(|profile| profile["name"] == "broken")
        .unwrap();
    assert_eq!(broken["ready"], false);
    assert!(broken["problems"][0]
        .as_str()
        .unwrap()
        .contains("gone.adoc"));
    assert_eq!(report["outcome"], "not_ready", "a profile cannot export");
    assert!(report["audit"]["counts"]["warning"].as_u64().unwrap() >= 1);
    assert!(report["not_checked"][0]
        .as_str()
        .unwrap()
        .contains("Antora build"));

    // One component version, with its profile fixed: ready, but only as far as was checked.
    let guide = client
        .call("validate_publication", json!({ "component": "guide" }))
        .unwrap();
    assert_eq!(guide["navigation"].as_array().unwrap().len(), 1);
    assert_eq!(guide["audit"]["completeness"]["status"], "complete");
    assert_eq!(guide["outcome"], "not_ready", "profiles are always checked");
    client.stop();

    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf.profiles.manual]\ncomponent = \"1.0@guide\"\n",
    )
    .unwrap();
    let mut client = Client::start(&root, false);
    let guide = client
        .call("validate_publication", json!({ "component": "guide" }))
        .unwrap();
    assert_eq!(guide["outcome"], "ready", "{guide}");
    let refused = client.call_result("validate_publication", json!({ "version": "1.0" }));
    assert_eq!(refused["_meta"]["error"]["code"], "invalid_argument");
    client.stop();
}

#[test]
fn the_prompts_are_recipes_that_grant_nothing() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, false);
    let listed = client.request("prompts/list", json!({}));
    let names: Vec<&str> = listed["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|prompt| prompt["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "create_howto",
            "prepare_release",
            "review_documentation",
            "review_documentation_change",
        ]
    );
    let tools_before = client.tool_names();
    for (name, arguments, mentions) in [
        (
            "create_howto",
            json!({ "component": "product", "topic": "authentication" }),
            ["list_pages", "preview_changes", "apply_changes"],
        ),
        (
            "prepare_release",
            json!({ "component": "product", "version": "3.0" }),
            ["validate_publication", "quality_audit", "preview_changes"],
        ),
        (
            "review_documentation",
            json!({}),
            ["quality_audit", "get_page_context", "preview_changes"],
        ),
        (
            "review_documentation_change",
            json!({ "pages": "product:ROOT:page$install.adoc" }),
            ["get_page_context", "find_references", "quality_audit"],
        ),
    ] {
        let got = client.request(
            "prompts/get",
            json!({ "name": name, "arguments": arguments }),
        );
        let text = got["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: {got}"));
        for tool in mentions {
            assert!(text.contains(tool), "{name} names {tool}");
        }
        assert!(
            text.contains("Only the server decides what you may change"),
            "{name}"
        );
    }
    assert_eq!(
        client.tool_names(),
        tools_before,
        "a prompt changes no tool"
    );
    let missing = client.request(
        "prompts/get",
        json!({ "name": "create_howto", "arguments": {} }),
    );
    assert!(
        missing.get("error").is_some(),
        "a required argument is required"
    );
    let unknown = client.request("prompts/get", json!({ "name": "deploy", "arguments": {} }));
    assert!(unknown.get("error").is_some());
    client.stop();
}

/// MCP PRD §11.4 test 24: running a site build is not something `--allow-write` grants. No
/// build tool is offered with it, and the separate `--allow-build` is not accepted until the
/// engine has a build command (#293).
#[test]
fn mcp_build_not_enabled_by_allow_write() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, true);
    let names = client.tool_names();
    assert!(
        names.iter().any(|name| name == "apply_changes"),
        "writing is allowed"
    );
    assert!(
        !names.iter().any(|name| name.contains("build")),
        "but building is not: {names:?}"
    );
    // Called anyway, it is refused for want of --allow-build, as a write tool is without
    // --allow-write.
    let refused = client.call_result("build_site", json!({}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(refused["_meta"]["error"]["code"], "permission_denied");
    assert!(refused["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("--allow-build"));
    client.stop();

    // `--allow-build` offers it, and nothing that writes.
    let (mut client, _) =
        Client::start_with_options(&fixture("discovery"), &["--allow-build"], "2025-06-18");
    let names = client.tool_names();
    assert!(names.iter().any(|name| name == "build_site"), "{names:?}");
    assert!(
        !names.iter().any(|name| name == "apply_changes"),
        "{names:?}"
    );
    client.stop();
}
