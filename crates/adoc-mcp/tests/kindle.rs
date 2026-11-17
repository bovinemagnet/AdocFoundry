//! `export_kindle`: Kindle export through the server, listed and callable only with
//! `--allow-write`, confined to the roots like every write.

mod common;

use std::path::Path;

use common::{copy_tree, fixture, Client};
use serde_json::json;

fn is_azw3(path: &Path) -> bool {
    std::fs::read(path).unwrap().get(60..68) == Some(&b"BOOKMOBI"[..])
}

fn is_kfx(path: &Path) -> bool {
    std::fs::read(path).unwrap().starts_with(b"CONT")
}

/// The Kindle export PRD, §12 test 17.
#[test]
fn kindle_mcp_export_is_write_gated() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&fixture("pdf/book"), &root);
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[kindle.profiles.handbook]\ncomponent = \"1.0@guide\"\nformat = \"kfx\"\noutput = \"build/handbook.kfx\"\n",
    )
    .unwrap();
    let page = "guide/modules/ROOT/pages/install.adoc";

    // Without `--allow-write`: not listed, and refused when called anyway.
    let mut client = Client::start(&root, false);
    assert!(!client
        .tool_names()
        .iter()
        .any(|name| name == "export_kindle"));
    let result = client.call_result("export_kindle", json!({ "path": page, "output": "a.azw3" }));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "permission_denied",
        "{result}"
    );
    assert!(!root.join("a.azw3").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    assert!(client
        .tool_names()
        .iter()
        .any(|name| name == "export_kindle"));

    // A page, in the format its output names.
    let exported = client
        .call(
            "export_kindle",
            json!({ "path": page, "output": "install.azw3" }),
        )
        .unwrap();
    assert_eq!(exported["output"], "install.azw3", "{exported}");
    assert_eq!(exported["format"], "azw3", "{exported}");
    assert_eq!(exported["sources"], json!([page]), "{exported}");
    assert!(is_azw3(&root.join("install.azw3")));
    let exported = client
        .call(
            "export_kindle",
            json!({ "path": page, "format": "kfx", "output": "install.kfx" }),
        )
        .unwrap();
    assert_eq!(exported["format"], "kfx", "{exported}");
    assert!(is_kfx(&root.join("install.kfx")));

    // A component version as a book.
    let exported = client
        .call(
            "export_kindle",
            json!({ "component": "1.0@guide", "output": "guide.kfx" }),
        )
        .unwrap();
    assert_eq!(exported["pages"], 8, "{exported}");
    assert!(is_kfx(&root.join("guide.kfx")));

    // A profile writes what the engine writes.
    let exported = client
        .call("export_kindle", json!({ "profile": "handbook" }))
        .unwrap();
    assert_eq!(exported["output"], "build/handbook.kfx", "{exported}");
    let from_server = std::fs::read(root.join("build/handbook.kfx")).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(common::WAIT));
    let mut args = engine.kindle_profile_export_args("handbook").unwrap();
    let from_engine = directory.path().join("engine.kfx");
    args.output = from_engine.clone();
    engine.export_kindle_blocking(args, common::WAIT).unwrap();
    assert!(
        std::fs::read(&from_engine).unwrap() == from_server,
        "the server's book differs from the engine's"
    );

    // What is refused, and what each refusal says; nothing is written or replaced.
    std::fs::write(root.join("notes.txt"), "keep me").unwrap();
    let refusals = [
        (json!({ "path": page }), "invalid_argument"),
        (
            json!({ "path": page, "component": "guide", "output": "x.kfx" }),
            "invalid_argument",
        ),
        (json!({ "profile": "missing" }), "not_found"),
        (
            json!({ "path": page, "output": "../escaped.kfx" }),
            "outside_workspace",
        ),
        (
            json!({ "path": page, "output": "book.epub" }),
            "invalid_argument",
        ),
        (
            json!({ "path": page, "format": "kfx", "output": "x.azw3" }),
            "invalid_argument",
        ),
        (
            json!({ "path": page, "format": "mobi", "output": "x.mobi" }),
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
        let result = client.call_result("export_kindle", arguments.clone());
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
    assert!(!directory.path().join("escaped.kfx").exists());

    // A link is never written through, whether it leads outside the roots or stays inside.
    #[cfg(unix)]
    {
        let outside = directory.path().join("linked.kfx");
        std::os::unix::fs::symlink(&outside, root.join("out.kfx")).unwrap();
        std::os::unix::fs::symlink(root.join("install.kfx"), root.join("inside.kfx")).unwrap();
        let installed = std::fs::read(root.join("install.kfx")).unwrap();
        for output in ["out.kfx", "inside.kfx"] {
            let result =
                client.call_result("export_kindle", json!({ "path": page, "output": output }));
            assert_eq!(result["isError"], true, "{output}: {result}");
        }
        assert!(!outside.exists());
        assert_eq!(std::fs::read(root.join("install.kfx")).unwrap(), installed);
    }
    client.stop();
}
