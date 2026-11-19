//! `export_docbook`: DocBook export through the server, listed and callable only with
//! `--allow-write`, confined to the roots like every write, its pictures always copied beside
//! the document.

mod common;

use common::{copy_tree, fixture, Client};
use serde_json::json;

/// The DocBook document at `path`, checked against the schema.
fn docbook(path: &std::path::Path) -> String {
    let xml = std::fs::read_to_string(path).unwrap();
    adoc_docbook::testing::assert_valid(&xml);
    xml
}

#[test]
fn docbook_mcp_export_is_write_gated() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("reference"));
    copy_tree(&fixture("docx/reference"), &root);
    // `[docbook] assets = "reference"` is the command line's to honour; the server copies.
    std::fs::write(
        root.join(".adocfoundry.toml"),
        "[docbook]\nassets = \"reference\"\n\n[docbook.profiles.handbook]\ncomponent = \"2.0@manual\"\noutput = \"build/handbook.xml\"\n",
    )
    .unwrap();
    let page = "manual/modules/ROOT/pages/media.adoc";

    // Without `--allow-write`: not listed, and refused when called anyway.
    let mut client = Client::start(&root, false);
    assert!(!client
        .tool_names()
        .iter()
        .any(|name| name == "export_docbook"));
    let result = client.call_result("export_docbook", json!({ "path": page, "output": "a.xml" }));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["_meta"]["error"]["code"], "permission_denied",
        "{result}"
    );
    assert!(!root.join("a.xml").exists());
    client.stop();

    let mut client = Client::start(&root, true);
    assert!(client
        .tool_names()
        .iter()
        .any(|name| name == "export_docbook"));

    // A page as an article, its pictures copied whatever the settings say.
    let exported = client
        .call(
            "export_docbook",
            json!({ "path": page, "output": "media.xml" }),
        )
        .unwrap();
    assert_eq!(exported["output"], "media.xml", "{exported}");
    assert_eq!(exported["assets"], "media_assets", "{exported}");
    assert_eq!(exported["pages"], 1, "{exported}");
    assert_eq!(exported["sources"], json!([page]), "{exported}");
    assert_eq!(exported["protected"], false, "{exported}");
    let xml = docbook(&root.join("media.xml"));
    assert!(xml.contains("fileref=\"media_assets/i1.png\""), "{xml}");
    assert!(root.join("media_assets/i1.png").is_file());

    // A component version as a book.
    let exported = client
        .call(
            "export_docbook",
            json!({ "component": "2.0@manual", "output": "manual.xml" }),
        )
        .unwrap();
    assert!(exported["pages"].as_u64().unwrap() >= 6, "{exported}");
    assert!(docbook(&root.join("manual.xml")).contains("<book "));

    // A profile writes what the engine writes with its pictures copied.
    let exported = client
        .call("export_docbook", json!({ "profile": "handbook" }))
        .unwrap();
    assert_eq!(exported["output"], "build/handbook.xml", "{exported}");
    let from_server = std::fs::read(root.join("build/handbook.xml")).unwrap();
    let mut settings = adoc_engine::Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = adoc_engine::Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(common::WAIT));
    let mut args = engine.docbook_profile_export_args("handbook").unwrap();
    let from_engine = root.join("build/engine/handbook.xml");
    args.output = from_engine.clone();
    args.assets = Some(adoc_engine::DocBookAssets::Copy);
    engine.export_docbook_blocking(args, common::WAIT).unwrap();
    assert!(
        String::from_utf8(std::fs::read(&from_engine).unwrap()).unwrap()
            == String::from_utf8(from_server).unwrap(),
        "the server's document differs from the engine's"
    );

    // What is refused, and what each refusal says; nothing is written or replaced.
    std::fs::write(root.join("notes.txt"), "keep me").unwrap();
    std::fs::create_dir(root.join("taken_assets")).unwrap();
    std::fs::write(root.join("taken_assets/mine.txt"), "mine").unwrap();
    let refusals = [
        (json!({ "path": page }), "invalid_argument"),
        (
            json!({ "path": page, "component": "manual", "output": "x.xml" }),
            "invalid_argument",
        ),
        (json!({ "output": "x.xml" }), "invalid_argument"),
        (json!({ "profile": "missing" }), "not_found"),
        (
            json!({ "path": page, "output": "../escaped.xml" }),
            "outside_workspace",
        ),
        (
            json!({ "path": page, "output": "book.dbk" }),
            "invalid_argument",
        ),
        (
            json!({ "path": page, "output": "notes.txt" }),
            "invalid_argument",
        ),
        (json!({ "path": page, "output": page }), "invalid_argument"),
        (
            json!({ "path": page, "output": "taken.xml" }),
            "invalid_argument",
        ),
    ];
    let before = common::snapshot(&root);
    for (arguments, code) in refusals {
        let result = client.call_result("export_docbook", arguments.clone());
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
    assert!(!directory.path().join("escaped.xml").exists());

    // A link is never written through, as the document or as its assets folder.
    #[cfg(unix)]
    {
        let outside = directory.path().join("linked.xml");
        std::os::unix::fs::symlink(&outside, root.join("out.xml")).unwrap();
        std::os::unix::fs::symlink(root.join("media.xml"), root.join("inside.xml")).unwrap();
        let pictures = directory.path().join("pictures");
        std::fs::create_dir(&pictures).unwrap();
        std::os::unix::fs::symlink(&pictures, root.join("folder_assets")).unwrap();
        let written = std::fs::read(root.join("media.xml")).unwrap();
        for (output, code) in [
            ("out.xml", "outside_workspace"),
            ("inside.xml", "invalid_argument"),
            ("folder.xml", "outside_workspace"),
        ] {
            let result =
                client.call_result("export_docbook", json!({ "path": page, "output": output }));
            assert_eq!(result["isError"], true, "{output}: {result}");
            assert_eq!(result["_meta"]["error"]["code"], code, "{output}: {result}");
        }
        assert!(!outside.exists());
        assert_eq!(std::fs::read_dir(&pictures).unwrap().count(), 0);
        assert_eq!(std::fs::read(root.join("media.xml")).unwrap(), written);
    }
    client.stop();

    // A page that asks to be read-only is refused, naming the page and line.
    let source = root.join(page);
    let text = std::fs::read_to_string(&source).unwrap();
    let (title, rest) = text.split_once('\n').unwrap();
    std::fs::write(&source, format!("{title}\n:export-read-only: true\n{rest}")).unwrap();
    std::fs::write(root.join("kept.xml"), "kept").unwrap();
    let mut client = Client::start(&root, true);
    let result = client.call_result(
        "export_docbook",
        json!({ "path": page, "output": "kept.xml" }),
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
        std::fs::read_to_string(root.join("kept.xml")).unwrap(),
        "kept"
    );
    client.stop();
}
