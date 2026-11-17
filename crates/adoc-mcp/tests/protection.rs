//! Read-only exports through the MCP server, READONLY 5/5 (`docs/prd/PRD-AdocFoundry_READONLY.md`
//! §10, test 23): each refusal's stable code and the pages it names, the write gate, and no
//! downgrade through a profile.

mod common;

use std::path::{Path, PathBuf};

use common::Client;
use serde_json::{json, Value};

/// A component `docs` 1.0 whose pages `a`, `b` and `c` have the headers given.
fn workspace(headers: [&str; 3], settings: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    let pages = root.join("docs/modules/ROOT/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        root.join("docs/antora.yml"),
        "name: docs\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docs/modules/ROOT/nav.adoc"),
        "* xref:a.adoc[]\n* xref:b.adoc[]\n* xref:c.adoc[]\n",
    )
    .unwrap();
    for (name, header) in ["a", "b", "c"].into_iter().zip(headers) {
        std::fs::write(
            pages.join(format!("{name}.adoc")),
            format!("= Page {name}\n{header}\nText.\n"),
        )
        .unwrap();
    }
    std::fs::write(root.join(".adocfoundry.toml"), settings).unwrap();
    (directory, root)
}

const ON: &str = ":export-read-only: true\n";

fn page(name: &str) -> String {
    format!("docs/modules/ROOT/pages/{name}.adoc")
}

/// The error a call ended with: its code and details.
fn error(result: &Value) -> (&str, &Value) {
    assert_eq!(result["isError"], true, "{result}");
    (
        result["_meta"]["error"]["code"].as_str().unwrap(),
        &result["_meta"]["error"]["details"],
    )
}

fn start(root: &Path) -> Client {
    let home = tempfile::tempdir().unwrap().keep();
    let env: [(&str, Option<&std::ffi::OsStr>); 2] = [
        ("HOME", Some(home.as_os_str())),
        ("XDG_CONFIG_HOME", Some(home.as_os_str())),
    ];
    Client::start_with_env(root, &["--allow-write"], "2025-06-18", &env).0
}

#[test]
fn protection_mcp_errors() {
    let settings = "[docx.profiles.unlocking]\npage = \"docs/modules/ROOT/pages/a.adoc\"\noutput = \"profile.docx\"\n\
                    [docx.profiles.unlocking.attributes]\nexport-read-only = \"false\"\n";
    let (_directory, root) = workspace([ON, "", ""], settings);
    let mut client = start(&root);

    // A protected page: Word carries it, and says so.
    let exported = client
        .call(
            "export_docx",
            json!({ "path": page("a"), "output": "a.docx" }),
        )
        .unwrap();
    assert_eq!(exported["protected"], true, "{exported}");
    let plain = client
        .call(
            "export_docx",
            json!({ "path": page("b"), "output": "b.docx" }),
        )
        .unwrap();
    assert_eq!(plain["protected"], false, "{plain}");

    // PDF cannot carry it yet: refused, naming the page and line, and nothing written.
    std::fs::write(root.join("a.pdf"), "kept").unwrap();
    let result = client.call_result(
        "export_pdf",
        json!({ "path": page("a"), "output": "a.pdf" }),
    );
    let (code, details) = error(&result);
    assert_eq!(code, "export_protection_unsupported");
    assert_eq!(
        details["requested_by"],
        json!([{ "path": page("a"), "line": 2 }])
    );
    assert_eq!(std::fs::read_to_string(root.join("a.pdf")).unwrap(), "kept");

    // PDF/A forbids it.
    let result = client.call_result(
        "export_pdf",
        json!({ "path": page("a"), "output": "archive.pdf", "standards": ["pdf-a-1b"] }),
    );
    assert_eq!(error(&result).0, "export_protection_pdfa_conflict");
    assert!(!root.join("archive.pdf").exists());

    // A book mixing pages that ask with pages that do not.
    let result = client.call_result(
        "export_docx",
        json!({ "component": "1.0@docs", "output": "book.docx" }),
    );
    let (code, details) = error(&result);
    assert_eq!(code, "export_protection_mixed");
    assert_eq!(details["requested_by"][0]["path"], page("a"));
    assert_eq!(details["editable"], json!([page("b"), page("c")]));
    assert!(!root.join("book.docx").exists());

    // A profile's attributes do not unlock the page.
    let exported = client
        .call("export_docx", json!({ "profile": "unlocking" }))
        .unwrap();
    assert_eq!(exported["protected"], true, "{exported}");
    client.stop();

    // An invalid value, located.
    std::fs::write(
        root.join(page("c")),
        "= Page c\n:export-read-only: yes\n\nText.\n",
    )
    .unwrap();
    let mut client = start(&root);
    let result = client.call_result(
        "export_docx",
        json!({ "path": page("c"), "output": "c.docx" }),
    );
    let (code, details) = error(&result);
    assert_eq!(code, "export_protection_invalid_header");
    assert_eq!(details["source"], json!({ "path": page("c"), "line": 2 }));
    client.stop();

    // The tools stay write-gated.
    let mut client = Client::start(&root, false);
    let result = client.call_result(
        "export_docx",
        json!({ "path": page("a"), "output": "x.docx" }),
    );
    assert_eq!(error(&result).0, "permission_denied");
    client.stop();
}
