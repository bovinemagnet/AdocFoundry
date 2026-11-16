//! `export_docx`: Word export through the server, listed and callable only with
//! `--allow-write`, confined to the roots like every write.

mod common;

use std::path::Path;

use common::{copy_tree, fixture, Client};
use serde_json::json;

/// The DOCX export PRD, §10 test 16.
#[test]
fn docx_mcp_export_is_write_gated() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&fixture("pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docx.profiles.manual]\nnav = [\"guide/modules/admin/nav.adoc\", \"guide/modules/ROOT/nav.adoc\"]\noutput = \"build/manual.docx\"\n",
    )
    .unwrap();
    let page = "guide/modules/ROOT/pages/install.adoc";

    // Without `--allow-write`: not listed, and refused when called anyway.
    let mut client = Client::start(&root, false);
    assert!(!client.tool_names().iter().any(|name| name == "export_docx"));
    let result = client.call_result("export_docx", json!({ "path": page, "output": "a.docx" }));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "permission_denied",
        "{result}"
    );
    assert!(!root.join("a.docx").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    assert!(client.tool_names().iter().any(|name| name == "export_docx"));

    // A page as an article.
    let exported = client
        .call(
            "export_docx",
            json!({ "path": page, "output": "install.docx" }),
        )
        .unwrap();
    assert_eq!(exported["output"], "install.docx", "{exported}");
    assert_eq!(exported["pages"], 1, "{exported}");
    assert_eq!(exported["sources"], json!([page]), "{exported}");
    assert!(is_docx(&root.join("install.docx")));

    // A component version as a book, its pages in navigation order.
    let exported = client
        .call(
            "export_docx",
            json!({ "component": "1.0@guide", "output": "guide.docx" }),
        )
        .unwrap();
    let sources = exported["sources"].as_array().unwrap();
    assert!(sources.len() > 1, "{exported}");
    assert_eq!(exported["pages"], sources.len(), "{exported}");
    assert!(is_docx(&root.join("guide.docx")));

    // A profile writes what the engine's command writes.
    let exported = client
        .call("export_docx", json!({ "profile": "manual" }))
        .unwrap();
    assert_eq!(exported["output"], "build/manual.docx", "{exported}");
    let from_server = std::fs::read(root.join("build/manual.docx")).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(common::WAIT));
    let mut args = engine.docx_profile_export_args("manual").unwrap();
    let from_engine = directory.path().join("engine.docx");
    args.output = from_engine.clone();
    engine.export_docx_blocking(args, common::WAIT).unwrap();
    assert!(
        std::fs::read(&from_engine).unwrap() == from_server,
        "the server's document differs from the engine's"
    );

    // What is refused, and what each refusal says.
    std::fs::write(root.join("notes.txt"), "keep me").unwrap();
    let refusals = [
        (json!({ "path": page }), "invalid_argument"),
        (
            json!({ "path": page, "component": "guide", "output": "x.docx" }),
            "invalid_argument",
        ),
        (
            json!({ "profile": "manual", "path": page, "output": "x.docx" }),
            "invalid_argument",
        ),
        (json!({ "profile": "missing" }), "not_found"),
        (
            json!({ "path": page, "output": "../escaped.docx" }),
            "outside_workspace",
        ),
        (
            json!({ "path": page, "output": "page.pdf" }),
            "invalid_argument",
        ),
        (
            json!({ "path": page, "output": "notes.txt" }),
            "invalid_argument",
        ),
        (json!({ "path": page, "output": page }), "invalid_argument"),
    ];
    let before = common::snapshot(&root);
    for (arguments, code) in refusals {
        let result = client.call_result("export_docx", arguments.clone());
        assert_eq!(result["isError"], true, "{arguments}: {result}");
        assert_eq!(
            result["_meta"]["error"]["code"], code,
            "{arguments}: {result}"
        );
    }
    assert_eq!(
        common::snapshot(&root),
        before,
        "nothing written or replaced"
    );
    assert!(!directory.path().join("escaped.docx").exists());

    // A link is never written through, whether it leads outside the roots or stays inside.
    #[cfg(unix)]
    {
        let outside = directory.path().join("linked.docx");
        std::os::unix::fs::symlink(&outside, root.join("out.docx")).unwrap();
        std::os::unix::fs::symlink(root.join("install.docx"), root.join("inside.docx")).unwrap();
        let installed = std::fs::read(root.join("install.docx")).unwrap();
        for output in ["out.docx", "inside.docx"] {
            let result =
                client.call_result("export_docx", json!({ "path": page, "output": output }));
            assert_eq!(result["isError"], true, "{output}: {result}");
        }
        assert!(!outside.exists());
        assert_eq!(std::fs::read(root.join("install.docx")).unwrap(), installed);
    }
    client.stop();
}

fn is_docx(path: &Path) -> bool {
    std::fs::read(path).unwrap().starts_with(b"PK\x03\x04")
}
