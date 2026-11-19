//! `export_epub`: EPUB export through the server, listed and callable only with
//! `--allow-write`, confined to the roots like every write.

mod common;

use common::{copy_tree, fixture, Client};
use serde_json::json;

/// The EPUB written at `path`, checked whole as a reader would check it, with its title.
fn epub_title(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let (_, package) = adoc_epub::testing::assert_valid_ocf(&bytes);
    package.title
}

#[test]
fn epub_mcp_export_is_write_gated() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("book"));
    copy_tree(&fixture("pdf/book"), &root);
    // Dated, so the server's book and the engine's are the same bytes.
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[epub.profiles.handbook]\ncomponent = \"1.0@guide\"\noutput = \"build/handbook.epub\"\nmodified = \"2026-11-19\"\n",
    )
    .unwrap();
    let page = "guide/modules/ROOT/pages/install.adoc";

    // Without `--allow-write`: not listed, and refused when called anyway.
    let mut client = Client::start(&root, false);
    assert!(!client.tool_names().iter().any(|name| name == "export_epub"));
    let result = client.call_result("export_epub", json!({ "path": page, "output": "a.epub" }));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "permission_denied",
        "{result}"
    );
    assert!(!root.join("a.epub").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    assert!(client.tool_names().iter().any(|name| name == "export_epub"));

    // A page as an article.
    let exported = client
        .call(
            "export_epub",
            json!({ "path": page, "output": "install.epub" }),
        )
        .unwrap();
    assert_eq!(exported["output"], "install.epub", "{exported}");
    assert_eq!(exported["pages"], 1, "{exported}");
    assert_eq!(exported["sources"], json!([page]), "{exported}");
    assert_eq!(exported["protected"], false, "{exported}");
    assert!(exported["warnings"].is_array(), "{exported}");
    assert!(!epub_title(&root.join("install.epub")).is_empty());

    // A component version as a book; an `.epub` there already is replaced.
    std::fs::write(root.join("guide.epub"), "an old book").unwrap();
    let exported = client
        .call(
            "export_epub",
            json!({ "component": "1.0@guide", "output": "guide.epub" }),
        )
        .unwrap();
    assert_eq!(exported["pages"], 8, "{exported}");
    assert_eq!(
        exported["sources"].as_array().unwrap().len(),
        8,
        "{exported}"
    );
    epub_title(&root.join("guide.epub"));

    // A profile writes what the engine writes.
    let exported = client
        .call("export_epub", json!({ "profile": "handbook" }))
        .unwrap();
    assert_eq!(exported["output"], "build/handbook.epub", "{exported}");
    let from_server = std::fs::read(root.join("build/handbook.epub")).unwrap();
    adoc_epub::testing::assert_valid_ocf(&from_server);
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(common::WAIT));
    let mut args = engine.epub_profile_export_args("handbook").unwrap();
    let from_engine = directory.path().join("engine.epub");
    args.output = from_engine.clone();
    engine.export_epub_blocking(args, common::WAIT).unwrap();
    assert!(
        std::fs::read(&from_engine).unwrap() == from_server,
        "the server's book differs from the engine's"
    );

    // What is refused, and what each refusal says; nothing is written or replaced.
    std::fs::write(root.join("notes.txt"), "keep me").unwrap();
    let refusals = [
        (json!({ "path": page }), "invalid_argument"),
        (
            json!({ "path": page, "component": "guide", "output": "x.epub" }),
            "invalid_argument",
        ),
        (json!({ "output": "x.epub" }), "invalid_argument"),
        (json!({ "profile": "missing" }), "not_found"),
        (
            json!({ "path": page, "output": "../escaped.epub" }),
            "outside_workspace",
        ),
        (
            json!({ "path": page, "output": "book.kfx" }),
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
        let result = client.call_result("export_epub", arguments.clone());
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
    assert!(!directory.path().join("escaped.epub").exists());

    // A link is never written through, whether it leads outside the roots or stays inside.
    #[cfg(unix)]
    {
        let outside = directory.path().join("linked.epub");
        std::os::unix::fs::symlink(&outside, root.join("out.epub")).unwrap();
        std::os::unix::fs::symlink(root.join("install.epub"), root.join("inside.epub")).unwrap();
        let installed = std::fs::read(root.join("install.epub")).unwrap();
        // The one outside is outside the roots once followed; the one inside is a link.
        for (output, code) in [
            ("out.epub", "outside_workspace"),
            ("inside.epub", "invalid_argument"),
        ] {
            let result =
                client.call_result("export_epub", json!({ "path": page, "output": output }));
            assert_eq!(result["isError"], true, "{output}: {result}");
            assert_eq!(result["_meta"]["error"]["code"], code, "{output}: {result}");
        }
        assert!(!outside.exists());
        assert_eq!(std::fs::read(root.join("install.epub")).unwrap(), installed);
    }
    client.stop();

    // A page that asks to be read-only is refused, naming the page and line, since an EPUB
    // carries no editing restriction; the book already there is kept.
    let source = root.join(page);
    let text = std::fs::read_to_string(&source).unwrap();
    let (title, rest) = text.split_once('\n').unwrap();
    std::fs::write(&source, format!("{title}\n:export-read-only: true\n{rest}")).unwrap();
    std::fs::write(root.join("kept.epub"), "kept").unwrap();
    let mut client = Client::start(&root, true);
    let result = client.call_result(
        "export_epub",
        json!({ "path": page, "output": "kept.epub" }),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "export_protection_unsupported",
        "{result}"
    );
    assert_eq!(
        result["_meta"]["error"]["details"]["requested_by"],
        json!([{ "path": page, "line": 2 }]),
        "{result}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("kept.epub")).unwrap(),
        "kept"
    );
    client.stop();
}
